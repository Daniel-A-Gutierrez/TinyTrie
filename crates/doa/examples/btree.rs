//! B+ tree consumer over doa's three-layer walker ladder.
//! u16 pointers, `Uniform` block, `PreOrder` layout.
//!
//! Splits are not wired in the crate (`TreeWalkMut::insert_child` returns `NodeFull`
//! for a full parent), so the map's `insert` is leaf-only and the two-level tree in
//! `two_level_demo` is hand-assembled with `insert_child` (no split driver).

use arrays::tiny_array::TinyArray;
use doa::blocks::{BlockTrait, UniformBlock};
use doa::metadata::{Ancestry, Fixable, Fixup, HasRoot, PosAncestry};
use doa::translator::Translator;
use doa::treeblock::{search, walker};
use doa::walker::{
    InsertErr, Node, NodeCursor, NodeWalker, NodeWalkerMut, TreeWalk, TreeWalkMut, TreeWalker,
};
use doa::PreOrder;
use std::cmp::Ordering;

const DEGREE: usize = 6; //max children per inode; leaves hold up to DEGREE pairs

struct INode {
    keys:     TinyArray<u64, { DEGREE - 1 }>,
    children: TinyArray<u16, DEGREE>,
}
struct LNode {
    keys:   TinyArray<u64, DEGREE>,
    values: TinyArray<u64, DEGREE>,
}

enum BNode {
    Internal(INode),
    Leaf(LNode),
}

impl BNode {
    fn internal() -> Self {
        BNode::Internal(INode { keys: TinyArray::new(), children: TinyArray::new() })
    }
    fn leaf(pairs: &[(u64, u64)]) -> Self {
        let mut n = LNode { keys: TinyArray::new(), values: TinyArray::new() };
        for (k, v) in pairs {
            n.keys.push(*k);
            n.values.push(*v);
        }
        BNode::Leaf(n)
    }
}

impl Default for BNode {
    fn default() -> Self {
        BNode::leaf(&[])
    }
}

impl Node for BNode {
    type K = u64;
    type V = u64;
    type P = u16;
    const DEGREE: usize = DEGREE;
}

///root phys + tree height (B+ leaves sit at depth == height).
#[derive(Clone, Copy, Debug, Default)]
struct BTreeMeta {
    root:   usize,
    height: u32,
}

impl Fixable<u16> for BTreeMeta {
    fn fixup<F: Fixup + ?Sized>(&mut self, f: &F, _tr: &Translator<u16>) {
        if f.affects_p(self.root) {
            f.fix_p(&mut self.root);
        }
    }
}

impl HasRoot<u16> for BTreeMeta {
    fn root(&self) -> usize {
        self.root
    }
    fn set_root(&mut self, root: usize) {
        self.root = root;
    }
}

///the crate impls `TreeBlock` for `Block<…, Uniform, …>` directly — no newtype, no
///forwarding; the walker types enter the constructors as fn generics.
type BlockT<'block> = UniformBlock<'block, BNode, u16, BTreeMeta, PreOrder>;

///min key of the child at vaddr `c` (leaf children only — the demo's height is 1).
fn child_min(b: &BlockT<'_>, c: u16) -> u64 {
    match b.vget(c) {
        BNode::Leaf(n) => *n.keys.get(0),
        _ => panic!("child_min: internal child (demo supports leaf children only)"),
    }
}

fn scan_leaf(n: &LNode, k: &u64) -> Option<u64> {
    n.keys.as_slice().iter().position(|&key| key == *k).map(|i| *n.values.get(i))
}

