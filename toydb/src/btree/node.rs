//! On-page format of B+tree nodes.
//!
//! Nodes are *decoded* from a page into an owned [`Node`] struct, modified
//! with ordinary `Vec` operations, and *encoded* back. This costs a copy per
//! node visit but keeps the tree algorithms readable; production engines
//! instead edit slotted pages in place.
//!
//! ```text
//! Common header (7 bytes)
//! +--------+-----------+------------------------------+
//! | kind   | num_keys  | link                         |
//! | u8     | u16       | u32                          |
//! +--------+-----------+------------------------------+
//!   kind 1 = leaf      link = next leaf (0 = none)
//!   kind 2 = internal  link = leftmost child
//!
//! Leaf entries, packed after the header, sorted by key:
//! +---------+---------+---------+-----------+
//! | key_len | val_len | key ... | value ... |      val_len bit 15 set =>
//! | u16     | u16     |         |           |      value is an overflow
//! +---------+---------+---------+-----------+      pointer (8 bytes):
//!                                                  [total_len u32][first u32]
//! Internal entries, packed after the header:
//! +---------+---------+-------------+
//! | key_len | key ... | right child |     child[i+1] holds keys >= key[i]
//! | u16     |         | u32         |     link (child[0]) holds keys < key[0]
//! +---------+---------+-------------+
//! ```

use crate::error::{corrupt, Result};
use crate::storage::{PageBuf, PageId, PAGE_SIZE};
use crate::util::bytes::{get_u16, get_u32, put_u16, put_u32};

pub const NODE_HEADER: usize = 7;
const KIND_LEAF: u8 = 1;
const KIND_INTERNAL: u8 = 2;
const OVERFLOW_FLAG: u16 = 0x8000;

/// Largest key we accept. Guarantees an internal node holds at least seven
/// keys, so splits always produce two non-empty halves.
pub const MAX_KEY_SIZE: usize = 512;

/// Largest leaf entry (header + key + inline value) we store inline. Four
/// such entries fit in a page, so splitting an overfull leaf at its byte
/// midpoint always yields two halves that fit. Bigger values go to overflow
/// pages.
pub const MAX_LEAF_ENTRY: usize = (PAGE_SIZE - NODE_HEADER) / 4;

/// A value as stored in a leaf: inline bytes, or a pointer to a chain of
/// overflow pages holding `len` bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cell {
    Inline(Vec<u8>),
    Overflow { len: u32, first: PageId },
}

