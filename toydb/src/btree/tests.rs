//! B+tree tests: hand-written cases plus property tests against
//! `std::collections::BTreeMap` as the reference model.

use super::*;
use crate::storage::pager::new_page_buf;
use crate::storage::PageBuf;
use proptest::prelude::*;
use std::collections::{BTreeMap, HashSet};

/// A trivial in-memory page store that also tracks which pages are live,
/// so tests can detect leaked (allocated but unreachable) pages.
#[derive(Default)]
pub(crate) struct MemStore {
    pages: Vec<Box<PageBuf>>,
    free: Vec<PageId>,
    live: HashSet<PageId>,
}

impl MemStore {
    pub(crate) fn new() -> Self {
        // Page 0 is reserved (it is the header page in the real store), so
        // that 0 can mean "no page".
        let mut s = MemStore::default();
        s.pages.push(new_page_buf());
        s
    }
    fn live_pages(&self) -> usize {
        self.live.len()
    }
}

impl PageStore for MemStore {
    fn read(&mut self, id: PageId) -> Result<&PageBuf> {
        assert!(self.live.contains(&id), "read of dead page {id}");
        Ok(&self.pages[id as usize])
    }
    fn write(&mut self, id: PageId) -> Result<&mut PageBuf> {
        assert!(self.live.contains(&id), "write of dead page {id}");
        Ok(&mut self.pages[id as usize])
    }
    fn allocate(&mut self) -> Result<PageId> {
        let id = self.free.pop().unwrap_or_else(|| {
            self.pages.push(new_page_buf());
            (self.pages.len() - 1) as PageId
        });
        self.pages[id as usize].fill(0);
        self.live.insert(id);
        Ok(id)
    }
    fn free(&mut self, id: PageId) -> Result<()> {
        assert!(self.live.remove(&id), "double free of page {id}");
        self.free.push(id);
        Ok(())
    }
}

fn key(i: u32) -> Vec<u8> {
    format!("key{i:08}").into_bytes()
}

/// Check invariants and that no page has leaked.
fn check_all(t: &BTree, s: &mut MemStore) -> TreeStats {
    let st = t.check(s).unwrap();
    assert_eq!(
        s.live_pages(),
        st.pages + st.overflow_pages,
        "leaked pages: {st:?}"
    );
    st
}

#[test]
fn empty_tree() {
    let mut s = MemStore::new();
    let t = BTree::create(&mut s).unwrap();
    assert_eq!(t.get(&mut s, b"x").unwrap(), None);
    assert_eq!(t.last(&mut s).unwrap(), None);
    assert!(t.scan(&mut s).unwrap().collect(&mut s).unwrap().is_empty());
    assert!(!t.delete(&mut s, b"x").unwrap());
    assert_eq!(check_all(&t, &mut s).depth, 1);
}

#[test]
fn insert_get_replace_delete() {
    let mut s = MemStore::new();
    let t = BTree::create(&mut s).unwrap();
    assert!(!t.insert(&mut s, b"b", b"2").unwrap());
    assert!(!t.insert(&mut s, b"a", b"1").unwrap());
    assert!(!t.insert(&mut s, b"", b"empty key").unwrap());
    assert!(t.insert(&mut s, b"a", b"one").unwrap());
    assert_eq!(t.get(&mut s, b"a").unwrap().unwrap(), b"one");
    assert_eq!(t.get(&mut s, b"").unwrap().unwrap(), b"empty key");
    assert!(t.delete(&mut s, b"a").unwrap());
    assert!(!t.delete(&mut s, b"a").unwrap());
    assert_eq!(t.get(&mut s, b"a").unwrap(), None);
    assert_eq!(t.get(&mut s, b"b").unwrap().unwrap(), b"2");
}

#[test]
fn sequential_inserts_split_and_scan_in_order() {
    let mut s = MemStore::new();
    let t = BTree::create(&mut s).unwrap();
    let n = 20_000;
    for i in 0..n {
        t.insert(&mut s, &key(i), &i.to_le_bytes().repeat(32)).unwrap();
    }
    let st = check_all(&t, &mut s);
    assert_eq!(st.entries, n as usize);
    assert!(st.depth >= 3, "{st:?}");
    let all = t.scan(&mut s).unwrap().collect(&mut s).unwrap();
    assert_eq!(all.len(), n as usize);
    for (i, (k, v)) in all.iter().enumerate() {
        assert_eq!(k, &key(i as u32));
        assert_eq!(v, &(i as u32).to_le_bytes().repeat(32));
    }
    assert_eq!(t.last(&mut s).unwrap().unwrap().0, key(n - 1));
}

