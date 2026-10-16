//! File-system abstraction ("VFS", a name borrowed from SQLite).
//!
//! Everything toydb persists goes through the [`Vfs`] and [`File`] traits
//! instead of `std::fs` directly. That indirection buys us one very valuable
//! thing: [`MemVfs`], an in-memory file system that can *simulate a power
//! failure*. It remembers which writes were made durable by `sync()` and, on
//! [`MemVfs::crash`], throws away (or tears) everything that wasn't — exactly
//! what a real disk is allowed to do. The crash-recovery tests are built on it.

use std::collections::HashMap;
use std::fmt;
use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::util::Rng;

/// A random-access file.
pub trait File: Send {
    /// Read up to `buf.len()` bytes starting at `offset`. Returns the number
    /// of bytes read, which is short only at end of file.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
    /// Write all of `buf` at `offset`, extending the file if necessary.
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()>;
    /// Current size in bytes.
    fn size(&mut self) -> io::Result<u64>;
    /// Truncate or extend the file.
    fn set_len(&mut self, len: u64) -> io::Result<()>;
    /// Make all previous writes durable (`fsync`).
    fn sync(&mut self) -> io::Result<()>;
    /// Try to take an exclusive advisory lock so two *processes* cannot open
    /// the same database. Returns `Ok(false)` if someone else holds it.
    fn try_lock(&mut self) -> io::Result<bool>;
}

/// A file system: something that can open files by path.
pub trait Vfs: Send + Sync + fmt::Debug {
    /// Open `path` for reading and writing, creating it if it doesn't exist.
    fn open(&self, path: &Path) -> io::Result<Box<dyn File>>;
}

// ---------------------------------------------------------------------------
// The real thing: std::fs
// ---------------------------------------------------------------------------

/// The operating system's file system.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsVfs;

struct OsFile(std::fs::File);

impl Vfs for OsVfs {
    fn open(&self, path: &Path) -> io::Result<Box<dyn File>> {
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        Ok(Box::new(OsFile(f)))
    }
}