impl Cell {
    fn encoded_len(&self) -> usize {
        match self {
            Cell::Inline(v) => v.len(),
            Cell::Overflow { .. } => 8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Leaf {
        keys: Vec<Vec<u8>>,
        vals: Vec<Cell>,
        /// Next leaf in key order, 0 if this is the last one.
        next: PageId,
    },
    Internal {
        /// `keys.len() + 1 == children.len()`.
        keys: Vec<Vec<u8>>,
        children: Vec<PageId>,
    },
}

/// Size in bytes of one leaf entry.
pub fn leaf_entry_size(key: &[u8], val: &Cell) -> usize {
    4 + key.len() + val.encoded_len()
}

/// Size in bytes of one internal entry.
pub fn internal_entry_size(key: &[u8]) -> usize {
    2 + key.len() + 4
}

impl Node {
    pub fn empty_leaf() -> Node {
        Node::Leaf {
            keys: vec![],
            vals: vec![],
            next: 0,
        }
    }

    pub fn is_leaf(&self) -> bool {
        matches!(self, Node::Leaf { .. })
    }

    pub fn keys(&self) -> &[Vec<u8>] {
        match self {
            Node::Leaf { keys, .. } | Node::Internal { keys, .. } => keys,
        }
    }

    /// Encoded size in bytes. A node fits in a page iff this is
    /// `<= PAGE_SIZE`.
    pub fn size(&self) -> usize {
        NODE_HEADER
            + match self {
                Node::Leaf { keys, vals, .. } => keys
                    .iter()
                    .zip(vals)
                    .map(|(k, v)| leaf_entry_size(k, v))
                    .sum::<usize>(),
                Node::Internal { keys, .. } => keys.iter().map(|k| internal_entry_size(k)).sum(),
            }
    }

    /// Parse a node out of a page, validating as we go so that a corrupt page
    /// produces an error instead of a panic.
    pub fn decode(page: &PageBuf) -> Result<Node> {
        let kind = page[0];
        let n = get_u16(page, 1) as usize;
        let link = get_u32(page, 3);
        let mut off = NODE_HEADER;
        let mut keys = Vec::with_capacity(n);
        let need = |off: usize, len: usize| -> Result<()> {
            if off + len > PAGE_SIZE {
                Err(corrupt!("B+tree node entry runs past end of page"))
            } else {
                Ok(())
            }
        };
        match kind {
            KIND_LEAF => {
                let mut vals = Vec::with_capacity(n);
                for _ in 0..n {
                    need(off, 4)?;
                    let klen = get_u16(page, off) as usize;
                    let raw_vlen = get_u16(page, off + 2);
                    off += 4;
                    let overflow = raw_vlen & OVERFLOW_FLAG != 0;
                    let vlen = if overflow { 8 } else { raw_vlen as usize };
                    need(off, klen + vlen)?;
                    keys.push(page[off..off + klen].to_vec());
                    off += klen;
                    vals.push(if overflow {
                        Cell::Overflow {
                            len: get_u32(page, off),
                            first: get_u32(page, off + 4),
                        }
                    } else {
                        Cell::Inline(page[off..off + vlen].to_vec())
                    });
                    off += vlen;
                }
                Ok(Node::Leaf {
                    keys,
                    vals,
                    next: link,
                })
            }
            KIND_INTERNAL => {
                let mut children = Vec::with_capacity(n + 1);
                children.push(link);
                for _ in 0..n {
                    need(off, 2)?;
                    let klen = get_u16(page, off) as usize;
                    off += 2;
                    need(off, klen + 4)?;
                    keys.push(page[off..off + klen].to_vec());
                    off += klen;
                    children.push(get_u32(page, off));
                    off += 4;
                }
                Ok(Node::Internal { keys, children })
            }
            k => Err(corrupt!("invalid B+tree node kind {k}")),
        }
    }

    /// Serialize into `page`. Panics if the node doesn't fit: callers must
    /// split first (that's a logic error, not a runtime condition).
    pub fn encode(&self, page: &mut PageBuf) {
        assert!(self.size() <= PAGE_SIZE, "node too large to encode");
        page.fill(0);
        let mut off = NODE_HEADER;
        match self {
            Node::Leaf { keys, vals, next } => {
                page[0] = KIND_LEAF;
                put_u16(page, 1, keys.len() as u16);
                put_u32(page, 3, *next);
                for (k, v) in keys.iter().zip(vals) {
                    put_u16(page, off, k.len() as u16);
                    let vlen = match v {
                        Cell::Inline(b) => b.len() as u16,
                        Cell::Overflow { .. } => OVERFLOW_FLAG,
                    };
                    put_u16(page, off + 2, vlen);
                    off += 4;
                    page[off..off + k.len()].copy_from_slice(k);
                    off += k.len();
                    match v {
                        Cell::Inline(b) => {
                            page[off..off + b.len()].copy_from_slice(b);
                            off += b.len();
                        }
                        Cell::Overflow { len, first } => {
                            put_u32(page, off, *len);
                            put_u32(page, off + 4, *first);
                            off += 8;
                        }
                    }
                }
            }
            Node::Internal { keys, children } => {
                page[0] = KIND_INTERNAL;
                put_u16(page, 1, keys.len() as u16);
                put_u32(page, 3, children[0]);
                for (k, &c) in keys.iter().zip(&children[1..]) {
                    put_u16(page, off, k.len() as u16);
                    off += 2;
                    page[off..off + k.len()].copy_from_slice(k);
                    off += k.len();
                    put_u32(page, off, c);
                    off += 4;
                }
            }
        }
    }

    /// Split an overfull node in two at (roughly) its byte midpoint.
    ///
    /// `self` keeps the left half. Returns the separator key to insert into
    /// the parent and the new right node. For leaves the separator is a copy
    /// of the right node's first key; for internal nodes the middle key moves
    /// up and appears in neither half.
    pub fn split(&mut self) -> (Vec<u8>, Node) {
        let half = (self.size() - NODE_HEADER) / 2;
        match self {
            Node::Leaf { keys, vals, .. } => {
                let mut acc = 0;
                let mut at = keys.len() - 1;
                for (i, (k, v)) in keys.iter().zip(vals.iter()).enumerate() {
                    acc += leaf_entry_size(k, v);
                    if acc >= half {
                        at = i + 1;
                        break;
                    }
                }
                let at = at.clamp(1, keys.len() - 1);
                let rkeys = keys.split_off(at);
                let rvals = vals.split_off(at);
                let sep = rkeys[0].clone();
                (
                    sep,
                    Node::Leaf {
                        keys: rkeys,
                        vals: rvals,
                        next: 0, // fixed up by the caller, who knows page ids
                    },
                )
            }
            Node::Internal { keys, children } => {
                let mut acc = 0;
                let mut mid = keys.len() / 2;
                for (i, k) in keys.iter().enumerate() {
                    acc += internal_entry_size(k);
                    if acc >= half {
                        mid = i;
                        break;
                    }
                }
                let mid = mid.clamp(1, keys.len() - 2);
                let mut rkeys = keys.split_off(mid);
                let sep = rkeys.remove(0);
                let rchildren = children.split_off(mid + 1);
                (
                    sep,
                    Node::Internal {
                        keys: rkeys,
                        children: rchildren,
                    },
                )
            }
        }
    }
}

// --- allocation-free access to encoded pages ------------------------------------
//
// Decoding a node allocates a Vec per key. On the hot paths — descending
// through internal nodes and point lookups in leaves — we only need to
// *read* a page, so these helpers work on the encoded bytes directly.

/// Is this page a leaf (as opposed to an internal node)?
pub fn page_is_leaf(page: &PageBuf) -> Result<bool> {
    match page[0] {
        KIND_LEAF => Ok(true),
        KIND_INTERNAL => Ok(false),
        k => Err(corrupt!("invalid B+tree node kind {k}")),
    }
}

/// Number of keys in an encoded node.
pub fn page_num_keys(page: &PageBuf) -> usize {
    get_u16(page, 1) as usize
}

/// For an encoded internal node: the child to descend into for `key`
/// (the child after the last separator `<= key`). `None` key means the
/// leftmost child.
pub fn page_find_child(page: &PageBuf, key: Option<&[u8]>) -> Result<PageId> {
    let mut child = get_u32(page, 3);
    let Some(key) = key else { return Ok(child) };
    let mut off = NODE_HEADER;
    for _ in 0..page_num_keys(page) {
        if off + 2 > PAGE_SIZE {
            return Err(corrupt!("B+tree node entry runs past end of page"));
        }
        let klen = get_u16(page, off) as usize;
        let end = off + 2 + klen + 4;
        if end > PAGE_SIZE {
            return Err(corrupt!("B+tree node entry runs past end of page"));
        }
        if &page[off + 2..off + 2 + klen] > key {
            break;
        }
        child = get_u32(page, off + 2 + klen);
        off = end;
    }
    Ok(child)
}

/// Where a leaf entry's value is, within an encoded page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellPos {
    /// `page[start..start + len]`
    Inline { start: usize, len: usize },
    Overflow { len: u32, first: PageId },
}

/// Parse the leaf entry starting at byte `off`: returns the key's position
/// (`page[kstart..kstart + klen]`), the value's position, and the offset of
/// the next entry. Validates bounds so corrupt pages give errors.
pub fn page_leaf_entry(page: &PageBuf, off: usize) -> Result<(usize, usize, CellPos, usize)> {
    if off + 4 > PAGE_SIZE {
        return Err(corrupt!("B+tree node entry runs past end of page"));
    }
    let klen = get_u16(page, off) as usize;
    let raw_vlen = get_u16(page, off + 2);
    let overflow = raw_vlen & OVERFLOW_FLAG != 0;
    let vlen = if overflow { 8 } else { raw_vlen as usize };
    let kstart = off + 4;
    let vstart = kstart + klen;
    if vstart + vlen > PAGE_SIZE {
        return Err(corrupt!("B+tree node entry runs past end of page"));
    }
    let cell = if overflow {
        CellPos::Overflow {
            len: get_u32(page, vstart),
            first: get_u32(page, vstart + 4),
        }
    } else {
        CellPos::Inline { start: vstart, len: vlen }
    };
    Ok((kstart, klen, cell, vstart + vlen))
}

/// For an encoded leaf: the cell stored under `key`, if any.
pub fn page_leaf_lookup(page: &PageBuf, key: &[u8]) -> Result<Option<Cell>> {
    let mut off = NODE_HEADER;
    for _ in 0..page_num_keys(page) {
        let (kstart, klen, cell, next) = page_leaf_entry(page, off)?;
        match page[kstart..kstart + klen].cmp(key) {
            std::cmp::Ordering::Less => off = next,
            std::cmp::Ordering::Greater => return Ok(None), // keys are sorted
            std::cmp::Ordering::Equal => {
                return Ok(Some(match cell {
                    CellPos::Inline { start, len } => Cell::Inline(page[start..start + len].to_vec()),
                    CellPos::Overflow { len, first } => Cell::Overflow { len, first },
                }))
            }
        }
    }
    Ok(None)
}

/// Encoded size of a node without decoding it (see [`Node::size`]).
pub fn page_node_size(page: &PageBuf) -> Result<usize> {
    let leaf = page_is_leaf(page)?;
    let mut off = NODE_HEADER;
    for _ in 0..page_num_keys(page) {
        if off + 4 > PAGE_SIZE {
            return Err(corrupt!("B+tree node entry runs past end of page"));
        }
        let klen = get_u16(page, off) as usize;
        off += if leaf {
            let raw = get_u16(page, off + 2);
            4 + klen + if raw & OVERFLOW_FLAG != 0 { 8 } else { raw as usize }
        } else {
            2 + klen + 4
        };
    }
    Ok(off)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::pager::new_page_buf;

    #[test]
    fn leaf_round_trip() {
        let n = Node::Leaf {
            keys: vec![b"a".to_vec(), b"bb".to_vec(), vec![]],
            vals: vec![
                Cell::Inline(b"1".to_vec()),
                Cell::Overflow { len: 9000, first: 42 },
                Cell::Inline(vec![]),
            ],
            next: 77,
        };
        let mut p = new_page_buf();
        n.encode(&mut p);
        assert_eq!(Node::decode(&p).unwrap(), n);
    }

    #[test]
    fn internal_round_trip() {
        let n = Node::Internal {
            keys: vec![b"m".to_vec(), b"t".to_vec()],
            children: vec![3, 4, 5],
        };
        let mut p = new_page_buf();
        n.encode(&mut p);
        assert_eq!(Node::decode(&p).unwrap(), n);
        assert_eq!(n.size(), NODE_HEADER + 2 * 7);
    }

    #[test]
    fn corrupt_pages_are_errors_not_panics() {
        let mut p = new_page_buf();
        p[0] = 9;
        assert!(Node::decode(&p).is_err());
        p[0] = KIND_LEAF;
        put_u16(&mut p[..], 1, 1000); // claims far more entries than fit
        put_u16(&mut p[..], NODE_HEADER, 4000);
        assert!(Node::decode(&p).is_err());
    }

    #[test]
    fn in_page_helpers_agree_with_decode() {
        let leaf = Node::Leaf {
            keys: vec![b"a".to_vec(), b"c".to_vec(), b"e".to_vec()],
            vals: vec![
                Cell::Inline(b"1".to_vec()),
                Cell::Overflow { len: 5000, first: 9 },
                Cell::Inline(vec![]),
            ],
            next: 0,
        };
        let mut p = new_page_buf();
        leaf.encode(&mut p);
        assert!(page_is_leaf(&p).unwrap());
        assert_eq!(page_node_size(&p).unwrap(), leaf.size());
        assert_eq!(page_leaf_lookup(&p, b"a").unwrap(), Some(Cell::Inline(b"1".to_vec())));
        assert_eq!(page_leaf_lookup(&p, b"c").unwrap(), Some(Cell::Overflow { len: 5000, first: 9 }));
        assert_eq!(page_leaf_lookup(&p, b"b").unwrap(), None);
        assert_eq!(page_leaf_lookup(&p, b"z").unwrap(), None);

        let internal = Node::Internal {
            keys: vec![b"d".to_vec(), b"h".to_vec()],
            children: vec![10, 20, 30],
        };
        internal.encode(&mut p);
        assert!(!page_is_leaf(&p).unwrap());
        assert_eq!(page_node_size(&p).unwrap(), internal.size());
        for (key, child) in [(&b"a"[..], 10), (b"d", 20), (b"e", 20), (b"h", 30), (b"z", 30)] {
            assert_eq!(page_find_child(&p, Some(key)).unwrap(), child, "{key:?}");
        }
        assert_eq!(page_find_child(&p, None).unwrap(), 10);
    }

    #[test]
    fn split_halves_fit() {
        let mut keys = vec![];
        let mut vals = vec![];
        for i in 0..20u8 {
            keys.push(vec![i; 10]);
            vals.push(Cell::Inline(vec![i; 300]));
        }
        let mut left = Node::Leaf { keys, vals, next: 0 };
        assert!(left.size() > PAGE_SIZE);
        let (sep, right) = left.split();
        assert!(left.size() <= PAGE_SIZE && right.size() <= PAGE_SIZE);
        assert_eq!(&sep, &right.keys()[0]);
        assert!(left.keys().last().unwrap() < &sep);
    }
}
