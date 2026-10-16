//! Transactions, locking and crash safety through the SQL interface.

use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use toydb::storage::file::MemVfs;
use toydb::storage::StorageOptions;
use toydb::util::Rng;
use toydb::{Database, Error, Options, QueryResult, Session, Value};

fn opts(timeout_ms: u64) -> Options {
    Options {
        lock_timeout: Duration::from_millis(timeout_ms),
        ..Options::default()
    }
}

fn count(s: &mut Session, table: &str) -> i64 {
    int(s, &format!("SELECT COUNT(*) FROM {table}"))
}

fn int(s: &mut Session, sql: &str) -> i64 {
    match s.execute(sql).unwrap() {
        QueryResult::Rows { rows, .. } => match rows[0][0] {
            Value::Integer(i) => i,
            ref v => panic!("{v:?}"),
        },
        r => panic!("{r:?}"),
    }
}

#[test]
fn commit_and_rollback() {
    let vfs = MemVfs::new();
    let db = Database::open_vfs(&vfs, "t.db", opts(100)).unwrap();
    let mut s = db.session();
    s.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)").unwrap();

    s.execute("BEGIN").unwrap();
    assert!(s.in_transaction());
    s.execute("INSERT INTO t VALUES (1)").unwrap();
    s.execute("INSERT INTO t VALUES (2)").unwrap();
    assert_eq!(count(&mut s, "t"), 2); // a transaction sees its own writes
    s.execute("ROLLBACK").unwrap();
    assert_eq!(count(&mut s, "t"), 0);

    s.execute("BEGIN").unwrap();
    s.execute("INSERT INTO t VALUES (3)").unwrap();
    s.execute("COMMIT").unwrap();
    assert_eq!(count(&mut s, "t"), 1);

    // Misuse is reported.
    assert!(matches!(s.execute("COMMIT"), Err(Error::Txn(_))));
    assert!(matches!(s.execute("ROLLBACK"), Err(Error::Txn(_))));
    s.execute("BEGIN").unwrap();
    assert!(matches!(s.execute("BEGIN"), Err(Error::Txn(_))));
    s.execute("ROLLBACK").unwrap();
}

#[test]
fn ddl_is_transactional() {
    let vfs = MemVfs::new();
    let db = Database::open_vfs(&vfs, "t.db", opts(100)).unwrap();
    let mut s = db.session();
    s.execute("CREATE TABLE keep (x INTEGER)").unwrap();
    s.execute("BEGIN").unwrap();
    s.execute("CREATE TABLE temp (x INTEGER)").unwrap();
    s.execute("INSERT INTO temp VALUES (1)").unwrap();
    s.execute("DROP TABLE keep").unwrap();
    s.execute("ROLLBACK").unwrap();
    assert!(s.execute("SELECT * FROM temp").is_err());
    assert_eq!(count(&mut s, "keep"), 0);
    assert_eq!(db.table_names().unwrap(), vec!["keep"]);
}

#[test]
fn failed_statement_keeps_transaction_open() {
    let vfs = MemVfs::new();
    let db = Database::open_vfs(&vfs, "t.db", opts(100)).unwrap();
    let mut s = db.session();
    s.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)").unwrap();
    s.execute("BEGIN").unwrap();
    s.execute("INSERT INTO t VALUES (1)").unwrap();
    // The second row is a duplicate: the statement fails as a whole (row 5
    // is not inserted either), but the transaction and row 1 survive.
    assert!(matches!(s.execute("INSERT INTO t VALUES (5), (1)"), Err(Error::Constraint(_))));
    assert!(s.in_transaction());
    s.execute("INSERT INTO t VALUES (2)").unwrap();
    s.execute("COMMIT").unwrap();
    assert_eq!(count(&mut s, "t"), 2);
    assert_eq!(int(&mut s, "SELECT SUM(id) FROM t"), 3);
}

