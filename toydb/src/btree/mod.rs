//! A B+tree mapping byte-string keys to byte-string values.
//!
//! ```text
//!                       root (page 3, never moves)
//!                      [  "m"  ]
//!                     /         \
//!            [ "d" | "h" ]      [ "t" ]               internal nodes
//!            /    |     \        /    \
//!         [a,c]->[d,f]->[h,k]->[m,p]->[t,z]           leaves, linked
//! ```
//!
//! * All values live in leaves; internal nodes only route searches.
//! * Leaves are chained left-to-right so range scans never revisit parents.
//! * The root page id is fixed for the lifetime of the tree (the catalog
//!   stores it): when the root splits, its contents move to a new page and
//!   the root becomes the new parent; when it shrinks to one child, that
//!   child is pulled up into the root page.
//! * Deletes merge a node into its sibling when it drops below a quarter
//!   full *and* the two fit in one page. There is no redistribution, so
//!   nodes can stay under-full; lookups remain correct, only space is lost.
//! * Values too big to sit in a leaf go to a chain of overflow pages.
//!
//! The algorithms are recursive and decode each node on the way down (see
//! `node.rs`). Trees are only a few levels deep, so recursion is fine.

pub mod node;

use std::ops::Bound;

use crate::error::{corrupt, Error, Result};
use crate::storage::{PageId, PageStore, PAGE_SIZE};
use crate::util::bytes::{get_u16, get_u32, put_u16, put_u32};
use node::{Cell, Node, MAX_KEY_SIZE, MAX_LEAF_ENTRY, NODE_HEADER};

/// A node smaller than this (in bytes) after a delete is a merge candidate.
const UNDERFLOW: usize = PAGE_SIZE / 4;

/// Overflow page layout: `[next u32][used u16][data ...]`.
const OVERFLOW_HEADER: usize = 6;
const OVERFLOW_CAPACITY: usize = PAGE_SIZE - OVERFLOW_HEADER;

/// Handle to a B+tree: just its root page id. All operations borrow the
/// page store explicitly, so many trees can share one store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BTree {
    root: PageId,
}

fn load(s: &mut dyn PageStore, id: PageId) -> Result<Node> {
    Node::decode(s.read(id)?)
}

fn store(s: &mut dyn PageStore, id: PageId, node: &Node) -> Result<()> {
    node.encode(s.write(id)?);
    Ok(())
}

/// Index of the child to descend into for `key`: the number of separator
/// keys `<= key`.
fn child_index(keys: &[Vec<u8>], key: &[u8]) -> usize {
    keys.partition_point(|k| k.as_slice() <= key)
}

/// Result of inserting into a subtree: did we replace an existing key, and
/// did the subtree's root split (separator + new right sibling)?
type InsertResult = (bool, Option<(Vec<u8>, PageId)>);

impl BTree {
    /// Create an empty tree (a single empty leaf).
    pub fn create(s: &mut dyn PageStore) -> Result<BTree> {
        let root = s.allocate()?;
        store(s, root, &Node::empty_leaf())?;
        Ok(BTree { root })
    }

    /// Open an existing tree by root page id.
    pub fn open(root: PageId) -> BTree {
        BTree { root }
    }

    pub fn root(&self) -> PageId {
        self.root
    }

    // --- lookup ------------------------------------------------------------

    /// Point lookup.
    pub fn get(&self, s: &mut dyn PageStore, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let mut id = self.root;
        loop {
            match load(s, id)? {
                Node::Internal { keys, children } => id = children[child_index(&keys, key)],
                Node::Leaf { keys, vals, .. } => {
                    return match keys.binary_search_by(|k| k.as_slice().cmp(key)) {
                        Ok(i) => Ok(Some(read_cell(s, &vals[i])?)),
                        Err(_) => Ok(None),
                    }
                }
            }
        }
    }

