//! Crash-recovery tests for the storage engine.
//!
//! We run random transactions against a B+tree stored in a [`MemVfs`], cut
//! the power after a random number of I/O operations, let the simulated disk
//! lose or tear every write that wasn't fsynced, then reopen and check that
//! the database contains exactly the committed transactions — plus, possibly,
//! the one whose commit was in flight when the power went out.

use std::collections::BTreeMap;
use std::path::Path;

use toydb::btree::BTree;
use toydb::storage::file::{MemVfs, OsVfs};
use toydb::storage::{Storage, StorageOptions};
use toydb::util::Rng;

type Model = BTreeMap<Vec<u8>, Vec<u8>>;

fn opts() -> StorageOptions {
    StorageOptions {
        pool_pages: 16,          // tiny pool: lots of eviction
        checkpoint_bytes: 64 << 10, // frequent checkpoints
        ..Default::default()
    }
}

fn open(vfs: &MemVfs) -> toydb::Result<Storage> {
    Storage::open(vfs, Path::new("crash.db"), opts())
}

/// Read the whole tree back.
fn contents(s: &mut Storage) -> Model {
    let tree = BTree::open(s.catalog_root().unwrap());
    tree.check(s).expect("tree invariants hold after recovery");
    tree.scan(s).unwrap().collect(s).unwrap().into_iter().collect()
}

/// Run one random transaction starting from committed state `base`.
/// `pending` is filled in with the state the transaction produces, so that
/// if the power fails *inside* `commit()` the caller knows the only other
/// state recovery may legitimately produce. Returns whether it committed
/// (it may also roll back on purpose).
fn run_txn(s: &mut Storage, rng: &mut Rng, base: &Model, pending: &mut Model) -> toydb::Result<bool> {
    let tree = BTree::open(s.catalog_root()?);
    *pending = base.clone();
    for _ in 0..1 + rng.below(20) {
        let k = format!("k{:04}", rng.below(500)).into_bytes();
        if rng.chance(1, 4) {
            tree.delete(s, &k)?;
            pending.remove(&k);
        } else {
            // Mostly small values, occasionally one that needs overflow pages.
            let len = if rng.chance(1, 20) { 5000 } else { rng.below(200) as usize };
            let v = vec![rng.below(256) as u8; len];
            tree.insert(s, &k, &v)?;
            pending.insert(k, v);
        }
    }
    if rng.chance(1, 6) {
        s.rollback();
        *pending = base.clone();
        return Ok(false);
    }
    s.commit()?;
    Ok(true)
}

#[test]
fn random_power_failures() {
    for seed in 0..150u64 {
        let mut rng = Rng::new(seed);
        let vfs = MemVfs::new();
        let mut committed = Model::new();
        {
            let mut s = open(&vfs).unwrap();
            let tree = BTree::create(&mut s).unwrap();
            s.set_catalog_root(tree.root()).unwrap();
            s.commit().unwrap();
        }
        // Several crash/recover cycles per seed.
        for cycle in 0..3 {
            let mut s = open(&vfs).unwrap();
            assert_eq!(contents(&mut s), committed, "seed {seed}: wrong state after recovery");
            vfs.fail_after(rng.below(3000));
            let mut pending = Model::new();
            loop {
                match run_txn(&mut s, &mut rng, &committed, &mut pending) {
                    Ok(true) => committed = pending.clone(),
                    Ok(false) => {}
                    Err(_) => break, // power is out
                }
            }
            drop(s); // Drop tries to checkpoint; it fails harmlessly.
            vfs.crash(seed * 31 + cycle);
            let mut s = open(&vfs).unwrap();
            let got = contents(&mut s);
            // Either the interrupted transaction reached its commit point
            // (then all of it is there) or it didn't (then none of it is).
            assert!(
                got == committed || got == pending,
                "seed {seed} cycle {cycle}: recovered state is neither before nor after the in-flight txn"
            );
            committed = got;
        }
    }
}

