//! The pager: a database file viewed as an array of fixed-size pages.
//!
//! ```text
//!   offset 0         4096        8192        12288
//!   +-----------+-----------+-----------+-----------+----
//!   |  page 0   |  page 1   |  page 2   |  page 3   | ...
//!   |  (header) |           |           |           |
//!   +-----------+-----------+-----------+-----------+----
//! ```
//!
//! Page `n` lives at byte offset `n * PAGE_SIZE`. The pager knows nothing
//! about what is *in* a page; it only moves whole pages between the file
//! and memory. Caching is the buffer pool's job, and deciding *when* writes
//! may happen (write-ahead logging) is the storage engine's job.

use std::fmt;

use super::file::File;
use crate::error::Result;

/// Size of every page, in bytes. 4 KiB matches the page size of most
/// operating systems and SSDs, so one page write is one device write.
pub const PAGE_SIZE: usize = 4096;

/// Identifies a page by its index in the file. Page 0 is the header.
pub type PageId = u32;

/// The raw contents of one page.
pub type PageBuf = [u8; PAGE_SIZE];

/// Allocate a zeroed page buffer on the heap (4 KiB is too big to want to
/// move around on the stack).
pub fn new_page_buf() -> Box<PageBuf> {
    Box::new([0u8; PAGE_SIZE])
}

/// Reads and writes whole pages of one file.
pub struct Pager {
    file: Box<dyn File>,
    /// Statistics, reported by `EXPLAIN`-style tooling and benchmarks.
    pub reads: u64,
    pub writes: u64,
}

impl fmt::Debug for Pager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pager")
            .field("reads", &self.reads)
            .field("writes", &self.writes)
            .finish()
    }
}

impl Pager {
    pub fn new(file: Box<dyn File>) -> Result<Self> {
        Ok(Pager {
            file,
            reads: 0,
            writes: 0,
        })
    }

    /// Number of complete pages currently in the file. A partial trailing
    /// page can only come from a torn write that was never committed
    /// (committed pages are rewritten in full by WAL replay), so we round
    /// down and ignore it.
    pub fn num_pages(&mut self) -> Result<u32> {
        let len = self.file.size()?;
        Ok((len / PAGE_SIZE as u64) as u32)
    }

    /// Read page `id` into `buf`. Pages past the end of the file read as
    /// zeroes: a page can be allocated (and committed through the WAL)
    /// before it has ever been written to the main file.
    pub fn read_page(&mut self, id: PageId, buf: &mut PageBuf) -> Result<()> {
        self.reads += 1;
        let n = self.file.read_at(id as u64 * PAGE_SIZE as u64, buf)?;
        if n < PAGE_SIZE {
            buf[n..].fill(0);
        }
        Ok(())
    }

    /// Write `buf` as page `id`. Not durable until [`Pager::sync`].
    pub fn write_page(&mut self, id: PageId, buf: &PageBuf) -> Result<()> {
        self.writes += 1;
        self.file.write_at(id as u64 * PAGE_SIZE as u64, buf)?;
        Ok(())
    }

    /// fsync the database file.
    pub fn sync(&mut self) -> Result<()> {
        self.file.sync()?;
        Ok(())
    }

    /// Take the inter-process lock on the database file.
    pub fn lock(&mut self) -> Result<()> {
        if self.file.try_lock()? {
            Ok(())
        } else {
            Err(crate::Error::Busy(
                "database file is locked by another process".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::file::{MemVfs, Vfs};
    use std::path::Path;

    #[test]
    fn round_trip_pages() {
        let vfs = MemVfs::new();
        let mut p = Pager::new(vfs.open(Path::new("db")).unwrap()).unwrap();
        assert_eq!(p.num_pages().unwrap(), 0);
        let mut a = new_page_buf();
        a[0] = 7;
        a[PAGE_SIZE - 1] = 9;
        p.write_page(3, &a).unwrap();
        assert_eq!(p.num_pages().unwrap(), 4);
        let mut b = new_page_buf();
        p.read_page(3, &mut b).unwrap();
        assert_eq!(a, b);
        // Unwritten pages inside the file and past its end read as zero.
        p.read_page(1, &mut b).unwrap();
        assert!(b.iter().all(|&x| x == 0));
        p.read_page(100, &mut b).unwrap();
        assert!(b.iter().all(|&x| x == 0));
    }
}