    /// Iterate over keys in `[start, end]` (bounds as given).
    pub fn range(
        &self,
        s: &mut dyn PageStore,
        start: Bound<&[u8]>,
        end: Bound<&[u8]>,
    ) -> Result<Cursor> {
        // Descend to the leaf that would contain the start key.
        let mut id = self.root;
        let leaf = loop {
            match load(s, id)? {
                Node::Internal { keys, children } => {
                    id = match start {
                        Bound::Unbounded => children[0],
                        Bound::Included(k) | Bound::Excluded(k) => {
                            children[child_index(&keys, k)]
                        }
                    }
                }
                leaf => break leaf,
            }
        };
        let Node::Leaf { keys, vals, next } = leaf else {
            unreachable!()
        };
        let pos = match start {
            Bound::Unbounded => 0,
            Bound::Included(k) => keys.partition_point(|x| x.as_slice() < k),
            Bound::Excluded(k) => keys.partition_point(|x| x.as_slice() <= k),
        };
        Ok(Cursor {
            keys,
            vals,
            pos,
            next,
            end: match end {
                Bound::Included(k) => Bound::Included(k.to_vec()),
                Bound::Excluded(k) => Bound::Excluded(k.to_vec()),
                Bound::Unbounded => Bound::Unbounded,
            },
            done: false,
        })
    }

    /// Iterate over the whole tree in key order.
    pub fn scan(&self, s: &mut dyn PageStore) -> Result<Cursor> {
        self.range(s, Bound::Unbounded, Bound::Unbounded)
    }

