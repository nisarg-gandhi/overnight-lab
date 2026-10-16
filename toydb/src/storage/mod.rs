//! The storage engine: pages, caching, allocation, durability.
//!
//! ```text
//!   B+tree / catalog / executor
//!            |  PageStore trait: read / write / allocate / free
//!            v
//!   +------------------------------------------------------+
//!   | Storage   (header page, free list, commit/rollback)  |
//!   |   +--------------------+      +-------------------+  |
//!   |   | BufferPool (LRU)   |      | Wal (redo log)    |  |
//!   |   |   +------------+   |      +-------------------+  |
//!   |   |   | Pager      |   |                |            |
//!   |   +---+------------+---+                |            |
//!   +-----------|-----------------------------|------------+
//!               v                             v
//!          path.db (pages)              path.db-wal
//! ```

pub mod buffer;
pub mod file;
pub mod pager;
pub mod wal;

use std::path::{Path, PathBuf};

use crate::error::{corrupt, Error, Result};
use crate::util::bytes::{get_u32, get_u64, put_u32, put_u64};
use buffer::{BufferPool, PoolStats};
use file::Vfs;
pub use pager::{PageBuf, PageId, PAGE_SIZE};
use pager::{new_page_buf, Pager};
use wal::Wal;

/// What the B+tree needs from the layer below it.
///
/// Implemented by [`Storage`]; tests can implement it on a toy in-memory
/// store. Methods take `&mut self` because even a read may load a page into
/// the cache and evict another.
pub trait PageStore {
    /// Read access to a page.
    fn read(&mut self, id: PageId) -> Result<&PageBuf>;
    /// Write access to a page (marks it dirty in the current transaction).
    fn write(&mut self, id: PageId) -> Result<&mut PageBuf>;
    /// Allocate a fresh zeroed page.
    fn allocate(&mut self) -> Result<PageId>;
    /// Return a page to the free list.
    fn free(&mut self, id: PageId) -> Result<()>;
}

/// Tunables for opening a database.
#[derive(Clone, Debug)]
pub struct StorageOptions {
    /// Buffer pool capacity in pages (4 KiB each).
    pub pool_pages: usize,
    /// fsync the WAL on every commit. Turning this off is like SQLite's
    /// `synchronous=OFF`: much faster, still safe if the *process* crashes
    /// (the OS has the writes), but an OS crash or power loss may lose
    /// recent commits or even corrupt the database. Checkpoints always sync.
    pub fsync: bool,
    /// Checkpoint once the WAL grows beyond this many bytes.
    pub checkpoint_bytes: u64,
}

impl Default for StorageOptions {
    fn default() -> Self {
        StorageOptions {
            pool_pages: 1024, // 4 MiB of cache
            fsync: true,
            checkpoint_bytes: 4 << 20,
        }
    }
}

// --- Header page (page 0) ----------------------------------------------------
//
//   offset  size  field
//   0       8     magic "TOYDB\0v1"
//   8       4     page size (always 4096; checked on open)
//   12      4     page count (pages in use, including this one)
//   16      4     free list head (0 = empty)
//   20      4     free list length
//   24      4     catalog root page (0 = not created yet)
//   28      8     last committed transaction id
//   36..4096      zero

const MAGIC: &[u8; 8] = b"TOYDB\0v1";
const H_PAGE_SIZE: usize = 8;
const H_PAGE_COUNT: usize = 12;
const H_FREE_HEAD: usize = 16;
const H_FREE_LEN: usize = 20;
const H_CATALOG_ROOT: usize = 24;
const H_TXN_ID: usize = 28;

/// A freed page starts with this byte (helps when debugging dumps) and
/// stores the next free page id at offset 4.
const FREE_PAGE_MARKER: u8 = 0xFE;

/// The storage engine. See the module docs for the big picture.
pub struct Storage {
    pool: BufferPool,
    wal: Wal,
    opts: StorageOptions,
    /// Set when an I/O error happened half-way through a commit. From then on
    /// in-memory state can't be trusted; every call fails until reopen.
    poisoned: bool,
    /// Pre-images of pages modified by the current *statement*, so a failed
    /// statement can be undone without aborting the whole transaction.
    /// `None` while no statement savepoint is active.
    savepoint: Option<Vec<(PageId, Box<PageBuf>)>>,
    savepoint_pages: std::collections::HashSet<PageId>,
    path: PathBuf,
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Storage").field("path", &self.path).finish()
    }
}

