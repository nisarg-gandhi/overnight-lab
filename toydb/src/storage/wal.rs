//! The write-ahead log (WAL): a redo log of full page images.
//!
//! Commit protocol (see [`super::Storage::commit`]):
//!
//! 1. Append one PAGE record per modified page, then a COMMIT record.
//! 2. fsync the WAL. **The transaction is committed once this returns.**
//! 3. Write the pages into the database file (not synced).
//!
//! A crash at any point is repaired on the next open by [`Wal::recover`]:
//! it replays the PAGE records of every transaction whose COMMIT record made
//! it to disk intact, and ignores everything after the first damaged or
//! incomplete record. Because records hold *whole pages* (physical logging),
//! replay is idempotent: applying a page image twice is the same as once.
//!
//! ```text
//! WAL file
//! +----------------------+
//! | header (24 bytes)    |  magic "TOYDBWAL", version, page size, salt, crc
//! +----------------------+
//! | PAGE   txn=7 page=0  |  \
//! | PAGE   txn=7 page=12 |   } transaction 7
//! | COMMIT txn=7 n=2     |  /
//! | PAGE   txn=8 page=0  |  \
//! | PAGE   txn=8 page=3  |   } transaction 8: no COMMIT => ignored
//! | (torn bytes...)      |  /
//! +----------------------+
//!
//! Record
//! +------+--------+---------+---------+------------------+--------+
//! | type | txn id | page id | length  | payload          | crc32  |
//! | u8   | u64    | u32     | u32     | `length` bytes   | u32    |
//! +------+--------+---------+---------+------------------+--------+
//!   type 1 = PAGE   payload = 4096-byte page image
//!   type 2 = COMMIT payload empty, page id field = number of PAGE records
//!   crc32 covers salt + every preceding byte of the record
//! ```
//!
//! The *salt* is a random number chosen each time the log is reset. Every
//! record's checksum includes it, so a stale record left behind by an
//! earlier generation of the log can never be mistaken for a current one.
//!
//! **Checkpointing** ([`Wal::reset`], called by `Storage::checkpoint` after
//! fsyncing the database file) truncates the log. Without it the WAL would
//! grow forever and recovery would get slower and slower.

use std::time::{SystemTime, UNIX_EPOCH};

use super::file::File;
use super::pager::{new_page_buf, PageBuf, PageId, Pager, PAGE_SIZE};
use crate::error::Result;
use crate::util::bytes::{get_u32, get_u64, put_u32, put_u64};
use crate::util::crc32;

const MAGIC: &[u8; 8] = b"TOYDBWAL";
const VERSION: u32 = 1;
const HEADER_SIZE: u64 = 24;
const REC_HEADER: usize = 1 + 8 + 4 + 4;
const REC_PAGE: u8 = 1;
const REC_COMMIT: u8 = 2;

pub struct Wal {
    file: Box<dyn File>,
    /// Append position == current log size.
    size: u64,
    salt: u32,
    fsync: bool,
    /// Reused buffer for building a transaction's records.
    buf: Vec<u8>,
}

/// What recovery found, for tests and logging.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Transactions replayed into the database file.
    pub committed_txns: u64,
    /// Page images written.
    pub pages_replayed: u64,
    /// Bytes at the end of the log that were ignored (incomplete or damaged
    /// records, or an uncommitted transaction).
    pub discarded_bytes: u64,
}

fn fresh_salt() -> u32 {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    (t ^ (t >> 32)) as u32 | 1
}

impl Wal {
    pub fn open(file: Box<dyn File>, fsync: bool) -> Result<Self> {
        Ok(Wal {
            file,
            size: 0,
            salt: fresh_salt(),
            fsync,
            buf: Vec::new(),
        })
    }

    /// Current size of the log in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Append one transaction: a PAGE record for each page, then COMMIT.
    /// Durable when this returns (if fsync is enabled).
    pub fn append_commit<'a>(
        &mut self,
        txn: u64,
        pages: impl Iterator<Item = (PageId, &'a PageBuf)>,
    ) -> Result<()> {
        let mut buf = std::mem::take(&mut self.buf);
        buf.clear();
        let mut n = 0u32;
        for (id, data) in pages {
            encode_record(&mut buf, self.salt, REC_PAGE, txn, id, data);
            n += 1;
        }
        encode_record(&mut buf, self.salt, REC_COMMIT, txn, n, &[]);
        // One write call for the whole transaction keeps syscalls down.
        let r = self.file.write_at(self.size, &buf);
        let len = buf.len() as u64;
        self.buf = buf;
        r?;
        self.size += len;
        if self.fsync {
            self.file.sync()?;
        }
        Ok(())
    }