    /// The largest entry, if any.
    pub fn last(&self, s: &mut dyn PageStore) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        self.last_in(s, self.root)
    }

    fn last_in(&self, s: &mut dyn PageStore, id: PageId) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        match load(s, id)? {
            Node::Leaf { mut keys, mut vals, .. } => match (keys.pop(), vals.pop()) {
                (Some(k), Some(v)) => Ok(Some((k, read_cell(s, &v)?))),
                _ => Ok(None),
            },
            // Usually the rightmost child has it, but a rightmost leaf may be
            // empty (see the module docs on merging), so fall back leftwards.
            Node::Internal { children, .. } => {
                for &c in children.iter().rev() {
                    if let Some(e) = self.last_in(s, c)? {
                        return Ok(Some(e));
                    }
                }
                Ok(None)
            }
        }
    }

    // --- insert ------------------------------------------------------------

    /// Insert or replace. Returns `true` if the key already existed.
    pub fn insert(&self, s: &mut dyn PageStore, key: &[u8], value: &[u8]) -> Result<bool> {
        if key.len() > MAX_KEY_SIZE {
            return Err(Error::Limit(format!(
                "key of {} bytes exceeds maximum of {MAX_KEY_SIZE}",
                key.len()
            )));
        }
        if value.len() > u32::MAX as usize {
            return Err(Error::Limit("value larger than 4 GiB".into()));
        }
        let cell = if 4 + key.len() + value.len() <= MAX_LEAF_ENTRY {
            Cell::Inline(value.to_vec())
        } else {
            Cell::Overflow {
                len: value.len() as u32,
                first: write_overflow(s, value)?,
            }
        };
        let (replaced, split) = self.insert_rec(s, self.root, key, cell)?;
        if let Some((sep, right)) = split {
            // The root split. Move its (left-half) contents to a new page and
            // turn the root page into a parent of the two halves.
            let left_node = load(s, self.root)?;
            let left = s.allocate()?;
            store(s, left, &left_node)?;
            store(
                s,
                self.root,
                &Node::Internal {
                    keys: vec![sep],
                    children: vec![left, right],
                },
            )?;
        }
        Ok(replaced)
    }

    fn insert_rec(
        &self,
        s: &mut dyn PageStore,
        id: PageId,
        key: &[u8],
        cell: Cell,
    ) -> Result<InsertResult> {
        let mut node = load(s, id)?;
        let replaced = match &mut node {
            Node::Leaf { keys, vals, .. } => {
                match keys.binary_search_by(|k| k.as_slice().cmp(key)) {
                    Ok(i) => {
                        if let Cell::Overflow { first, .. } = vals[i] {
                            free_overflow(s, first)?;
                        }
                        vals[i] = cell;
                        true
                    }
                    Err(i) => {
                        keys.insert(i, key.to_vec());
                        vals.insert(i, cell);
                        false
                    }
                }
            }
            Node::Internal { keys, children } => {
                let i = child_index(keys, key);
                let (replaced, split) = self.insert_rec(s, children[i], key, cell)?;
                match split {
                    // Child absorbed the insert; this node is unchanged.
                    None => return Ok((replaced, None)),
                    Some((sep, right)) => {
                        keys.insert(i, sep);
                        children.insert(i + 1, right);
                    }
                }
                replaced
            }
        };

        if node.size() <= PAGE_SIZE {
            store(s, id, &node)?;
            return Ok((replaced, None));
        }
        // Overfull: split, keeping the left half in this page.
        let (sep, mut right) = node.split();
        let right_id = s.allocate()?;
        if let (Node::Leaf { next, .. }, Node::Leaf { next: rnext, .. }) = (&mut node, &mut right) {
            *rnext = *next;
            *next = right_id;
        }
        store(s, id, &node)?;
        store(s, right_id, &right)?;
        Ok((replaced, Some((sep, right_id))))
    }

    // --- delete ------------------------------------------------------------

    /// Delete `key`. Returns `true` if it was present.
    pub fn delete(&self, s: &mut dyn PageStore, key: &[u8]) -> Result<bool> {
        let found = self.delete_rec(s, self.root, key)?;
        // Collapse a root that has a single child: pull the child up.
        loop {
            match load(s, self.root)? {
                Node::Internal { keys, children } if keys.is_empty() => {
                    let child = load(s, children[0])?;
                    store(s, self.root, &child)?;
                    s.free(children[0])?;
                }
                _ => break,
            }
        }
        Ok(found)
    }

    fn delete_rec(&self, s: &mut dyn PageStore, id: PageId, key: &[u8]) -> Result<bool> {
        let mut node = load(s, id)?;
        if let Node::Leaf { keys, vals, .. } = &mut node {
            let Ok(i) = keys.binary_search_by(|k| k.as_slice().cmp(key)) else {
                return Ok(false);
            };
            keys.remove(i);
            if let Cell::Overflow { first, .. } = vals.remove(i) {
                free_overflow(s, first)?;
            }
            store(s, id, &node)?;
            return Ok(true);
        }
        let Node::Internal { keys, children } = &node else { unreachable!() };
        let i = child_index(keys, key);
        if !self.delete_rec(s, children[i], key)? {
            return Ok(false);
        }
        if self.maybe_merge(s, &mut node, i)? {
            store(s, id, &node)?;
        }
        Ok(true)
    }

    /// After a delete below `parent.children[i]`, merge that child with a
    /// sibling if it is under-full and the pair fits in one page. Returns
    /// whether `parent` changed.
    fn maybe_merge(&self, s: &mut dyn PageStore, parent: &mut Node, i: usize) -> Result<bool> {
        let Node::Internal { keys, children } = parent else { unreachable!() };
        let child = load(s, children[i])?;
        if child.size() >= UNDERFLOW || children.len() < 2 {
            return Ok(false);
        }
        // Prefer merging with the right sibling; the rightmost child merges
        // into its left sibling instead. `li` is the left of the pair.
        let li = if i + 1 < children.len() { i } else { i - 1 };
        let (mut left, right) = if li == i {
            (child, load(s, children[i + 1])?)
        } else {
            (load(s, children[li])?, child)
        };
        let sep = &keys[li];
        let merged_size = left.size() + right.size() - NODE_HEADER
            + if left.is_leaf() { 0 } else { node::internal_entry_size(sep) };
        if merged_size > PAGE_SIZE {
            return Ok(false);
        }
        match (&mut left, right) {
            (
                Node::Leaf { keys: lk, vals: lv, next: ln },
                Node::Leaf { keys: rk, vals: rv, next: rn },
            ) => {
                lk.extend(rk);
                lv.extend(rv);
                *ln = rn;
            }
            (
                Node::Internal { keys: lk, children: lc },
                Node::Internal { keys: rk, children: rc },
            ) => {
                // The separator comes back down between the two halves.
                lk.push(sep.clone());
                lk.extend(rk);
                lc.extend(rc);
            }
            _ => return Err(corrupt!("siblings at different B+tree levels")),
        }
        store(s, children[li], &left)?;
        s.free(children[li + 1])?;
        keys.remove(li);
        children.remove(li + 1);
        Ok(true)
    }

    // --- whole-tree operations -----------------------------------------------

    /// Free every page of the tree, including the root. The handle must not
    /// be used afterwards.
    pub fn destroy(self, s: &mut dyn PageStore) -> Result<()> {
        fn rec(s: &mut dyn PageStore, id: PageId) -> Result<()> {
            match load(s, id)? {
                Node::Leaf { vals, .. } => {
                    for v in vals {
                        if let Cell::Overflow { first, .. } = v {
                            free_overflow(s, first)?;
                        }
                    }
                }
                Node::Internal { children, .. } => {
                    for c in children {
                        rec(s, c)?;
                    }
                }
            }
            s.free(id)
        }
        rec(s, self.root)
    }

    /// Verify structural invariants; used heavily by the tests.
    ///
    /// Checks that keys are sorted, every key lies within the bounds implied
    /// by its ancestors' separators, all leaves are at the same depth, the
    /// leaf chain visits leaves in order, and every node fits in a page.
    /// Returns statistics about the tree.
    pub fn check(&self, s: &mut dyn PageStore) -> Result<TreeStats> {
        let mut st = TreeStats::default();
        let mut leaves = vec![];
        self.check_rec(s, self.root, None, None, 1, &mut st, &mut leaves)?;
        // Leaf chain must equal in-order leaf sequence.
        for w in leaves.windows(2) {
            let Node::Leaf { next, .. } = load(s, w[0])? else { unreachable!() };
            if next != w[1] {
                return Err(corrupt!("leaf chain broken at page {}", w[0]));
            }
        }
        if let Some(&last) = leaves.last() {
            let Node::Leaf { next, .. } = load(s, last)? else { unreachable!() };
            if next != 0 {
                return Err(corrupt!("last leaf has a next pointer"));
            }
        }
        Ok(st)
    }

    #[allow(clippy::too_many_arguments)]
    fn check_rec(
        &self,
        s: &mut dyn PageStore,
        id: PageId,
        lo: Option<&[u8]>,
        hi: Option<&[u8]>,
        depth: usize,
        st: &mut TreeStats,
        leaves: &mut Vec<PageId>,
    ) -> Result<()> {
        let node = load(s, id)?;
        st.pages += 1;
        let keys = node.keys();
        if keys.windows(2).any(|w| w[0] >= w[1]) {
            return Err(corrupt!("keys out of order in page {id}"));
        }
        for k in keys {
            if lo.is_some_and(|lo| k.as_slice() < lo) || hi.is_some_and(|hi| k.as_slice() >= hi) {
                return Err(corrupt!("key outside separator bounds in page {id}"));
            }
        }
        match &node {
            Node::Leaf { vals, .. } => {
                if st.depth == 0 {
                    st.depth = depth;
                } else if st.depth != depth {
                    return Err(corrupt!("leaves at different depths"));
                }
                st.entries += keys.len();
                st.leaves += 1;
                leaves.push(id);
                for v in vals {
                    if let Cell::Overflow { first, len } = v {
                        st.overflow_pages += read_overflow(s, *first, *len)?.1;
                    }
                }
            }
            Node::Internal { keys, children } => {
                if children.len() != keys.len() + 1 {
                    return Err(corrupt!("internal node {id} has wrong child count"));
                }
                for (i, &c) in children.iter().enumerate() {
                    let clo = if i == 0 { lo } else { Some(keys[i - 1].as_slice()) };
                    let chi = if i == keys.len() { hi } else { Some(keys[i].as_slice()) };
                    self.check_rec(s, c, clo, chi, depth + 1, st, leaves)?;
                }
            }
        }
        Ok(())
    }
}