fn block_get(b: &BlockT<'_>, k: &u64) -> Option<u64> {
    let w: TreeWalker<PreOrder, Cursor<'_, '_>> = search(b, k);
    match w.nw.current() {
        BNode::Leaf(n) => scan_leaf(n, k),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// layer 1 — consumer cursors. tracked state = the crate's `PosAncestry`.
// ---------------------------------------------------------------------------

fn root_state(b: &BlockT<'_>) -> PosAncestry {
    PosAncestry { pos: b.data().root, ancestry: Ancestry::default() }
}

struct Cursor<'block, 'walker> {
    b:     &'walker BlockT<'block>,
    state: PosAncestry,
}

impl<'block, 'a> From<&'a BlockT<'block>> for Cursor<'block, 'a>
where
    'block: 'a,
{
    fn from(b: &'a BlockT<'block>) -> Self {
        Self { b, state: root_state(b) }
    }
}

impl<'block, 'walker> NodeCursor<'block, BlockT<'block>> for Cursor<'block, 'walker> {
    fn block(&self) -> &BlockT<'block> {
        self.b
    }
    fn position(&self) -> usize {
        self.state.pos
    }
    fn is_leaf(&self) -> bool {
        self.state.ancestry.len() == self.b.data().height as usize
    }
    fn current(&self) -> &BNode {
        self.b.get(self.state.pos)
    }
    fn child_count(&self) -> usize {
        match self.current() {
            BNode::Internal(n) => n.children.len(),
            BNode::Leaf(_) => 0,
        }
    }
    fn child(&self, idx: usize) -> u16 {
        match self.current() {
            BNode::Internal(n) => *n.children.get(idx),
            BNode::Leaf(_) => panic!("child: leaf"),
        }
    }
    ///`(pos, cmp)`: pos = descent child, cmp = k vs that child. B+ separators bound
    ///children 1.. — child 0 is unbounded below (min fetch); k within a child's key
    ///span is `Equal` (equal-right); beyond the last separator is the append
    ///`(cc-1, Greater)`. leaves: item-relative (unused by tree ops).
    fn lookup(&self, k: &u64) -> (usize, Ordering) {
        match self.current() {
            BNode::Leaf(n) => {
                let ks = n.keys.as_slice();
                match ks.iter().position(|&key| key >= *k) {
                    Some(i) if ks[i] == *k => (i, Ordering::Equal),
                    Some(i) => (i, Ordering::Less),
                    None if ks.is_empty() => (0, Ordering::Less),
                    None => (ks.len() - 1, Ordering::Greater),
                }
            }
            BNode::Internal(n) => {
                let cc = n.children.len();
                if cc == 0 {
                    return (0, Ordering::Less);
                }
                let keys = n.keys.as_slice();
                let p = keys.iter().position(|&key| key > *k).unwrap_or(keys.len());
                if p == 0 {
                    if *k < child_min(self.b, *n.children.get(0)) {
                        (0, Ordering::Less)
                    } else {
                        (0, Ordering::Equal)
                    }
                } else if p == keys.len() && *k > keys[keys.len() - 1] {
                    (cc - 1, Ordering::Greater)
                } else {
                    (p, Ordering::Equal) //k ∈ child p's span, equal-right on the left edge
                }
            }
        }
    }

    ///B+ routing: descend `pos` in every case (spans are contiguous and equal-right;
    ///the append `Greater` is still the last child).
    fn search(&mut self, k: &u64) -> Option<&BNode> {
        if self.b.occupied() == 0 {
            return None;
        }
        while !self.is_leaf() {
            let (i, _) = self.lookup(k);
            self.descend(i);
        }
        Some(self.current())
    }
    fn descend(&mut self, child_idx: usize) -> &BNode {
        let child = self.child(child_idx);
        let phys = self.b.v2p(child);
        self.state.ancestry.push(self.state.pos, child_idx);
        self.state.pos = phys;
        self.b.get(phys)
    }
}

impl<'block, 'walker> NodeWalker<'block, BlockT<'block>> for Cursor<'block, 'walker> {
    fn depth(&self) -> usize {
        self.state.ancestry.len()
    }
    fn ascend(&mut self) -> &BNode {
        let a = self.state.ancestry.pop().expect("ascend: at root");
        self.state.pos = a.parent;
        self.b.get(self.state.pos)
    }
    fn parent(&self) -> Option<(usize, usize)> {
        self.state.ancestry.last().map(|a| (a.parent, a.child))
    }
}

struct CursorMut<'block, 'walker> {
    b:     &'walker mut BlockT<'block>,
    state: PosAncestry,
}

impl<'block, 'a> From<&'a mut BlockT<'block>> for CursorMut<'block, 'a>
where
    'block: 'a,
{
    fn from(b: &'a mut BlockT<'block>) -> Self {
        let state = root_state(b);
        Self { b, state }
    }
}

//the mut cursor reborrows its `&'walker mut B` down to shared through `&self` — all the
//read methods are safe since their returns tie to the `&self` borrow, not `'walker`.
impl<'block, 'walker> NodeCursor<'block, BlockT<'block>> for CursorMut<'block, 'walker> {
    fn block(&self) -> &BlockT<'block> {
        self.b
    }
    fn position(&self) -> usize {
        self.state.pos
    }
    fn is_leaf(&self) -> bool {
        self.state.ancestry.len() == self.block().data().height as usize
    }
    fn current(&self) -> &BNode {
        self.block().get(self.state.pos)
    }
    fn child_count(&self) -> usize {
        match self.current() {
            BNode::Internal(n) => n.children.len(),
            BNode::Leaf(_) => 0,
        }
    }
    fn child(&self, idx: usize) -> u16 {
        match self.current() {
            BNode::Internal(n) => *n.children.get(idx),
            BNode::Leaf(_) => panic!("child: leaf"),
        }
    }
    ///`(pos, cmp)`: pos = descent child, cmp = k vs that child. B+ separators bound
    ///children 1.. — child 0 is unbounded below (min fetch); k within a child's key
    ///span is `Equal` (equal-right); beyond the last separator is the append
    ///`(cc-1, Greater)`. leaves: item-relative (unused by tree ops).
    fn lookup(&self, k: &u64) -> (usize, Ordering) {
        match self.current() {
            BNode::Leaf(n) => {
                let ks = n.keys.as_slice();
                match ks.iter().position(|&key| key >= *k) {
                    Some(i) if ks[i] == *k => (i, Ordering::Equal),
                    Some(i) => (i, Ordering::Less),
                    None if ks.is_empty() => (0, Ordering::Less),
                    None => (ks.len() - 1, Ordering::Greater),
                }
            }
            BNode::Internal(n) => {
                let cc = n.children.len();
                if cc == 0 {
                    return (0, Ordering::Less);
                }
                let keys = n.keys.as_slice();
                let p = keys.iter().position(|&key| key > *k).unwrap_or(keys.len());
                if p == 0 {
                    if *k < child_min(self.block(), *n.children.get(0)) {
                        (0, Ordering::Less)
                    } else {
                        (0, Ordering::Equal)
                    }
                } else if p == keys.len() && *k > keys[keys.len() - 1] {
                    (cc - 1, Ordering::Greater)
                } else {
                    (p, Ordering::Equal) //k ∈ child p's span, equal-right on the left edge
                }
            }
        }
    }

    ///B+ routing: descend `pos` in every case (spans are contiguous and equal-right;
    ///the append `Greater` is still the last child).
    fn search(&mut self, k: &u64) -> Option<&BNode> {
        if self.block().occupied() == 0 {
            return None;
        }
        while !self.is_leaf() {
            let (i, _) = self.lookup(k);
            self.descend(i);
        }
        Some(self.current())
    }
    fn descend(&mut self, child_idx: usize) -> &BNode {
        let child = self.child(child_idx);
        let phys = self.block().v2p(child);
        self.state.ancestry.push(self.state.pos, child_idx);
        self.state.pos = phys;
        self.block().get(phys)
    }
}