/// Path of the WAL file belonging to database `path`.
pub fn wal_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push("-wal");
    PathBuf::from(s)
}

impl Storage {
    /// Open (or create) the database at `path`, running crash recovery if a
    /// WAL with committed transactions is present.
    pub fn open(vfs: &dyn Vfs, path: &Path, opts: StorageOptions) -> Result<Self> {
        let mut pager = Pager::new(vfs.open(path)?)?;
        pager.lock()?;
        let mut wal = Wal::open(vfs.open(&wal_path(path))?, opts.fsync)?;

        // Crash recovery: re-apply every committed transaction found in the
        // WAL to the database file, then start a fresh, empty log.
        wal.recover(&mut pager)?;

        let mut pool = BufferPool::new(pager, opts.pool_pages.max(8));
        let fresh = pool.pager().num_pages()? == 0;
        let mut s = Storage {
            pool,
            wal,
            opts,
            poisoned: false,
            savepoint: None,
            savepoint_pages: Default::default(),
            path: path.to_path_buf(),
        };
        if fresh {
            // Brand-new database: write the header page through the normal
            // commit path so that creation is itself crash-safe.
            let h = s.pool.create(0)?;
            h[..8].copy_from_slice(MAGIC);
            put_u32(h, H_PAGE_SIZE, PAGE_SIZE as u32);
            put_u32(h, H_PAGE_COUNT, 1);
            s.commit()?;
        } else {
            let h = s.pool.read(0)?;
            if &h[..8] != MAGIC {
                return Err(corrupt!("not a toydb database (bad magic)"));
            }
            if get_u32(h, H_PAGE_SIZE) as usize != PAGE_SIZE {
                return Err(corrupt!("unsupported page size {}", get_u32(h, H_PAGE_SIZE)));
            }
        }
        Ok(s)
    }

    fn check(&self) -> Result<()> {
        if self.poisoned {
            Err(Error::Poisoned)
        } else {
            Ok(())
        }
    }

    // --- header fields ----------------------------------------------------

    fn header_u32(&mut self, off: usize) -> Result<u32> {
        Ok(get_u32(self.read(0)?, off))
    }
    fn set_header_u32(&mut self, off: usize, v: u32) -> Result<()> {
        put_u32(self.write(0)?, off, v);
        Ok(())
    }

    /// Number of pages in use (the file may be longer).
    pub fn page_count(&mut self) -> Result<u32> {
        self.header_u32(H_PAGE_COUNT)
    }
    /// Number of pages on the free list.
    pub fn free_count(&mut self) -> Result<u32> {
        self.header_u32(H_FREE_LEN)
    }
    /// Root page of the catalog B+tree (0 if not created yet).
    pub fn catalog_root(&mut self) -> Result<PageId> {
        self.header_u32(H_CATALOG_ROOT)
    }
    pub fn set_catalog_root(&mut self, root: PageId) -> Result<()> {
        self.set_header_u32(H_CATALOG_ROOT, root)
    }

    // --- transactions -------------------------------------------------------

    /// Does the current transaction have uncommitted changes?
    pub fn is_dirty(&self) -> bool {
        !self.pool.dirty_page_ids().is_empty()
    }

    /// Make every change since the last commit durable.
    ///
    /// 1. Append an after-image of every dirty page to the WAL, followed by
    ///    a commit record, and fsync the WAL. *This is the commit point.*
    /// 2. Write the same pages into the database file (no fsync needed: the
    ///    WAL can redo them) and mark them clean.
    /// 3. If the WAL has grown large, checkpoint.
    pub fn commit(&mut self) -> Result<()> {
        self.check()?;
        self.savepoint = None;
        self.savepoint_pages.clear();
        let dirty = self.pool.dirty_page_ids();
        if dirty.is_empty() {
            return Ok(()); // read-only transaction: nothing to do
        }
        let result = self.commit_pages(&dirty);
        if result.is_err() {
            // We don't know how much of the commit reached disk. Refuse to
            // continue; reopening runs recovery and sorts it out.
            self.poisoned = true;
        }
        result
    }