/// Shape statistics returned by [`BTree::check`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TreeStats {
    /// Number of key/value pairs.
    pub entries: usize,
    /// Height of the tree (1 = root is a leaf).
    pub depth: usize,
    /// Node pages (leaves + internal).
    pub pages: usize,
    pub leaves: usize,
    pub overflow_pages: usize,
}

// --- overflow pages ----------------------------------------------------------

fn write_overflow(s: &mut dyn PageStore, data: &[u8]) -> Result<PageId> {
    let chunks: Vec<&[u8]> = data.chunks(OVERFLOW_CAPACITY).collect();
    let ids = (0..chunks.len())
        .map(|_| s.allocate())
        .collect::<Result<Vec<_>>>()?;
    for (i, chunk) in chunks.iter().enumerate() {
        let next = ids.get(i + 1).copied().unwrap_or(0);
        let p = s.write(ids[i])?;
        put_u32(p, 0, next);
        put_u16(p, 4, chunk.len() as u16);
        p[OVERFLOW_HEADER..OVERFLOW_HEADER + chunk.len()].copy_from_slice(chunk);
    }
    Ok(ids[0])
}

/// Returns the value and the number of pages it occupied.
fn read_overflow(s: &mut dyn PageStore, first: PageId, len: u32) -> Result<(Vec<u8>, usize)> {
    let mut out = Vec::with_capacity(len as usize);
    let mut id = first;
    let mut pages = 0;
    while id != 0 && out.len() < len as usize {
        let p = s.read(id)?;
        let used = get_u16(p, 4) as usize;
        if used > OVERFLOW_CAPACITY {
            return Err(corrupt!("bad overflow page {id}"));
        }
        out.extend_from_slice(&p[OVERFLOW_HEADER..OVERFLOW_HEADER + used]);
        id = get_u32(p, 0);
        pages += 1;
    }
    if out.len() != len as usize {
        return Err(corrupt!("overflow chain length mismatch"));
    }
    Ok((out, pages))
}