    /// Truncate the log and start a new generation with a new salt. Only
    /// safe once every logged page is durable in the database file.
    pub fn reset(&mut self) -> Result<()> {
        self.salt = self.salt.wrapping_add(fresh_salt()) | 1;
        let mut h = [0u8; HEADER_SIZE as usize];
        h[..8].copy_from_slice(MAGIC);
        put_u32(&mut h, 8, VERSION);
        put_u32(&mut h, 12, PAGE_SIZE as u32);
        put_u32(&mut h, 16, self.salt);
        let crc = crc32::checksum(&h[..20]);
        put_u32(&mut h, 20, crc);
        self.file.set_len(0)?;
        self.file.write_at(0, &h)?;
        // Always sync here, even with fsync disabled: a checkpoint is rare,
        // and the truncation must be durable before new records follow.
        self.file.sync()?;
        self.size = HEADER_SIZE;
        Ok(())
    }

    /// Replay committed transactions into the database file, sync it, and
    /// reset the log. Called once when the database is opened.
    pub fn recover(&mut self, pager: &mut Pager) -> Result<RecoveryReport> {
        let mut report = RecoveryReport::default();
        let file_len = self.file.size()?;
        let mut h = [0u8; HEADER_SIZE as usize];
        let header_ok = file_len >= HEADER_SIZE
            && self.file.read_at(0, &mut h)? == h.len()
            && &h[..8] == MAGIC
            && get_u32(&h, 8) == VERSION
            && get_u32(&h, 12) == PAGE_SIZE as u32
            && crc32::checksum(&h[..20]) == get_u32(&h, 20);
        if header_ok {
            self.salt = get_u32(&h, 16);
            let mut off = HEADER_SIZE;
            let mut committed_end = HEADER_SIZE;
            // Page images of the transaction currently being read; applied
            // only when its COMMIT record turns up.
            let mut pending: Vec<(PageId, Box<PageBuf>)> = Vec::new();
            let mut pending_txn = None;
            while let Some((kind, txn, id, payload, next)) = self.read_record(off, file_len)? {
                if pending_txn.is_some_and(|t| t != txn) {
                    pending.clear(); // can't happen with a single writer, but be safe
                }
                pending_txn = Some(txn);
                match kind {
                    REC_PAGE => {
                        let mut page = new_page_buf();
                        page.copy_from_slice(&payload);
                        pending.push((id, page));
                    }
                    REC_COMMIT => {
                        if id as usize == pending.len() {
                            for (pid, page) in pending.drain(..) {
                                pager.write_page(pid, &page)?;
                                report.pages_replayed += 1;
                            }
                            report.committed_txns += 1;
                            committed_end = next;
                        } else {
                            break; // page count mismatch: treat as damage
                        }
                        pending_txn = None;
                    }
                    _ => unreachable!("read_record validates the type"),
                }
                off = next;
            }
            report.discarded_bytes = file_len - committed_end;
            if report.pages_replayed > 0 {
                pager.sync()?;
            }
        }
        self.reset()?;
        Ok(report)
    }

    /// Read and validate the record at `off`. Returns `None` at the end of
    /// the valid log (EOF, truncated record, bad type, or bad checksum).
    #[allow(clippy::type_complexity)]
    fn read_record(
        &mut self,
        off: u64,
        file_len: u64,
    ) -> Result<Option<(u8, u64, u32, Vec<u8>, u64)>> {
        if off + REC_HEADER as u64 + 4 > file_len {
            return Ok(None);
        }
        let mut hdr = [0u8; REC_HEADER];
        self.file.read_at(off, &mut hdr)?;
        let kind = hdr[0];
        let txn = get_u64(&hdr, 1);
        let id = get_u32(&hdr, 9);
        let len = get_u32(&hdr, 13) as usize;
        let valid_len = match kind {
            REC_PAGE => len == PAGE_SIZE,
            REC_COMMIT => len == 0,
            _ => false,
        };
        let end = off + (REC_HEADER + len + 4) as u64;
        if !valid_len || end > file_len {
            return Ok(None);
        }
        let mut rest = vec![0u8; len + 4];
        self.file.read_at(off + REC_HEADER as u64, &mut rest)?;
        let mut h = crc32::Hasher::new();
        h.update(&self.salt.to_le_bytes());
        h.update(&hdr);
        h.update(&rest[..len]);
        if h.finish() != get_u32(&rest, len) {
            return Ok(None);
        }
        rest.truncate(len);
        Ok(Some((kind, txn, id, rest, end)))
    }
}