    fn commit_pages(&mut self, dirty: &[PageId]) -> Result<()> {
        // Bump the transaction id stored in the header (page 0 is then part
        // of every write transaction, which also keeps the WAL honest).
        let txn = {
            let h = self.pool.write(0)?;
            let t = get_u64(h, H_TXN_ID) + 1;
            put_u64(h, H_TXN_ID, t);
            t
        };
        let mut ids = dirty.to_vec();
        if !ids.contains(&0) {
            ids.insert(0, 0);
        }
        // 1. WAL first.
        {
            let pool = &self.pool;
            let pages = ids.iter().map(|&id| (id, pool.peek(id).expect("dirty page resident")));
            self.wal.append_commit(txn, pages)?;
        }
        // 2. Then the database file.
        for &id in &ids {
            let data = *self.pool.peek(id).expect("resident");
            self.pool.pager().write_page(id, &data)?;
        }
        self.pool.mark_all_clean();
        // 3. Checkpoint if the log is big.
        if self.wal.size() >= self.opts.checkpoint_bytes {
            self.checkpoint()?;
        }
        Ok(())
    }

    /// Discard every change since the last commit.
    pub fn rollback(&mut self) {
        self.savepoint = None;
        self.savepoint_pages.clear();
        self.pool.discard_dirty();
    }

    /// Flush the database file and empty the WAL. Afterwards the database
    /// file alone holds every committed transaction.
    pub fn checkpoint(&mut self) -> Result<()> {
        self.check()?;
        let r = (|| {
            self.pool.pager().sync()?;
            self.wal.reset()
        })();
        if r.is_err() {
            self.poisoned = true;
        }
        r
    }

    // --- statement savepoints ----------------------------------------------

    /// Start recording page pre-images so the current statement can be
    /// undone with [`Storage::rollback_statement`].
    pub fn begin_statement(&mut self) {
        self.savepoint = Some(Vec::new());
        self.savepoint_pages.clear();
    }

    /// The statement succeeded: forget its pre-images.
    pub fn end_statement(&mut self) {
        self.savepoint = None;
        self.savepoint_pages.clear();
    }

    /// The statement failed: put back every page it modified, leaving the
    /// rest of the transaction intact.
    pub fn rollback_statement(&mut self) -> Result<()> {
        if let Some(images) = self.savepoint.take() {
            for (id, data) in images.into_iter().rev() {
                // Restored pages stay dirty: an earlier statement of the same
                // transaction may have modified them too. Committing an
                // unchanged page is harmless.
                self.pool.restore(id, &data, true)?;
            }
        }
        self.savepoint_pages.clear();
        Ok(())
    }

    /// Record `id`'s current contents if this is the statement's first
    /// modification of it.
    fn save_preimage(&mut self, id: PageId, fresh: bool) -> Result<()> {
        if self.savepoint.is_none() || !self.savepoint_pages.insert(id) {
            return Ok(());
        }
        let mut img = new_page_buf();
        if !fresh {
            img.copy_from_slice(self.pool.read(id)?);
        }
        if let Some(images) = self.savepoint.as_mut() {
            images.push((id, img));
        }
        Ok(())
    }

    // --- introspection --------------------------------------------------------

    pub fn pool_stats(&self) -> PoolStats {
        self.pool.stats
    }
    pub fn wal_size(&self) -> u64 {
        self.wal.size()
    }
    pub fn options(&self) -> &StorageOptions {
        &self.opts
    }
}

impl PageStore for Storage {
    fn read(&mut self, id: PageId) -> Result<&PageBuf> {
        self.check()?;
        self.pool.read(id)
    }

    fn write(&mut self, id: PageId) -> Result<&mut PageBuf> {
        self.check()?;
        self.save_preimage(id, false)?;
        self.pool.write(id)
    }

    fn allocate(&mut self) -> Result<PageId> {
        self.check()?;
        let head = self.header_u32(H_FREE_HEAD)?;
        let id = if head != 0 {
            // Pop the free list.
            let next = get_u32(self.read(head)?, 4);
            self.set_header_u32(H_FREE_HEAD, next)?;
            let len = self.header_u32(H_FREE_LEN)?;
            self.set_header_u32(H_FREE_LEN, len - 1)?;
            self.save_preimage(head, false)?;
            head
        } else {
            // Extend the file.
            let count = self.header_u32(H_PAGE_COUNT)?;
            if count == PageId::MAX - 1 {
                return Err(Error::Limit("database is full".into()));
            }
            self.set_header_u32(H_PAGE_COUNT, count + 1)?;
            self.save_preimage(count, true)?;
            count
        };
        self.pool.create(id)?;
        Ok(id)
    }