#[test]
fn dropped_session_rolls_back() {
    let vfs = MemVfs::new();
    let db = Database::open_vfs(&vfs, "t.db", opts(100)).unwrap();
    db.execute("CREATE TABLE t (id INTEGER)").unwrap();
    {
        let mut s = db.session();
        s.execute("BEGIN").unwrap();
        s.execute("INSERT INTO t VALUES (1)").unwrap();
    }
    let mut s = db.session();
    assert_eq!(count(&mut s, "t"), 0);
    // And the lock was released: we can write.
    s.execute("INSERT INTO t VALUES (2)").unwrap();
}

#[test]
fn writer_blocks_readers_until_commit() {
    let vfs = MemVfs::new();
    let db = Database::open_vfs(&vfs, "t.db", opts(100)).unwrap();
    db.execute("CREATE TABLE t (id INTEGER)").unwrap();
    let mut a = db.session();
    let mut b = db.session();
    a.execute("BEGIN").unwrap();
    a.execute("INSERT INTO t VALUES (1)").unwrap();
    // B can neither read A's uncommitted row nor write.
    assert!(matches!(b.execute("SELECT * FROM t"), Err(Error::Busy(_))));
    assert!(matches!(b.execute("INSERT INTO t VALUES (2)"), Err(Error::Busy(_))));
    // B's failure didn't disturb A.
    a.execute("COMMIT").unwrap();
    assert_eq!(count(&mut b, "t"), 1);
}

#[test]
fn readers_share_and_block_writers() {
    let vfs = MemVfs::new();
    let db = Database::open_vfs(&vfs, "t.db", opts(100)).unwrap();
    db.execute("CREATE TABLE t (id INTEGER)").unwrap();
    let (mut a, mut b, mut c) = (db.session(), db.session(), db.session());
    a.execute("BEGIN").unwrap();
    b.execute("BEGIN").unwrap();
    assert_eq!(count(&mut a, "t"), 0);
    assert_eq!(count(&mut b, "t"), 0); // two readers at once
    assert!(matches!(c.execute("INSERT INTO t VALUES (1)"), Err(Error::Busy(_))));
    // Both readers try to upgrade: the one that would deadlock gets Busy
    // immediately rather than waiting for its timeout.
    let db2 = db.clone();
    let h = thread::spawn(move || {
        let _ = db2; // keep the database alive in this thread
        a.execute("INSERT INTO t VALUES (1)").map(|_| a)
    });
    thread::sleep(Duration::from_millis(20));
    let r = b.execute("INSERT INTO t VALUES (2)");
    assert!(matches!(r, Err(Error::Busy(_))), "{r:?}");
    b.execute("ROLLBACK").unwrap(); // releases b's shared lock; a proceeds
    let mut a = h.join().unwrap().unwrap();
    a.execute("COMMIT").unwrap();
    assert_eq!(count(&mut c, "t"), 1);
}