fn free_overflow(s: &mut dyn PageStore, first: PageId) -> Result<()> {
    let mut id = first;
    while id != 0 {
        let next = get_u32(s.read(id)?, 0);
        s.free(id)?;
        id = next;
    }
    Ok(())
}

fn read_cell(s: &mut dyn PageStore, cell: &Cell) -> Result<Vec<u8>> {
    match cell {
        Cell::Inline(v) => Ok(v.clone()),
        Cell::Overflow { len, first } => Ok(read_overflow(s, *first, *len)?.0),
    }
}

// --- cursors -------------------------------------------------------------------

/// A forward iterator over a key range.
///
/// The cursor holds a decoded copy of the current leaf and follows the leaf
/// chain. It doesn't implement `Iterator` because every step needs the page
/// store, which the caller passes in. If the tree is modified while a cursor
/// is open, the cursor may observe a stale leaf; the executor therefore never
/// mutates a tree it is scanning (UPDATE/DELETE collect keys first).
#[derive(Debug)]
pub struct Cursor {
    keys: Vec<Vec<u8>>,
    vals: Vec<Cell>,
    pos: usize,
    next: PageId,
    end: Bound<Vec<u8>>,
    done: bool,
}

impl Cursor {
    /// Advance and return the next entry, or `None` at the end of the range.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self, s: &mut dyn PageStore) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
        while !self.done {
            if self.pos < self.keys.len() {
                let i = self.pos;
                self.pos += 1;
                let past_end = match &self.end {
                    Bound::Unbounded => false,
                    Bound::Included(e) => self.keys[i] > *e,
                    Bound::Excluded(e) => self.keys[i] >= *e,
                };
                if past_end {
                    self.done = true;
                    break;
                }
                let v = read_cell(s, &self.vals[i])?;
                return Ok(Some((std::mem::take(&mut self.keys[i]), v)));
            }
            if self.next == 0 {
                self.done = true;
                break;
            }
            match load(s, self.next)? {
                Node::Leaf { keys, vals, next } => {
                    self.keys = keys;
                    self.vals = vals;
                    self.next = next;
                    self.pos = 0;
                }
                Node::Internal { .. } => return Err(corrupt!("leaf chain points at internal node")),
            }
        }
        Ok(None)
    }

    /// Drain the rest of the cursor into a vector (convenience for tests).
    pub fn collect(mut self, s: &mut dyn PageStore) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut out = vec![];
        while let Some(e) = self.next(s)? {
            out.push(e);
        }
        Ok(out)
    }
}

#[cfg(test)]
pub(crate) mod tests;
