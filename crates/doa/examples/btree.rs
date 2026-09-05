//! B+ tree consumer over doa's three-layer walker ladder.
//! u16 pointers, `Uniform` block, `PreOrder` layout.
//!
//! Splits are wired (`SplitTreeWalker::split_child`/`split_root`): the map's insert
//! drives them bottom-up, and `two_level_demo` hand-assembles with `insert_child`.

use arrays::tiny_array::TinyArray;
use doa::PreOrder;
use doa::blocks::{BlockTrait, UniformBlock};
use doa::metadata::{Ancestry, Fixable, Fixup, HasRoot, PosAncestry};
use doa::translator::Translator;
use doa::treeblock::{search, walker};
use doa::walker::{InsertErr, Node, NodeCursor, NodeWalker, NodeWalkerMut, SplitTreeWalker,
                  SplittableNode, TreeWalk, TreeWalkMut, TreeWalker};
use std::cmp::Ordering;
use std::mem::MaybeUninit;

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

impl Node for BNode {
    type K = u64;
    type V = u64;
    type P = u16;
    const DEGREE: usize = DEGREE;
    const STORES_PARENTS: bool = false; //nodes carry no parent fields
    type Payload = (); //B+ separators are child mins — nothing promotes besides them
}

impl SplittableNode for BNode {
    ///promoted root, pre-wired with its first child = the old root.
    fn new_root(r_v: u16) -> Self {
        let mut children = TinyArray::new();
        children.push(r_v);
        BNode::Internal(INode { keys: TinyArray::new(), children })
    }

    ///drain the right half into the reserved `slot`. leaf: promote the right
    ///half's min (copied up — leaves keep all keys); internal: move the boundary
    ///separator up.
    fn split(&mut self, slot: &mut MaybeUninit<Self>) -> (Self::K, Self::Payload) {
        match self {
            BNode::Leaf(n) => {
                let len = n.keys.len();
                let mid = len >> 1;
                let mut r = LNode { keys: TinyArray::new(), values: TinyArray::new() };
                for i in mid..len {
                    r.keys.push(*n.keys.get(i));
                    r.values.push(*n.values.get(i));
                }
                for _ in mid..len {
                    n.keys.remove(mid);
                    n.values.remove(mid);
                }
                let sep = *r.keys.get(0);
                slot.write(BNode::Leaf(r));
                (sep, ())
            }
            BNode::Internal(n) => {
                let cc = n.children.len();
                let mid = cc >> 1;
                let mut r = INode { keys: TinyArray::new(), children: TinyArray::new() };
                let sep = *n.keys.get(mid - 1); //min of child[mid] — the boundary
                for i in mid..cc {
                    r.children.push(*n.children.get(i));
                }
                for i in mid..cc - 1 {
                    r.keys.push(*n.keys.get(i));
                }
                for _ in mid..cc {
                    n.children.remove(mid);
                }
                for _ in mid - 1..cc - 1 {
                    n.keys.remove(mid - 1);
                }
                slot.write(BNode::Internal(r));
                (sep, ())
            }
        }
    }
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
    fn height(&self) -> u32 {
        self.height
    }
    fn set_height(&mut self, height: u32) {
        self.height = height;
    }
}

///the crate impls `TreeBlock` for `Block<…, Uniform, …>` directly — no newtype, no
///forwarding; the walker types enter the constructors as fn generics.
type BlockT<'block> = UniformBlock<'block, BNode, u16, BTreeMeta, PreOrder>;