impl<'block, 'walker> NodeWalker<'block, BlockT<'block>> for CursorMut<'block, 'walker> {
    fn depth(&self) -> usize {
        self.state.ancestry.len()
    }
    fn ascend(&mut self) -> &BNode {
        let a = self.state.ancestry.pop().expect("ascend: at root");
        self.state.pos = a.parent;
        self.block().get(self.state.pos)
    }
    fn parent(&self) -> Option<(usize, usize)> {
        self.state.ancestry.last().map(|a| (a.parent, a.child))
    }
}

///what the node-level `insert_child` places. the separator is re-derived from the
///placed child's own min — no key carried.
#[allow(dead_code)] //Value: no leaf-level caller goes through the trait yet
enum Payload {
    Value(u64, u64),
    Child(u16),
}

impl<'block, 'walker> NodeWalkerMut<'block, BlockT<'block>> for CursorMut<'block, 'walker> {
    type Payload = Payload;
    type State = PosAncestry;

    fn child_payload(&self, _k: &u64, ptr: u16) -> Payload {
        Payload::Child(ptr)
    }

    fn parts(&mut self) -> (&mut PosAncestry, &BlockT<'block>) {
        (&mut self.state, &*self.b)
    }
    fn parts_mut(&mut self) -> (&mut PosAncestry, &mut BlockT<'block>) {
        (&mut self.state, &mut *self.b)
    }