    fn free(&mut self, id: PageId) -> Result<()> {
        assert!(id != 0, "cannot free the header page");
        let head = self.header_u32(H_FREE_HEAD)?;
        let len = self.header_u32(H_FREE_LEN)?;
        let p = self.write(id)?;
        p.fill(0);
        p[0] = FREE_PAGE_MARKER;
        put_u32(p, 4, head);
        self.set_header_u32(H_FREE_HEAD, id)?;
        self.set_header_u32(H_FREE_LEN, len + 1)?;
        Ok(())
    }
}

impl Drop for Storage {
    fn drop(&mut self) {
        // Best effort clean shutdown: uncommitted changes are discarded, and
        // the WAL is checkpointed so the next open has nothing to replay.
        // Errors are ignored; recovery handles whatever state we leave.
        self.rollback();
        if !self.poisoned {
            let _ = self.checkpoint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use file::MemVfs;

    fn open(vfs: &MemVfs) -> Storage {
        let opts = StorageOptions {
            pool_pages: 8,
            ..Default::default()
        };
        Storage::open(vfs, Path::new("t.db"), opts).unwrap()
    }

    #[test]
    fn allocate_commit_reopen() {
        let vfs = MemVfs::new();
        {
            let mut s = open(&vfs);
            assert_eq!(s.page_count().unwrap(), 1);
            for i in 0..50u8 {
                let id = s.allocate().unwrap();
                assert_eq!(id, i as u32 + 1);
                s.write(id).unwrap()[10] = i;
            }
            s.commit().unwrap();
        }
        let mut s = open(&vfs);
        assert_eq!(s.page_count().unwrap(), 51);
        for i in 0..50u8 {
            assert_eq!(s.read(i as u32 + 1).unwrap()[10], i);
        }
    }

    #[test]
    fn rollback_discards_changes() {
        let vfs = MemVfs::new();
        let mut s = open(&vfs);
        let id = s.allocate().unwrap();
        s.write(id).unwrap()[0] = 1;
        s.commit().unwrap();
        s.write(id).unwrap()[0] = 2;
        let id2 = s.allocate().unwrap();
        assert_eq!(s.page_count().unwrap(), 3);
        s.rollback();
        assert_eq!(s.read(id).unwrap()[0], 1);
        assert_eq!(s.page_count().unwrap(), 2);
        // The rolled-back allocation is handed out again.
        assert_eq!(s.allocate().unwrap(), id2);
    }

    #[test]
    fn free_list_reuses_pages() {
        let vfs = MemVfs::new();
        let mut s = open(&vfs);
        let a = s.allocate().unwrap();
        let b = s.allocate().unwrap();
        s.free(a).unwrap();
        s.free(b).unwrap();
        assert_eq!(s.free_count().unwrap(), 2);
        assert_eq!(s.allocate().unwrap(), b); // LIFO
        assert_eq!(s.allocate().unwrap(), a);
        assert_eq!(s.free_count().unwrap(), 0);
        assert_eq!(s.allocate().unwrap(), 3);
    }

    #[test]
    fn statement_rollback_keeps_earlier_statements() {
        let vfs = MemVfs::new();
        let mut s = open(&vfs);
        let id = s.allocate().unwrap();
        s.commit().unwrap();

        s.begin_statement();
        s.write(id).unwrap()[0] = 1;
        s.end_statement();

        s.begin_statement();
        s.write(id).unwrap()[0] = 2;
        let fresh = s.allocate().unwrap();
        s.write(fresh).unwrap()[0] = 3;
        s.rollback_statement().unwrap();

        assert_eq!(s.read(id).unwrap()[0], 1);
        assert_eq!(s.page_count().unwrap(), 2);
        s.commit().unwrap();
        drop(s);
        let mut s = open(&vfs);
        assert_eq!(s.read(id).unwrap()[0], 1);
    }

    #[test]
    fn rejects_garbage_file() {
        let vfs = MemVfs::new();
        {
            let mut f = vfs.open(Path::new("t.db")).unwrap();
            f.write_at(0, &[1u8; PAGE_SIZE]).unwrap();
        }
        let err = Storage::open(&vfs, Path::new("t.db"), StorageOptions::default()).unwrap_err();
        assert!(matches!(err, Error::Corrupt(_)), "{err}");
    }
}