///min key of the subtree rooted at vaddr `c` — its leftmost leaf's first key.
fn child_min(b: &BlockT<'_>, mut c: u16) -> u64 {
    loop {
        match b.vget(c) {
            BNode::Leaf(n) => return *n.keys.get(0),
            BNode::Internal(n) => c = *n.children.get(0),
        }
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
where 'block: 'a
{
    fn from(b: &'a BlockT<'block>) -> Self {
        Self { b, state: root_state(b) }
    }
}

impl<'block, 'walker> NodeCursor<'block, BlockT<'block>> for Cursor<'block, 'walker> {
    type State = PosAncestry;

    fn state(&self) -> &PosAncestry {
        &self.state
    }
    fn state_mut(&mut self) -> &mut PosAncestry {
        &mut self.state
    }
    fn block(&self) -> &BlockT<'block> {
        self.b
    }
    fn is_leaf(&self) -> bool {
        self.state.ancestry.len() == self.b.data().height as usize
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
}

//position/current/descend: crate defaults over the state traits.
//ascend/parent: per-shape — parent knowledge is the `PosAncestry` stack.
impl<'block, 'walker> NodeWalker<'block, BlockT<'block>> for Cursor<'block, 'walker> {
    fn ascend<'b>(&'b mut self) -> &'b BNode
    where 'block: 'b {
        let a = self.state.ancestry.pop().expect("ascend: at root");
        self.state.pos = a.parent;
        self.b.get(a.parent)
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
where 'block: 'a
{
    fn from(b: &'a mut BlockT<'block>) -> Self {
        let state = root_state(b);
        Self { b, state }
    }
}

//the mut cursor reborrows its `&'walker mut B` down to shared through `&self` — all the
//read methods are safe since their returns tie to the `&self` borrow, not `'walker`.
impl<'block, 'walker> NodeCursor<'block, BlockT<'block>> for CursorMut<'block, 'walker> {
    type State = PosAncestry;

    fn state(&self) -> &PosAncestry {
        &self.state
    }
    fn state_mut(&mut self) -> &mut PosAncestry {
        &mut self.state
    }
    fn block(&self) -> &BlockT<'block> {
        self.b
    }
    fn is_leaf(&self) -> bool {
        self.state.ancestry.len() == self.block().data().height as usize
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
}

//position/current/descend: crate defaults over the state traits.
//ascend/parent: per-shape — this walker's parent knowledge is its `PosAncestry`
//stack (a parent-pointer tree would read the node's stored field instead).
impl<'block, 'walker> NodeWalker<'block, BlockT<'block>> for CursorMut<'block, 'walker> {
    fn ascend<'b>(&'b mut self) -> &'b BNode
    where 'block: 'b {
        let a = self.state.ancestry.pop().expect("ascend: at root");
        self.state.pos = a.parent;
        self.block().get(a.parent)
    }
    fn parent(&self) -> Option<(usize, usize)> {
        self.state.ancestry.last().map(|a| (a.parent, a.child))
    }
}

impl<'block, 'walker> NodeWalkerMut<'block, BlockT<'block>> for CursorMut<'block, 'walker> {
    fn parts(&mut self) -> (&mut PosAncestry, &BlockT<'block>) {
        (&mut self.state, &*self.b)
    }
    fn parts_mut(&mut self) -> (&mut PosAncestry, &mut BlockT<'block>) {
        (&mut self.state, &mut *self.b)
    }

    fn block_mut(&mut self) -> &mut BlockT<'block> {
        self.b
    }
    //current_mut/set_position: crate defaults over `CursorState`.
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
    ///node-level wire. the separator is re-derived from the placed child's own min
    ///(B+ separators are child mins, not the caller's routing key): children stay
    ///sorted, keys[i] = min(children[i+1]) holds.
    fn insert_child(&mut self, child_idx: usize, _k: &u64, _payload: (), ptr: u16) {
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
    fn remove_child(&mut self, child_idx: usize) -> (Option<u64>, Option<()>, u16) {
        let BNode::Internal(n) = self.current_mut() else {
            panic!("remove_child: not internal")
        };
        let p = n.children.remove(child_idx);
        let sep = (child_idx > 0).then(|| *n.keys.get(child_idx - 1));
        let _ = n.keys.remove(child_idx.saturating_sub(1));
        (sep, None, p)
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
        let root = block.insert_root(BNode::leaf(&[])); //fresh tree root = a leaf
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

    ///insert with the split driver: on a full leaf, ascend to the nearest node with
    ///room and split its full child on the path (`split_root` at the top), then
    ///re-search. every split shifts indices, so retrying from the top is simpler and
    ///sounder than tracking the path.
    pub fn insert(&mut self, k: u64, v: u64) -> Result<(), InsertErr> {
        enum Step {
            Done,
            Placed,
            Full,
        }
        loop {
            let mut w: TreeWalker<PreOrder, CursorMut<'_, '_>> = search(&mut self.block, &k);
            let step = match w.nw.current_mut() {
                BNode::Leaf(n) if n.keys.as_slice().contains(&k) => {
                    let pos = n.keys.as_slice().iter().position(|&key| key == k).unwrap();
                    *n.values.get_mut(pos) = v;
                    Step::Done //overwrite: no len change
                }
                BNode::Leaf(n) if !n.keys.is_full() => {
                    let pos = n
                        .keys
                        .as_slice()
                        .iter()
                        .position(|&key| k < key)
                        .unwrap_or(n.keys.len());
                    n.keys.insert_at(pos, k);
                    n.values.insert_at(pos, v);
                    Step::Placed
                }
                _ => Step::Full, //full leaf: split the path below
            };
            match step {
                Step::Done => return Ok(()),
                Step::Placed => {
                    self.len += 1;
                    return Ok(());
                }
                Step::Full => {}
            }
            //walk up to the first node with room; split the full child we came from
            //(the root has no parent — split it itself)
            loop {
                let Some((_, idx)) = w.nw.parent() else {
                    w.split_root()?;
                    break;
                };
                w.nw.ascend();
                if w.nw.has_space() {
                    w.split_child(idx)?;
                    break;
                }
            }
        }
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
    //enough inserts to split the leaf root (root promotion) and split leaves under
    //a two-level tree — DEGREE 6, so 100 keys ⇒ ≥3 leaves
    let items: Vec<(u64, u64)> = (0..100u64).map(|i| (i * 13 + 1, i * 100 + 7)).collect();
    for (k, v) in &items {
        m.insert(*k, *v).unwrap();
    }
    assert!(m.block.data().height >= 2); //multiple root promotions

    for (k, v) in &items {
        assert_eq!(m.get(k), Some(*v), "get({k})");
    }
    assert_eq!(m.get(&0), None);
    assert_eq!(m.get(&2), None); //between 1 and 14

    //overwrite
    m.insert(40, 999).unwrap();
    assert_eq!(m.get(&40), Some(999));
    assert_eq!(m.len(), 100);

    assert_eq!(m.remove(&14), Some(107));
    assert_eq!(m.get(&14), None);
    assert_eq!(m.len(), 99);

    let pairs = m.pairs();
    let want: Vec<(u64, u64)> = {
        let mut w = items.clone();
        w.retain(|&(k, _)| k != 14 && k != 40); //14 removed; 40 overwritten
        w.push((40, 999));
        w.sort();
        w
    };
    assert_eq!(pairs, want);
    println!(
        "map demo: ok (100 keys, splits + multi root promotions, get/overwrite/remove/iter)"
    );
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
        w.insert_child(&40, (), BNode::leaf(&[(40, 400), (42, 421)])).unwrap();
        w.insert_child(&30, (), BNode::leaf(&[(30, 303), (33, 331)])).unwrap();
        w.insert_child(&35, (), BNode::leaf(&[(35, 351), (37, 372)])).unwrap();
        w.insert_child(&25, (), BNode::leaf(&[(25, 251)])).unwrap();
        w.insert_child(&20, (), BNode::leaf(&[(20, 201)])).unwrap();
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

    for (k, v) in
        [(20, 201), (25, 251), (30, 303), (33, 331), (35, 351), (37, 372), (40, 400), (42, 421)]
    {
        assert_eq!(block_get(&block, &k), Some(v), "get({k})");
    }
    assert_eq!(block_get(&block, &22), None);
    assert_eq!(block_get(&block, &28), None);
    assert_eq!(block_get(&block, &50), None);

    println!(
        "two-level demo: ok (insert_child x5, all anchor kinds, in-run + out-of-run slides, preorder placement, cross-leaf get)"
    );
}

fn main() {
    map_demo();
    two_level_demo();
    println!("btree example: all checks passed");
}