    fn block_mut(&mut self) -> &mut BlockT<'block> {
        self.b
    }
    fn current_mut(&mut self) -> &mut BNode {
        self.b.get_mut(self.state.pos)
    }
    fn has_space(&self) -> bool {
        match self.current() {
            BNode::Internal(n) => !n.children.is_full(),
            BNode::Leaf(n) => !n.keys.is_full(),
        }
    }
    fn set_child(&mut self, up: usize, child_idx: usize, ptr: u16) {
        let target = match up {
            0 => self.state.pos,
            n => self.state.ancestry.stack[self.state.ancestry.len() - n].parent,
        };
        match self.block_mut().get_mut(target) {
            BNode::Internal(n) => *n.children.get_mut(child_idx) = ptr,
            BNode::Leaf(_) => panic!("set_child: leaf"),
        }
    }
    fn set_parent(&mut self, _ptr: u16) {} //nodes store no parent fields
    ///node-level wire. `child_idx` is the crate-routed gap (lookup); the separator is
    ///re-derived from the placed child's own min (B+ separators are child mins, not the
    ///caller's routing key): children stay sorted, keys[i] = min(children[i+1]) holds.
    fn insert_child(&mut self, child_idx: usize, payload: Payload) {
        match payload {
            Payload::Value(k, v) => {
                let BNode::Leaf(n) = self.current_mut() else { panic!("insert_child: value into non-leaf") };
                let pos = n.keys.as_slice().iter().position(|&key| k < key).unwrap_or(n.keys.len());
                n.keys.insert_at(pos, k);
                n.values.insert_at(pos, v);
            }
            Payload::Child(ptr) => {
                //separator from the placed child's own min; new leftmost's separator
                //is the OLD leftmost's min
                let (m, old_left) = {
                    let shared = self.block();
                    let m = child_min(shared, ptr);
                    let BNode::Internal(n) = shared.get(self.state.pos) else {
                        panic!("insert_child: child into non-internal")
                    };
                    let old_left = if child_idx == 0 && n.children.len() > 0 {
                        Some(child_min(shared, *n.children.get(0)))
                    } else {
                        None
                    };
                    (m, old_left)
                };
                let BNode::Internal(n) = self.current_mut() else { panic!() };
                n.children.insert_at(child_idx, ptr);
                if child_idx == 0 {
                    if let Some(sep) = old_left {
                        n.keys.insert_at(0, sep);
                    }
                } else {
                    n.keys.insert_at(child_idx - 1, m);
                }
            }
        }
    }
    fn remove_child(&mut self, child_idx: usize) -> Payload {
        let BNode::Internal(n) = self.current_mut() else { panic!("remove_child: not internal") };
        let p = n.children.remove(child_idx);
        let _ = n.keys.remove(child_idx.saturating_sub(1));
        Payload::Child(p)
    }
}

// ---------------------------------------------------------------------------
// the map
// ---------------------------------------------------------------------------

pub struct BTreeMap {
    block: BlockT<'static>,
    len:   usize,
}

