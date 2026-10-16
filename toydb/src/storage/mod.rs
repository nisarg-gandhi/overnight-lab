//! The storage engine: pages, caching, allocation, durability.

pub mod buffer;
pub mod file;
pub mod pager;

pub use pager::{PageBuf, PageId, PAGE_SIZE};

use crate::error::Result;

/// What the B+tree needs from the layer below it.
///
/// Implemented by `Storage`; tests can implement it on a toy in-memory
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