/// Many threads increment counters inside explicit transactions, retrying
/// on Busy. Strict 2PL makes this serializable, so no increment is lost.
#[test]
fn concurrent_increments_are_serializable() {
    let vfs = MemVfs::new();
    let db = Database::open_vfs(&vfs, "t.db", opts(2000)).unwrap();
    db.execute("CREATE TABLE c (id INTEGER PRIMARY KEY, n INTEGER)").unwrap();
    db.execute("INSERT INTO c VALUES (1, 0), (2, 0)").unwrap();
    let threads = 6;
    let per_thread = 25;
    let barrier = Arc::new(Barrier::new(threads));
    let handles: Vec<_> = (0..threads)
        .map(|t| {
            let db = db.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let mut s = db.session();
                barrier.wait();
                let mut done = 0;
                let mut retries = 0;
                while done < per_thread {
                    // Read-modify-write: the classic lost-update pattern.
                    let attempt = (|| -> toydb::Result<()> {
                        s.execute("BEGIN")?;
                        let n = match s.execute("SELECT n FROM c WHERE id = 1")? {
                            QueryResult::Rows { rows, .. } => match rows[0][0] {
                                Value::Integer(n) => n,
                                _ => unreachable!(),
                            },
                            _ => unreachable!(),
                        };
                        s.execute(&format!("UPDATE c SET n = {} WHERE id = 1", n + 1))?;
                        s.execute(&format!("UPDATE c SET n = n + 1 WHERE id = {}", 2))?;
                        s.execute("COMMIT")?;
                        Ok(())
                    })();
                    match attempt {
                        Ok(()) => done += 1,
                        Err(Error::Busy(_)) => {
                            if s.in_transaction() {
                                s.execute("ROLLBACK").unwrap();
                            }
                            retries += 1;
                            thread::sleep(Duration::from_micros(100 * (t as u64 + 1)));
                        }
                        Err(e) => panic!("{e}"),
                    }
                }
                retries
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let mut s = db.session();
    let expected = (threads * per_thread) as i64;
    assert_eq!(int(&mut s, "SELECT n FROM c WHERE id = 1"), expected);
    assert_eq!(int(&mut s, "SELECT n FROM c WHERE id = 2"), expected);
}

/// The classic bank test: money moves between accounts in transactions,
/// the power fails at a random moment, and after recovery the total must be
/// unchanged and the schema intact.
#[test]
fn bank_transfers_survive_power_failures() {
    for seed in 0..40u64 {
        let mut rng = Rng::new(seed);
        let vfs = MemVfs::new();
        let options = Options {
            storage: StorageOptions {
                pool_pages: 16,
                checkpoint_bytes: 32 << 10,
                ..Default::default()
            },
            ..opts(100)
        };
        let accounts = 50;
        {
            let db = Database::open_vfs(&vfs, "bank.db", options.clone()).unwrap();
            let mut s = db.session();
            s.execute("CREATE TABLE acct (id INTEGER PRIMARY KEY, owner TEXT UNIQUE, balance INTEGER NOT NULL)")
                .unwrap();
            // Every transfer also rewrites entries in this index.
            s.execute("CREATE INDEX by_balance ON acct (balance)").unwrap();
            s.execute("BEGIN").unwrap();
            for i in 0..accounts {
                s.execute(&format!("INSERT INTO acct VALUES ({i}, 'owner {i}', 1000)")).unwrap();
            }
            s.execute("COMMIT").unwrap();
        }
        for round in 0..3 {
            {
                let db = Database::open_vfs(&vfs, "bank.db", options.clone()).unwrap();
                let mut s = db.session();
                vfs.fail_after(rng.below(400));
                for _ in 0..1000 {
                    let (from, to) = (rng.below(accounts), rng.below(accounts));
                    let amt = rng.below(100);
                    let r = s.execute_script(&format!(
                        "BEGIN;
                         UPDATE acct SET balance = balance - {amt} WHERE id = {from};
                         UPDATE acct SET balance = balance + {amt} WHERE id = {to};
                         COMMIT;"
                    ));
                    if r.is_err() {
                        break; // the power is out
                    }
                }
            }
            vfs.crash(seed * 7 + round);
            let db = Database::open_vfs(&vfs, "bank.db", options.clone()).unwrap();
            let mut s = db.session();
            assert_eq!(count(&mut s, "acct"), accounts as i64, "seed {seed}");
            assert_eq!(
                int(&mut s, "SELECT SUM(balance) FROM acct"),
                accounts as i64 * 1000,
                "seed {seed} round {round}: money was created or destroyed"
            );
            let problems = s.check_integrity().unwrap();
            assert!(problems.is_empty(), "seed {seed} round {round}: {problems:?}");
            assert_eq!(
                int(&mut s, "SELECT COUNT(*) FROM acct WHERE balance >= -1000000"),
                accounts as i64,
                "index scan sees every row"
            );
        }
    }
}