fn encode_record(buf: &mut Vec<u8>, salt: u32, kind: u8, txn: u64, id: u32, payload: &[u8]) {
    let start = buf.len();
    let mut hdr = [0u8; REC_HEADER];
    hdr[0] = kind;
    put_u64(&mut hdr, 1, txn);
    put_u32(&mut hdr, 9, id);
    put_u32(&mut hdr, 13, payload.len() as u32);
    buf.extend_from_slice(&hdr);
    buf.extend_from_slice(payload);
    let mut h = crc32::Hasher::new();
    h.update(&salt.to_le_bytes());
    h.update(&buf[start..]);
    buf.extend_from_slice(&h.finish().to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::file::{MemVfs, Vfs};
    use std::path::Path;

    fn page(b: u8) -> Box<PageBuf> {
        let mut p = new_page_buf();
        p.fill(b);
        p
    }

    fn setup(vfs: &MemVfs) -> (Wal, Pager) {
        let wal = Wal::open(vfs.open(Path::new("w")).unwrap(), true).unwrap();
        let pager = Pager::new(vfs.open(Path::new("d")).unwrap()).unwrap();
        (wal, pager)
    }

    #[test]
    fn replays_committed_transactions_only() {
        let vfs = MemVfs::new();
        let (mut wal, mut pager) = setup(&vfs);
        wal.recover(&mut pager).unwrap(); // initialise an empty log
        let (a, b) = (page(1), page(2));
        wal.append_commit(1, [(0, &*a), (1, &*a)].into_iter()).unwrap();
        wal.append_commit(2, [(1, &*b)].into_iter()).unwrap();
        // A third transaction whose COMMIT record is cut off.
        let mut buf = vec![];
        encode_record(&mut buf, wal.salt, REC_PAGE, 3, 0, &*page(3));
        let size = wal.size;
        wal.file.write_at(size, &buf).unwrap();
        drop(wal);

        let (mut wal, mut pager) = setup(&vfs);
        let report = wal.recover(&mut pager).unwrap();
        assert_eq!(report.committed_txns, 2);
        assert_eq!(report.pages_replayed, 3);
        assert!(report.discarded_bytes > 0);
        let mut out = new_page_buf();
        pager.read_page(0, &mut out).unwrap();
        assert_eq!(out[0], 1);
        pager.read_page(1, &mut out).unwrap();
        assert_eq!(out[0], 2);
        // The log was reset after recovery.
        assert_eq!(wal.size(), HEADER_SIZE);
    }

    #[test]
    fn corrupted_record_stops_replay() {
        let vfs = MemVfs::new();
        let (mut wal, mut pager) = setup(&vfs);
        wal.recover(&mut pager).unwrap();
        wal.append_commit(1, [(0, &*page(1))].into_iter()).unwrap();
        let second = wal.size;
        wal.append_commit(2, [(0, &*page(2))].into_iter()).unwrap();
        // Flip one byte in the middle of the second transaction's page.
        wal.file.write_at(second + 100, &[0xAA]).unwrap();
        drop(wal);
        let (mut wal, mut pager) = setup(&vfs);
        let report = wal.recover(&mut pager).unwrap();
        assert_eq!(report.committed_txns, 1);
        let mut out = new_page_buf();
        pager.read_page(0, &mut out).unwrap();
        assert_eq!(out[0], 1);
    }

    #[test]
    fn stale_records_from_old_generation_are_ignored() {
        let vfs = MemVfs::new();
        let (mut wal, mut pager) = setup(&vfs);
        wal.recover(&mut pager).unwrap();
        // Records written under an old salt...
        let mut old = vec![];
        encode_record(&mut old, wal.salt.wrapping_add(1), REC_PAGE, 1, 0, &*page(9));
        encode_record(&mut old, wal.salt.wrapping_add(1), REC_COMMIT, 1, 1, &[]);
        let size = wal.size;
        wal.file.write_at(size, &old).unwrap();
        drop(wal);
        let (mut wal, mut pager) = setup(&vfs);
        assert_eq!(wal.recover(&mut pager).unwrap().committed_txns, 0);
    }
}
