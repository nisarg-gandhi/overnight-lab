//! The buffer pool: an in-memory cache of pages with LRU eviction.
//!
//! ```text
//!             page_table: HashMap<PageId, FrameId>
//!                  |
//!   frames:  [ f0: page 7  | f1: page 0 (dirty) | f2: page 3 | ... ]
//!                  ^                                  ^
//!   LRU list: head (most recent) <-> ... <-> tail (least recent)
//! ```
//!
//! Every page access goes through here. On a miss we load the page from the
//! [`Pager`] into a free frame, or evict the least-recently-used *clean*
//! frame to make room.
//!
//! **Why only clean frames?** toydb uses a *no-steal* policy: pages modified
//! by a transaction that hasn't committed yet must never reach the database
//! file, because then a crash (or ROLLBACK) would leave uncommitted data on
//! disk with no undo log to remove it. So dirty frames are effectively
//! pinned until commit. If every frame is dirty the pool grows past its
//! nominal capacity rather than failing; see DESIGN.md ("Tradeoffs").
//!
//! **Where are the pin counts?** A classic C/C++ buffer pool hands out raw
//! pointers and uses pin counts to stop a frame being evicted while someone
//! holds a pointer into it. Here, `read`/`write` return references borrowed
//! from `&mut self`, so the borrow checker statically guarantees nobody holds
//! a reference across the next pool call — which is the only time eviction
//! can happen. The B+tree copies what it needs out of the page (it decodes
//! nodes into owned structs), so this is never limiting.

use std::collections::HashMap;

use super::pager::{new_page_buf, PageBuf, PageId, Pager};
use crate::error::Result;

type FrameId = usize;
const NIL: FrameId = usize::MAX;

struct Frame {
    page_id: PageId,
    data: Box<PageBuf>,
    /// Modified since the last commit / flush.
    dirty: bool,
    // Intrusive doubly linked LRU list, by frame index.
    prev: FrameId,
    next: FrameId,
}

/// Counters exposed for tests, benchmarks and the REPL's `.stats`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

pub struct BufferPool {
    pager: Pager,
    frames: Vec<Frame>,
    page_table: HashMap<PageId, FrameId>,
    /// Most recently used frame.
    head: FrameId,
    /// Least recently used frame.
    tail: FrameId,
    /// Nominal number of frames. Exceeded only if all frames are dirty.
    capacity: usize,
    /// Frames freed by `discard_dirty`, available for reuse.
    free_frames: Vec<FrameId>,
    pub stats: PoolStats,
}

impl BufferPool {
    pub fn new(pager: Pager, capacity: usize) -> Self {
        assert!(capacity >= 2, "buffer pool needs at least two frames");
        BufferPool {
            pager,
            frames: Vec::with_capacity(capacity),
            page_table: HashMap::with_capacity(capacity),
            head: NIL,
            tail: NIL,
            capacity,
            free_frames: Vec::new(),
            stats: PoolStats::default(),
        }
    }

    pub fn pager(&mut self) -> &mut Pager {
        &mut self.pager
    }