#[test]
fn reverse_and_interleaved_inserts() {
    let mut s = MemStore::new();
    let t = BTree::create(&mut s).unwrap();
    for i in (0..5000).rev() {
        t.insert(&mut s, &key(i), b"v").unwrap();
    }
    for i in 0..5000 {
        t.insert(&mut s, &key(i * 2 + 1_000_000), b"w").unwrap();
    }
    assert_eq!(check_all(&t, &mut s).entries, 10_000);
}

#[test]
fn delete_everything_shrinks_back_to_one_page() {
    let mut s = MemStore::new();
    let t = BTree::create(&mut s).unwrap();
    let n = 10_000;
    for i in 0..n {
        t.insert(&mut s, &key(i), &[7u8; 50]).unwrap();
    }
    assert!(check_all(&t, &mut s).depth >= 3);
    // Delete in a scrambled order to exercise left and right merges.
    let mut order: Vec<u32> = (0..n).collect();
    let mut rng = crate::util::Rng::new(1);
    for i in (1..order.len()).rev() {
        order.swap(i, rng.below(i as u64 + 1) as usize);
    }
    for (j, &i) in order.iter().enumerate() {
        assert!(t.delete(&mut s, &key(i)).unwrap());
        if j % 997 == 0 {
            check_all(&t, &mut s);
        }
    }
    let st = check_all(&t, &mut s);
    assert_eq!(st.entries, 0);
    assert_eq!(st.pages, 1, "tree should collapse to its root: {st:?}");
}

#[test]
fn range_bounds() {
    let mut s = MemStore::new();
    let t = BTree::create(&mut s).unwrap();
    for i in 0..1000 {
        t.insert(&mut s, &key(i * 2), b"").unwrap(); // even keys only
    }
    let keys = |c: Cursor, s: &mut MemStore| -> Vec<Vec<u8>> {
        c.collect(s).unwrap().into_iter().map(|(k, _)| k).collect()
    };
    let (k10, k11, k20) = (key(10), key(11), key(20));
    let r = t.range(&mut s, Bound::Included(&k10), Bound::Excluded(&k20)).unwrap();
    assert_eq!(keys(r, &mut s), vec![key(10), key(12), key(14), key(16), key(18)]);
    let r = t.range(&mut s, Bound::Excluded(&k10), Bound::Included(&k20)).unwrap();
    assert_eq!(keys(r, &mut s), vec![key(12), key(14), key(16), key(18), key(20)]);
    let r = t.range(&mut s, Bound::Included(&k11), Bound::Included(&k11)).unwrap();
    assert!(keys(r, &mut s).is_empty());
    let r = t.range(&mut s, Bound::Unbounded, Bound::Excluded(&k10)).unwrap();
    assert_eq!(keys(r, &mut s).len(), 5);
    let big = key(5000);
    let r = t.range(&mut s, Bound::Included(&big), Bound::Unbounded).unwrap();
    assert!(keys(r, &mut s).is_empty());
}

#[test]
fn overflow_values() {
    let mut s = MemStore::new();
    let t = BTree::create(&mut s).unwrap();
    let big: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    t.insert(&mut s, b"big", &big).unwrap();
    t.insert(&mut s, b"small", b"x").unwrap();
    assert_eq!(t.get(&mut s, b"big").unwrap().unwrap(), big);
    let st = check_all(&t, &mut s);
    assert_eq!(st.overflow_pages, 100_000usize.div_ceil(OVERFLOW_CAPACITY));
    // Replacing and deleting must free the old chain.
    t.insert(&mut s, b"big", &big[..5000]).unwrap();
    assert_eq!(t.get(&mut s, b"big").unwrap().unwrap(), &big[..5000]);
    check_all(&t, &mut s);
    t.delete(&mut s, b"big").unwrap();
    let st = check_all(&t, &mut s);
    assert_eq!(st.overflow_pages, 0);
    // Scans read overflow values too.
    t.insert(&mut s, b"big2", &big).unwrap();
    let all = t.scan(&mut s).unwrap().collect(&mut s).unwrap();
    assert_eq!(all[0].1, big);
}

#[test]
fn oversized_key_is_rejected() {
    let mut s = MemStore::new();
    let t = BTree::create(&mut s).unwrap();
    let err = t.insert(&mut s, &[0u8; MAX_KEY_SIZE + 1], b"").unwrap_err();
    assert!(matches!(err, Error::Limit(_)));
    t.insert(&mut s, &[1u8; MAX_KEY_SIZE], &[2u8; 3000]).unwrap();
}