impl File for OsFile {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.0.seek(SeekFrom::Start(offset))?;
        let mut done = 0;
        while done < buf.len() {
            match self.0.read(&mut buf[done..]) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(done)
    }
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
        self.0.seek(SeekFrom::Start(offset))?;
        self.0.write_all(buf)
    }
    fn size(&mut self) -> io::Result<u64> {
        Ok(self.0.metadata()?.len())
    }
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.0.set_len(len)
    }
    fn sync(&mut self) -> io::Result<()> {
        // sync_data is enough: we never depend on mtime etc., and file
        // length changes are covered by fdatasync on Linux.
        self.0.sync_data()
    }
    fn try_lock(&mut self) -> io::Result<bool> {
        match self.0.try_lock() {
            Ok(()) => Ok(true),
            Err(std::fs::TryLockError::WouldBlock) => Ok(false),
            Err(std::fs::TryLockError::Error(e)) => Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// In-memory VFS with power-failure simulation
// ---------------------------------------------------------------------------

/// One not-yet-durable modification to a file.
#[derive(Clone, Debug)]
enum PendingOp {
    Write { offset: u64, data: Vec<u8> },
    SetLen(u64),
}

#[derive(Default, Debug)]
struct MemFileData {
    /// What reads currently observe (as if served from the OS page cache).
    current: Vec<u8>,
    /// What is guaranteed to survive a crash.
    durable: Vec<u8>,
    /// Operations since the last `sync`, oldest first.
    pending: Vec<PendingOp>,
}

#[derive(Default, Debug)]
struct MemState {
    files: HashMap<PathBuf, MemFileData>,
    /// When `Some(n)`, the "power" fails after `n` more mutating operations:
    /// from then on every operation returns an error until [`MemVfs::crash`].
    ops_until_power_loss: Option<u64>,
    powered_off: bool,
}

/// An in-memory file system that can simulate crashes.
///
/// Cloning a `MemVfs` gives another handle to the *same* file system, so a
/// test can keep a handle, drop the database, call [`MemVfs::crash`], and
/// reopen the database on the surviving bytes.
#[derive(Clone, Default, Debug)]
pub struct MemVfs {
    state: Arc<Mutex<MemState>>,
}

struct MemFile {
    state: Arc<Mutex<MemState>>,
    path: PathBuf,
}

const SECTOR: usize = 512;

fn apply(buf: &mut Vec<u8>, op: &PendingOp) {
    match op {
        PendingOp::Write { offset, data } => {
            let end = *offset as usize + data.len();
            if buf.len() < end {
                buf.resize(end, 0);
            }
            buf[*offset as usize..end].copy_from_slice(data);
        }
        PendingOp::SetLen(len) => buf.resize(*len as usize, 0),
    }
}

fn power_loss_err() -> io::Error {
    io::Error::other("simulated power loss")
}

impl MemVfs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Arrange for the power to fail after `n` more writes/syncs/truncates.
    /// The operation that hits the limit fails and so does everything after
    /// it, until [`MemVfs::crash`] is called.
    pub fn fail_after(&self, n: u64) {
        self.state.lock().unwrap().ops_until_power_loss = Some(n);
    }

    /// Simulate a power failure and reboot.
    ///
    /// For every file, each operation issued since the last `sync` is
    /// independently either lost, fully persisted, or — for writes — torn at
    /// 512-byte sector granularity. The resulting bytes become both the
    /// durable and the visible contents, and power is restored.
    pub fn crash(&self, seed: u64) {
        let mut rng = Rng::new(seed);
        let mut st = self.state.lock().unwrap();
        for f in st.files.values_mut() {
            let mut img = f.durable.clone();
            for op in f.pending.drain(..) {
                match rng.below(3) {
                    0 => {} // lost entirely
                    1 => apply(&mut img, &op),
                    _ => match &op {
                        // Torn write: some sectors made it, some didn't.
                        PendingOp::Write { offset, data } => {
                            for (i, chunk) in data.chunks(SECTOR).enumerate() {
                                if rng.chance(1, 2) {
                                    apply(
                                        &mut img,
                                        &PendingOp::Write {
                                            offset: offset + (i * SECTOR) as u64,
                                            data: chunk.to_vec(),
                                        },
                                    );
                                }
                            }
                        }
                        PendingOp::SetLen(_) => apply(&mut img, &op),
                    },
                }
            }
            f.current = img.clone();
            f.durable = img;
        }
        st.ops_until_power_loss = None;
        st.powered_off = false;
    }

    /// Size of a file, if it exists (handy in tests).
    pub fn file_len(&self, path: &Path) -> Option<usize> {
        self.state.lock().unwrap().files.get(path).map(|f| f.current.len())
    }
}

impl Vfs for MemVfs {
    fn open(&self, path: &Path) -> io::Result<Box<dyn File>> {
        let mut st = self.state.lock().unwrap();
        if st.powered_off {
            return Err(power_loss_err());
        }
        st.files.entry(path.to_path_buf()).or_default();
        Ok(Box::new(MemFile {
            state: self.state.clone(),
            path: path.to_path_buf(),
        }))
    }
}

impl MemFile {
    /// Run `f` on this file's data, honouring the power-loss countdown for
    /// mutating operations.
    fn with<T>(
        &self,
        mutating: bool,
        f: impl FnOnce(&mut MemFileData) -> T,
    ) -> io::Result<T> {
        let mut st = self.state.lock().unwrap();
        if st.powered_off {
            return Err(power_loss_err());
        }
        if mutating {
            if let Some(n) = st.ops_until_power_loss.as_mut() {
                if *n == 0 {
                    st.powered_off = true;
                    return Err(power_loss_err());
                }
                *n -= 1;
            }
        }
        let data = st.files.get_mut(&self.path).expect("file exists");
        Ok(f(data))
    }
}

impl File for MemFile {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.with(false, |d| {
            let start = (offset as usize).min(d.current.len());
            let end = (start + buf.len()).min(d.current.len());
            buf[..end - start].copy_from_slice(&d.current[start..end]);
            end - start
        })
    }
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
        self.with(true, |d| {
            let op = PendingOp::Write {
                offset,
                data: buf.to_vec(),
            };
            apply(&mut d.current, &op);
            d.pending.push(op);
        })
    }
    fn size(&mut self) -> io::Result<u64> {
        self.with(false, |d| d.current.len() as u64)
    }
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        self.with(true, |d| {
            let op = PendingOp::SetLen(len);
            apply(&mut d.current, &op);
            d.pending.push(op);
        })
    }
    fn sync(&mut self) -> io::Result<()> {
        self.with(true, |d| {
            d.durable = d.current.clone();
            d.pending.clear();
        })
    }
    fn try_lock(&mut self) -> io::Result<bool> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsynced_writes_can_be_lost() {
        let vfs = MemVfs::new();
        let p = Path::new("f");
        let mut f = vfs.open(p).unwrap();
        f.write_at(0, b"durable").unwrap();
        f.sync().unwrap();
        f.write_at(0, &[b'x'; 4096]).unwrap();
        drop(f);
        // Try a few seeds: across them we should see both outcomes.
        let mut saw_lost = false;
        let mut saw_kept = false;
        for seed in 0..32 {
            let v2 = MemVfs::new();
            let mut f = v2.open(p).unwrap();
            f.write_at(0, b"durable").unwrap();
            f.sync().unwrap();
            f.write_at(0, &[b'x'; 4096]).unwrap();
            v2.crash(seed);
            let mut buf = [0u8; 7];
            f.read_at(0, &mut buf).unwrap();
            if &buf == b"durable" {
                saw_lost = true;
            } else {
                saw_kept = true;
            }
        }
        assert!(saw_lost && saw_kept);
    }

    #[test]
    fn power_loss_fails_operations() {
        let vfs = MemVfs::new();
        let mut f = vfs.open(Path::new("f")).unwrap();
        vfs.fail_after(1);
        f.write_at(0, b"a").unwrap();
        assert!(f.write_at(0, b"b").is_err());
        assert!(f.sync().is_err());
        vfs.crash(0);
        f.write_at(0, b"c").unwrap();
    }
}