    /// Number of frames currently holding a page.
    pub fn resident(&self) -> usize {
        self.page_table.len()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Get read access to a page, loading it if necessary.
    pub fn read(&mut self, id: PageId) -> Result<&PageBuf> {
        let f = self.fetch(id, true)?;
        Ok(&self.frames[f].data)
    }

    /// Get write access to a page, loading it if necessary. The page is
    /// marked dirty and stays in memory until [`BufferPool::mark_all_clean`]
    /// or [`BufferPool::discard_dirty`].
    pub fn write(&mut self, id: PageId) -> Result<&mut PageBuf> {
        let f = self.fetch(id, true)?;
        self.frames[f].dirty = true;
        Ok(&mut self.frames[f].data)
    }

    /// Like [`BufferPool::write`] but for a freshly allocated page: the
    /// frame is zero-filled instead of being read from disk.
    pub fn create(&mut self, id: PageId) -> Result<&mut PageBuf> {
        let f = self.fetch(id, false)?;
        self.frames[f].data.fill(0);
        self.frames[f].dirty = true;
        Ok(&mut self.frames[f].data)
    }

    /// Ids of all dirty pages, sorted.
    pub fn dirty_page_ids(&self) -> Vec<PageId> {
        let mut ids: Vec<PageId> = self
            .frames
            .iter()
            .filter(|f| f.dirty && f.page_id != PageId::MAX)
            .map(|f| f.page_id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Read a page that is known to be resident (used at commit time to
    /// collect dirty pages without disturbing LRU order).
    pub fn peek(&self, id: PageId) -> Option<&PageBuf> {
        self.page_table.get(&id).map(|&f| &*self.frames[f].data)
    }

    /// Mark every frame clean (after its contents were made safe on disk).
    pub fn mark_all_clean(&mut self) {
        for f in &mut self.frames {
            f.dirty = false;
        }
    }

    /// Throw away every dirty frame, e.g. on ROLLBACK. The next access
    /// re-reads the committed version from disk.
    pub fn discard_dirty(&mut self) {
        for id in self.dirty_page_ids() {
            let f = self.page_table.remove(&id).expect("dirty page resident");
            self.unlink(f);
            self.frames[f].dirty = false;
            self.frames[f].page_id = PageId::MAX;
            self.free_frames.push(f);
        }
    }

    /// Overwrite a resident page's contents without marking it dirty (used
    /// to restore a page snapshot that is already identical to disk).
    pub fn restore(&mut self, id: PageId, data: &PageBuf, dirty: bool) -> Result<()> {
        let f = self.fetch(id, false)?;
        self.frames[f].data.copy_from_slice(data);
        self.frames[f].dirty = dirty;
        Ok(())
    }

    /// Drop a page from the cache if it is resident and clean.
    pub fn forget(&mut self, id: PageId) {
        if let Some(&f) = self.page_table.get(&id) {
            if !self.frames[f].dirty {
                self.page_table.remove(&id);
                self.unlink(f);
                self.frames[f].page_id = PageId::MAX;
                self.free_frames.push(f);
            }
        }
    }

    /// Find or load page `id` and move it to the front of the LRU list.
    fn fetch(&mut self, id: PageId, load: bool) -> Result<FrameId> {
        if let Some(&f) = self.page_table.get(&id) {
            self.stats.hits += 1;
            self.touch(f);
            return Ok(f);
        }
        self.stats.misses += 1;
        let f = self.free_frame()?;
        if load {
            self.pager.read_page(id, &mut self.frames[f].data)?;
        }
        self.frames[f].page_id = id;
        self.frames[f].dirty = false;
        self.page_table.insert(id, f);
        self.push_front(f);
        Ok(f)
    }

    /// Get an unused frame: a recycled one, a brand-new one if we are under
    /// capacity, or the least-recently-used clean one.
    fn free_frame(&mut self) -> Result<FrameId> {
        if let Some(f) = self.free_frames.pop() {
            return Ok(f);
        }
        if self.frames.len() < self.capacity {
            return Ok(self.new_frame());
        }
        // Walk from the LRU end looking for a clean victim.
        let mut f = self.tail;
        while f != NIL {
            if !self.frames[f].dirty {
                self.stats.evictions += 1;
                self.page_table.remove(&self.frames[f].page_id);
                self.unlink(f);
                return Ok(f);
            }
            f = self.frames[f].prev;
        }
        // Everything is dirty (a large uncommitted transaction): grow.
        Ok(self.new_frame())
    }

    fn new_frame(&mut self) -> FrameId {
        self.frames.push(Frame {
            page_id: PageId::MAX,
            data: new_page_buf(),
            dirty: false,
            prev: NIL,
            next: NIL,
        });
        self.frames.len() - 1
    }

    // --- LRU list maintenance -------------------------------------------

    fn unlink(&mut self, f: FrameId) {
        let (p, n) = (self.frames[f].prev, self.frames[f].next);
        if p != NIL {
            self.frames[p].next = n;
        } else {
            self.head = n;
        }
        if n != NIL {
            self.frames[n].prev = p;
        } else {
            self.tail = p;
        }
        self.frames[f].prev = NIL;
        self.frames[f].next = NIL;
    }

    fn push_front(&mut self, f: FrameId) {
        self.frames[f].prev = NIL;
        self.frames[f].next = self.head;
        if self.head != NIL {
            self.frames[self.head].prev = f;
        }
        self.head = f;
        if self.tail == NIL {
            self.tail = f;
        }
    }

    fn touch(&mut self, f: FrameId) {
        if self.head != f {
            self.unlink(f);
            self.push_front(f);
        }
    }

    /// Page ids from most to least recently used (for tests).
    #[cfg(test)]
    fn lru_order(&self) -> Vec<PageId> {
        let mut out = vec![];
        let mut f = self.head;
        while f != NIL {
            out.push(self.frames[f].page_id);
            f = self.frames[f].next;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::file::{MemVfs, Vfs};
    use std::path::Path;

    fn pool(cap: usize) -> BufferPool {
        let vfs = MemVfs::new();
        let pager = Pager::new(vfs.open(Path::new("db")).unwrap()).unwrap();
        BufferPool::new(pager, cap)
    }

    /// Write a page and flush it, as a commit would.
    fn put(p: &mut BufferPool, id: PageId, byte: u8) {
        p.create(id).unwrap().fill(byte);
        let data = *p.peek(id).unwrap();
        p.pager().write_page(id, &data).unwrap();
        p.mark_all_clean();
    }

    #[test]
    fn lru_eviction_order() {
        let mut p = pool(3);
        for id in 0..3 {
            put(&mut p, id, id as u8);
        }
        assert_eq!(p.lru_order(), vec![2, 1, 0]);
        p.read(0).unwrap(); // 0 becomes most recent
        assert_eq!(p.lru_order(), vec![0, 2, 1]);
        put(&mut p, 3, 3); // evicts 1, the least recently used
        assert_eq!(p.lru_order(), vec![3, 0, 2]);
        assert_eq!(p.stats.evictions, 1);
        // Page 1 comes back from disk intact.
        assert!(p.read(1).unwrap().iter().all(|&b| b == 1));
        assert_eq!(p.resident(), 3);
    }

    #[test]
    fn dirty_pages_are_never_evicted() {
        let mut p = pool(2);
        p.create(0).unwrap().fill(1);
        p.create(1).unwrap().fill(2);
        // Both frames are dirty, so the pool must grow instead of evicting.
        p.create(2).unwrap().fill(3);
        assert_eq!(p.resident(), 3);
        assert_eq!(p.dirty_page_ids(), vec![0, 1, 2]);
        assert_eq!(p.stats.evictions, 0);
    }

    #[test]
    fn discard_dirty_reverts_to_disk() {
        let mut p = pool(4);
        put(&mut p, 0, 7);
        p.write(0).unwrap().fill(9);
        assert_eq!(p.read(0).unwrap()[0], 9);
        p.discard_dirty();
        assert_eq!(p.read(0).unwrap()[0], 7);
        assert!(p.dirty_page_ids().is_empty());
    }

    #[test]
    fn many_pages_through_small_pool() {
        let mut p = pool(4);
        for id in 0..100u32 {
            put(&mut p, id, (id % 251) as u8);
        }
        for id in (0..100u32).rev() {
            assert_eq!(p.read(id).unwrap()[100], (id % 251) as u8);
        }
        assert!(p.resident() <= 4);
        assert!(p.stats.evictions > 90);
    }
}