/// Stronger check for the in-flight case: record the exact model of the
/// interrupted transaction and require the recovered state to be one of the
/// two candidates.
#[test]
fn in_flight_transaction_is_all_or_nothing() {
    for seed in 0..300u64 {
        let mut rng = Rng::new(seed ^ 0xABCDEF);
        let vfs = MemVfs::new();
        let mut s = open(&vfs).unwrap();
        let tree = BTree::create(&mut s).unwrap();
        s.set_catalog_root(tree.root()).unwrap();
        s.commit().unwrap();

        let mut committed = Model::new();
        // Some committed history first.
        let mut pending = Model::new();
        for _ in 0..rng.below(10) {
            if run_txn(&mut s, &mut rng, &committed, &mut pending).unwrap() {
                committed = pending.clone();
            }
        }
        // Now one big transaction during which the power fails at a random
        // operation (possibly inside commit, possibly before it).
        let mut target = committed.clone();
        for i in 0..300u32 {
            let k = format!("big{i:05}").into_bytes();
            let v = vec![(i % 256) as u8; 40];
            tree.insert(&mut s, &k, &v).unwrap();
            target.insert(k, v);
        }
        vfs.fail_after(rng.below(40));
        let result = s.commit();
        drop(s);
        vfs.crash(seed);
        let mut s = open(&vfs).unwrap();
        let got = contents(&mut s);
        if result.is_ok() {
            assert_eq!(got, target, "seed {seed}: acknowledged commit was lost");
        } else {
            assert!(
                got == committed || got == target,
                "seed {seed}: partial transaction visible after recovery"
            );
        }
    }
}

#[test]
fn checkpoint_truncates_wal() {
    let vfs = MemVfs::new();
    let mut s = open(&vfs).unwrap();
    let tree = BTree::create(&mut s).unwrap();
    s.set_catalog_root(tree.root()).unwrap();
    s.commit().unwrap();
    for i in 0..200u32 {
        tree.insert(&mut s, &i.to_be_bytes(), &[1u8; 100]).unwrap();
        s.commit().unwrap();
        assert!(s.wal_size() < opts().checkpoint_bytes + 64 * 1024);
    }
    s.checkpoint().unwrap();
    assert!(s.wal_size() <= 64);
    drop(s);
    let wal_len = vfs.file_len(Path::new("crash.db-wal")).unwrap();
    assert!(wal_len <= 64, "wal not truncated: {wal_len}");
}

/// A real-file-system test: "kill -9" the process by leaking the Storage
/// (so no clean-shutdown checkpoint happens), then reopen and recover.
#[test]
fn recovery_after_process_kill_on_real_files() {
    let dir = std::env::temp_dir().join(format!("toydb-crash-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("kill.db");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(toydb::storage::wal_path(&path));
    let big_ckpt = StorageOptions {
        checkpoint_bytes: u64::MAX, // never checkpoint: everything stays in the WAL
        ..Default::default()
    };
    {
        let mut s = Storage::open(&OsVfs, &path, big_ckpt.clone()).unwrap();
        let tree = BTree::create(&mut s).unwrap();
        s.set_catalog_root(tree.root()).unwrap();
        s.commit().unwrap();
        for i in 0..1000u32 {
            tree.insert(&mut s, &i.to_be_bytes(), &i.to_le_bytes()).unwrap();
        }
        s.commit().unwrap();
        tree.insert(&mut s, b"uncommitted", b"!").unwrap();
        // Simulate the process dying: no Drop, so no rollback and no
        // checkpoint. (The leaked handle keeps its file lock, which is why
        // we recover from a copy of the files below.)
        std::mem::forget(s);
    }
    assert!(std::fs::metadata(toydb::storage::wal_path(&path)).unwrap().len() > 4096);
    let copy = dir.join("kill-copy.db");
    std::fs::copy(&path, &copy).unwrap();
    std::fs::copy(toydb::storage::wal_path(&path), toydb::storage::wal_path(&copy)).unwrap();
    let mut s = Storage::open(&OsVfs, &copy, big_ckpt).unwrap();
    let tree = BTree::open(s.catalog_root().unwrap());
    tree.check(&mut s).unwrap();
    assert_eq!(tree.get(&mut s, &999u32.to_be_bytes()).unwrap().unwrap(), 999u32.to_le_bytes());
    assert_eq!(tree.get(&mut s, b"uncommitted").unwrap(), None);
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}