#[test]
fn max_size_keys_split_correctly() {
    let mut s = MemStore::new();
    let t = BTree::create(&mut s).unwrap();
    for i in 0..2000u32 {
        let mut k = vec![b'k'; MAX_KEY_SIZE];
        k[..4].copy_from_slice(&i.to_be_bytes());
        t.insert(&mut s, &k, &[0u8; 400]).unwrap();
    }
    let st = check_all(&t, &mut s);
    assert_eq!(st.entries, 2000);
}

#[test]
fn destroy_frees_everything() {
    let mut s = MemStore::new();
    let t = BTree::create(&mut s).unwrap();
    for i in 0..3000 {
        t.insert(&mut s, &key(i), &[1u8; 2000]).unwrap(); // some overflow
    }
    t.destroy(&mut s).unwrap();
    assert_eq!(s.live_pages(), 0);
}

// --- property tests ----------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    Insert(Vec<u8>, Vec<u8>),
    Delete(Vec<u8>),
    Get(Vec<u8>),
    Range(Vec<u8>, Vec<u8>),
}

fn arb_key() -> impl Strategy<Value = Vec<u8>> {
    // A small alphabet and short keys force plenty of collisions, so
    // replaces and deletes of existing keys are common.
    prop::collection::vec(0u8..4, 0..6)
}

fn arb_value() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        8 => prop::collection::vec(any::<u8>(), 0..400),
        1 => prop::collection::vec(any::<u8>(), 900..9000), // overflow
    ]
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        5 => (arb_key(), arb_value()).prop_map(|(k, v)| Op::Insert(k, v)),
        3 => arb_key().prop_map(Op::Delete),
        1 => arb_key().prop_map(Op::Get),
        1 => (arb_key(), arb_key()).prop_map(|(a, b)| Op::Range(a, b)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn behaves_like_btreemap(ops in prop::collection::vec(arb_op(), 1..600)) {
        let mut s = MemStore::new();
        let t = BTree::create(&mut s).unwrap();
        let mut model: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
        for op in ops {
            match op {
                Op::Insert(k, v) => {
                    let existed = t.insert(&mut s, &k, &v).unwrap();
                    prop_assert_eq!(existed, model.insert(k, v).is_some());
                }
                Op::Delete(k) => {
                    prop_assert_eq!(t.delete(&mut s, &k).unwrap(), model.remove(&k).is_some());
                }
                Op::Get(k) => {
                    prop_assert_eq!(t.get(&mut s, &k).unwrap(), model.get(&k).cloned());
                }
                Op::Range(a, b) => {
                    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                    let got = t
                        .range(&mut s, Bound::Included(&lo), Bound::Excluded(&hi))
                        .unwrap()
                        .collect(&mut s)
                        .unwrap();
                    let want: Vec<_> = model
                        .range(lo..hi)
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    prop_assert_eq!(got, want);
                }
            }
        }
        let st = check_all(&t, &mut s);
        prop_assert_eq!(st.entries, model.len());
        let all = t.scan(&mut s).unwrap().collect(&mut s).unwrap();
        let want: Vec<_> = model.into_iter().collect();
        prop_assert_eq!(t.last(&mut s).unwrap(), want.last().cloned());
        prop_assert_eq!(all, want);
    }

    /// Many keys with larger values so the tree gets several levels deep,
    /// then delete a random subset and compare.
    #[test]
    fn deep_tree_insert_then_delete(
        keys in prop::collection::btree_set(any::<u32>(), 200..1500),
        del_mask in any::<u64>(),
    ) {
        let mut s = MemStore::new();
        let t = BTree::create(&mut s).unwrap();
        let mut model = BTreeMap::new();
        for &k in &keys {
            let kb = k.to_be_bytes().to_vec();
            let v = vec![(k % 256) as u8; (k % 200) as usize];
            t.insert(&mut s, &kb, &v).unwrap();
            model.insert(kb, v);
        }
        for (i, &k) in keys.iter().enumerate() {
            if del_mask.rotate_left(i as u32) & 1 == 1 {
                let kb = k.to_be_bytes().to_vec();
                prop_assert!(t.delete(&mut s, &kb).unwrap());
                model.remove(&kb);
            }
        }
        check_all(&t, &mut s);
        let all = t.scan(&mut s).unwrap().collect(&mut s).unwrap();
        prop_assert_eq!(all, model.into_iter().collect::<Vec<_>>());
    }
}