impl BTreeMap {
    pub fn new() -> Self {
        let mut block = BlockT::new();
        let root = block.insert_root(BNode::default());
        block.set_data(BTreeMeta { root, height: 0 });
        Self { block, len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn get(&self, k: &u64) -> Option<u64> {
        let w: TreeWalker<PreOrder, Cursor<'_, '_>> = search(&self.block, k);
        match w.nw.current() {
            BNode::Leaf(n) => scan_leaf(n, k),
            _ => None,
        }
    }

    ///leaf-level insert only — no split driver in the crate yet; a full leaf errors.
    pub fn insert(&mut self, k: u64, v: u64) -> Result<(), InsertErr> {
        {
            let mut w: TreeWalker<PreOrder, CursorMut<'_, '_>> = search(&mut self.block, &k);
            let BNode::Leaf(n) = w.nw.current_mut() else { panic!("insert: not at a leaf") };
            if let Some(pos) = n.keys.as_slice().iter().position(|&key| key == k) {
                *n.values.get_mut(pos) = v;
                return Ok(());
            }
            if n.keys.is_full() {
                return Err(InsertErr::NodeFull);
            }
            let pos = n.keys.as_slice().iter().position(|&key| k < key).unwrap_or(n.keys.len());
            n.keys.insert_at(pos, k);
            n.values.insert_at(pos, v);
        }
        self.len += 1;
        Ok(())
    }

    pub fn remove(&mut self, k: &u64) -> Option<u64> {
        let v = {
            let mut w: TreeWalker<PreOrder, CursorMut<'_, '_>> = search(&mut self.block, k);
            let BNode::Leaf(n) = w.nw.current_mut() else { return None };
            let pos = n.keys.as_slice().iter().position(|&key| key == *k)?;
            let v = n.values.remove(pos);
            n.keys.remove(pos);
            v
        };
        self.len -= 1;
        Some(v)
    }

    ///all (k, v) pairs in key order, via the preorder walk (skipping internal nodes).
    pub fn pairs(&self) -> Vec<(u64, u64)> {
        let mut out = vec![];
        let mut w: TreeWalker<PreOrder, Cursor<'_, '_>> = walker(&self.block);
        if w.first().is_none() {
            return out;
        }
        loop {
            if let BNode::Leaf(n) = w.nw.current() {
                out.extend(
                    n.keys.as_slice().iter().zip(n.values.as_slice()).map(|(k, v)| (*k, *v)),
                );
            }
            if w.next().is_none() {
                break;
            }
        }
        out
    }
}

impl Default for BTreeMap {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// demos
// ---------------------------------------------------------------------------

fn map_demo() {
    let mut m = BTreeMap::new();
    for (k, v) in [(10, 100), (5, 50), (20, 200), (15, 150), (25, 250), (30, 300)] {
        m.insert(k, v).unwrap();
    }
    assert_eq!(m.insert(35, 350), Err(InsertErr::NodeFull)); //leaf full, splits unwired

    for (k, v) in [(10, 100), (5, 50), (20, 200), (15, 150), (25, 250), (30, 300)] {
        assert_eq!(m.get(&k), Some(v));
    }
    assert_eq!(m.get(&7), None);
    assert_eq!(m.get(&22), None);

    assert_eq!(m.remove(&15), Some(150));
    assert_eq!(m.get(&15), None);
    assert_eq!(m.len(), 5);

    let pairs = m.pairs();
    let want: Vec<(u64, u64)> = [(5, 50), (10, 100), (20, 200), (25, 250), (30, 300)].to_vec();
    assert_eq!(pairs, want);
    println!("map demo: ok (get/insert/full/remove/iter over a leaf root)");
}

///hand-assembled two-level tree: root inode + five leaves placed via the crate's
///`TreeWalkMut::insert_child` (spreads + slides + wiring + preorder placement, no split
///driver). 40/30/25 are "new leftmost" inserts (gap 0 → parent-adjacent anchor); 35 is
///a mid-gap insert (descend + subtree-edge anchor); the final 20 insert lands on the
///out-of-run slide case — the None sits two slots right of the anchor, so fixup walks
///the run from below the anchor and restores its state instead of walking back through
///the pointers it just rewrote.
fn two_level_demo() {
    let mut block = BlockT::new();
    let root = block.insert_root(BNode::internal());
    block.set_data(BTreeMeta { root, height: 1 });

    {
        let mut w: TreeWalker<PreOrder, CursorMut<'_, '_>> = walker(&mut block); //at the root
        w.insert_child(&40, BNode::leaf(&[(40, 400), (42, 421)])).unwrap();
        w.insert_child(&30, BNode::leaf(&[(30, 303), (33, 331)])).unwrap();
        w.insert_child(&35, BNode::leaf(&[(35, 351), (37, 372)])).unwrap();
        w.insert_child(&25, BNode::leaf(&[(25, 251)])).unwrap();
        w.insert_child(&20, BNode::leaf(&[(20, 201)])).unwrap();
    }

    //preorder node order: root then leaves in key order
    let mut w: TreeWalker<PreOrder, Cursor<'_, '_>> = walker(&block);
    w.first().unwrap();
    let mut order = vec![];
    loop {
        match w.nw.current() {
            BNode::Internal(n) => {
                order.push(1000 + n.children.len() as u64);
            }
            BNode::Leaf(n) => order.push(*n.keys.get(0)),
        }
        if w.next().is_none() {
            break;
        }
    }
    assert_eq!(order, vec![1005, 20, 25, 30, 35, 40]);

    for (k, v) in [
        (20, 201),
        (25, 251),
        (30, 303),
        (33, 331),
        (35, 351),
        (37, 372),
        (40, 400),
        (42, 421),
    ] {
        assert_eq!(block_get(&block, &k), Some(v), "get({k})");
    }
    assert_eq!(block_get(&block, &22), None);
    assert_eq!(block_get(&block, &28), None);
    assert_eq!(block_get(&block, &50), None);

    println!("two-level demo: ok (insert_child x5, all anchor kinds, in-run + out-of-run slides, preorder placement, cross-leaf get)");
}

fn main() {
    map_demo();
    two_level_demo();
    println!("btree example: all checks passed");
}