//!split-driver tests: one parent-storing (`STORES_PARENTS`) B+ consumer, Vec-backed,
//!run under all three orderings. exercises the split driver arms (leaf/internal/root,
//!pre/in/post), the in-order parent hop (insert + split triggers), the postorder
//!X-relocation swaps, `open_two`/`find_2_slots`, and the reparent matrix
//!(`swap_current`/`reparent_run`/`adopt_node`).
//!validation: structural DFS (stored parent fields, separator re-derivation, leaf
//!order, reachable-node count == occupied) + `TreeWalk` order vs a reference DFS with
//!strictly increasing phys (the walk-==-slot-order invariant).
use std::cmp::Ordering as Cmp;
use std::mem::MaybeUninit;

use crate::metadata::{Fixable, HasRoot, PosAncestry};
use crate::treeblock::search;
use crate::walker::{InsertErr, Node, NodeCursor, NodeWalker, NodeWalkerMut, SplitTreeWalker,
                    SplittableNode, TreeWalk, TreeWalkMut, TreeWalker};
use crate::{InOrder, Order, Ordering, PostOrder, PreOrder};

use crate::blocks::{BlockTrait, UniformBlock};
use crate::store::Store;

const DEGREE: usize = 4; //small — force many splits and promotions

struct INode {
    keys:     Vec<u64>,
    children: Vec<u16>,
    parent:   u16,
}
struct LNode {
    keys:   Vec<u64>,
    values: Vec<u64>,
    parent: u16,
}
enum BNode {
    Internal(INode),
    Leaf(LNode),
}

impl BNode {
    fn internal() -> Self {
        BNode::Internal(INode { keys: Vec::new(), children: Vec::new(), parent: 0 })
    }
    fn leaf(pairs: &[(u64, u64)]) -> Self {
        let mut n = LNode { keys: Vec::new(), values: Vec::new(), parent: 0 };
        for (k, v) in pairs {
            n.keys.push(*k);
            n.values.push(*v);
        }
        BNode::Leaf(n)
    }
    fn parent_field(&self) -> u16 {
        match self {
            BNode::Internal(n) => n.parent,
            BNode::Leaf(n) => n.parent,
        }
    }
    fn set_parent_field(&mut self, p: u16) {
        match self {
            BNode::Internal(n) => n.parent = p,
            BNode::Leaf(n) => n.parent = p,
        }
    }
}

impl Node for BNode {
    type K = u64;
    type V = u64;
    type P = u16;
    const DEGREE: usize = DEGREE;
    const STORES_PARENTS: bool = true;
    type Payload = (); //B+ separators are child mins — nothing promotes besides them
}

impl SplittableNode for BNode {
    fn new_root(r_v: u16) -> Self {
        let mut n = Self::internal();
        match &mut n {
            BNode::Internal(i) => i.children.push(r_v),
            _ => unreachable!(),
        }
        n
    }

    ///leaf: right half's min copied up; internal: boundary separator moved up.
    ///Y's parent field is 0 — `adopt_node` sets it at every split site.
    fn split(&mut self, slot: &mut MaybeUninit<Self>) -> (u64, ()) {
        match self {
            BNode::Leaf(n) => {
                let mut r = LNode {
                    keys:   n.keys.split_off(n.keys.len() >> 1),
                    values: Vec::new(),
                    parent: 0,
                };
                r.values = n.values.split_off(n.values.len() - r.keys.len());
                let sep = r.keys[0];
                slot.write(BNode::Leaf(r));
                (sep, ())
            }
            BNode::Internal(n) => {
                let mid = n.children.len() >> 1;
                let rchildren = n.children.split_off(mid);
                //boundary key keys[mid-1] moves up; Y keeps keys[mid..] (its own
                //child separators); X keeps [0..mid-1) — arity preserved both sides
                let mut rkeys = n.keys.split_off(mid - 1);
                let sep = rkeys.remove(0);
                slot.write(BNode::Internal(INode {
                    keys:     rkeys,
                    children: rchildren,
                    parent:   0,
                }));
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
    fn fixup<F: crate::metadata::Fixup + ?Sized>(
        &mut self,
        f: &F,
        _tr: &crate::translator::Translator<u16>,
    ) {
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

type BlockT<'block, O> = UniformBlock<'block, BNode, u16, BTreeMeta, O>;

fn child_min<O: Ordering>(b: &BlockT<'_, O>, mut c: u16) -> u64 {
    loop {
        match b.vget(c) {
            BNode::Leaf(n) => return n.keys[0],
            BNode::Internal(n) => c = n.children[0],
        }
    }
}

// ---------------------------------------------------------------------------
// cursor read surface — shared by the shared/mut cursors via free fns
// ---------------------------------------------------------------------------

fn c_is_leaf(b: &BlockT<'_, impl Ordering>, s: &PosAncestry) -> bool {
    s.ancestry.len() == b.data().height as usize
}

fn c_child_count(b: &BlockT<'_, impl Ordering>, s: &PosAncestry) -> usize {
    match b.get(s.pos) {
        BNode::Internal(n) => n.children.len(),
        BNode::Leaf(_) => 0,
    }
}

fn c_child(b: &BlockT<'_, impl Ordering>, s: &PosAncestry, idx: usize) -> u16 {
    match b.get(s.pos) {
        BNode::Internal(n) => n.children[idx],
        BNode::Leaf(_) => panic!("child: leaf"),
    }
}

///B+ routing: `(pos, cmp)` per the btree example's convention (separators bound
///children 1.., child 0 unbounded below, equal-right, append = Greater).
fn c_lookup(b: &BlockT<'_, impl Ordering>, s: &PosAncestry, k: &u64) -> (usize, Cmp) {
    match b.get(s.pos) {
        BNode::Leaf(n) => match n.keys.iter().position(|&key| key >= *k) {
            Some(i) if n.keys[i] == *k => (i, Cmp::Equal),
            Some(i) => (i, Cmp::Less),
            None if n.keys.is_empty() => (0, Cmp::Less),
            None => (n.keys.len() - 1, Cmp::Greater),
        },
        BNode::Internal(n) => {
            let cc = n.children.len();
            if cc == 0 {
                return (0, Cmp::Less);
            }
            let p = n.keys.iter().position(|&key| key > *k).unwrap_or(n.keys.len());
            if p == 0 {
                if *k < child_min(b, n.children[0]) { (0, Cmp::Less) } else { (0, Cmp::Equal) }
            } else if p == n.keys.len() && *k > n.keys[n.keys.len() - 1] {
                (cc - 1, Cmp::Greater)
            } else {
                (p, Cmp::Equal)
            }
        }
    }
}

fn c_search<O: Ordering>(b: &BlockT<'_, O>, s: &mut PosAncestry, k: &u64) -> Option<usize> {
    if b.occupied() == 0 {
        return None;
    }
    while !c_is_leaf(b, s) {
        let (i, _) = c_lookup(b, s, k);
        let child = c_child(b, s, i);
        let phys = b.v2p(child);
        s.ancestry.push(s.pos, i);
        s.pos = phys;
    }
    Some(s.pos)
}

fn root_state(b: &BlockT<'_, impl Ordering>) -> PosAncestry {
    PosAncestry { pos: b.data().root(), ancestry: Default::default() }
}

struct Cursor<'block, 'walker, O: Ordering> {
    b:     &'walker BlockT<'block, O>,
    state: PosAncestry,
}

impl<'block, 'a, O: Ordering> From<&'a BlockT<'block, O>> for Cursor<'block, 'a, O>
where 'block: 'a
{
    fn from(b: &'a BlockT<'block, O>) -> Self {
        Self { b, state: root_state(b) }
    }
}

impl<'block, 'walker, O: Ordering> NodeCursor<'block, BlockT<'block, O>>
    for Cursor<'block, 'walker, O>
{
    type State = PosAncestry;

    fn state(&self) -> &PosAncestry {
        &self.state
    }
    fn state_mut(&mut self) -> &mut PosAncestry {
        &mut self.state
    }
    fn block(&self) -> &BlockT<'block, O> {
        self.b
    }
    fn is_leaf(&self) -> bool {
        c_is_leaf(self.b, &self.state)
    }
    fn child_count(&self) -> usize {
        c_child_count(self.b, &self.state)
    }
    fn child(&self, idx: usize) -> u16 {
        c_child(self.b, &self.state, idx)
    }
    fn lookup(&self, k: &u64) -> (usize, Cmp) {
        c_lookup(self.b, &self.state, k)
    }
    fn search(&mut self, k: &u64) -> Option<&BNode> {
        let p = c_search(self.b, &mut self.state, k)?;
        Some(self.b.get(p))
    }
}

impl<'block, 'walker, O: Ordering> NodeWalker<'block, BlockT<'block, O>>
    for Cursor<'block, 'walker, O>
{
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

struct CursorMut<'block, 'walker, O: Ordering> {
    b:     &'walker mut BlockT<'block, O>,
    state: PosAncestry,
}

impl<'block, 'a, O: Ordering> From<&'a mut BlockT<'block, O>> for CursorMut<'block, 'a, O>
where 'block: 'a
{
    fn from(b: &'a mut BlockT<'block, O>) -> Self {
        let state = root_state(b);
        Self { b, state }
    }
}

impl<'block, 'walker, O: Ordering> NodeCursor<'block, BlockT<'block, O>>
    for CursorMut<'block, 'walker, O>
{
    type State = PosAncestry;

    fn state(&self) -> &PosAncestry {
        &self.state
    }
    fn state_mut(&mut self) -> &mut PosAncestry {
        &mut self.state
    }
    fn block(&self) -> &BlockT<'block, O> {
        self.b
    }
    fn is_leaf(&self) -> bool {
        c_is_leaf(self.b, &self.state)
    }
    fn child_count(&self) -> usize {
        c_child_count(self.b, &self.state)
    }
    fn child(&self, idx: usize) -> u16 {
        c_child(self.b, &self.state, idx)
    }
    fn lookup(&self, k: &u64) -> (usize, Cmp) {
        c_lookup(self.b, &self.state, k)
    }
    fn search(&mut self, k: &u64) -> Option<&BNode> {
        let p = c_search(self.b, &mut self.state, k)?;
        Some(self.b.get(p))
    }
}

impl<'block, 'walker, O: Ordering> NodeWalker<'block, BlockT<'block, O>>
    for CursorMut<'block, 'walker, O>
{
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

impl<'block, 'walker, O: Ordering> NodeWalkerMut<'block, BlockT<'block, O>>
    for CursorMut<'block, 'walker, O>
{
    fn parts(&mut self) -> (&mut PosAncestry, &BlockT<'block, O>) {
        (&mut self.state, &*self.b)
    }
    fn parts_mut(&mut self) -> (&mut PosAncestry, &mut BlockT<'block, O>) {
        (&mut self.state, &mut *self.b)
    }
    fn block_mut(&mut self) -> &mut BlockT<'block, O> {
        self.b
    }
    fn has_space(&self) -> bool {
        match self.b.get(self.state.pos) {
            BNode::Internal(n) => n.children.len() < DEGREE,
            BNode::Leaf(n) => n.keys.len() < DEGREE,
        }
    }
    fn set_child(&mut self, up: usize, child_idx: usize, ptr: u16) {
        let target = match up {
            0 => self.state.pos,
            n => self.state.ancestry.stack[self.state.ancestry.len() - n].parent,
        };
        match self.b.get_mut(target) {
            BNode::Internal(n) => n.children[child_idx] = ptr,
            BNode::Leaf(_) => panic!("set_child: leaf"),
        }
    }
    fn set_parent(&mut self, ptr: u16) {
        self.b.get_mut(self.state.pos).set_parent_field(ptr);
    }
    ///separator re-derived from the placed child's own min (B+ child-min separators);
    ///a new leftmost inserts the OLD leftmost's min. the new node's own parent field
    ///is the crate's (`adopt_node`) — this only wires the entry.
    fn insert_child(&mut self, child_idx: usize, _k: &u64, _payload: (), ptr: u16) {
        let m = child_min(self.b, ptr);
        let old_left = match self.b.get(self.state.pos) {
            BNode::Internal(n) if child_idx == 0 && !n.children.is_empty() => {
                Some(child_min(self.b, n.children[0]))
            }
            _ => None,
        };
        match self.b.get_mut(self.state.pos) {
            BNode::Internal(n) => {
                n.children.insert(child_idx, ptr);
                if child_idx == 0 {
                    if let Some(sep) = old_left {
                        n.keys.insert(0, sep);
                    }
                } else {
                    n.keys.insert(child_idx - 1, m);
                }
            }
            _ => panic!("insert_child: child into non-internal"),
        }
    }
    fn remove_child(&mut self, child_idx: usize) -> (Option<u64>, Option<()>, u16) {
        match self.b.get_mut(self.state.pos) {
            BNode::Internal(n) => {
                let p = n.children.remove(child_idx);
                let sep = (child_idx > 0).then(|| n.keys[child_idx - 1]);
                if child_idx > 0 {
                    n.keys.remove(child_idx - 1);
                }
                (sep, None, p)
            }
            _ => panic!("remove_child: not internal"),
        }
    }
}

// ---------------------------------------------------------------------------
// the map
// ---------------------------------------------------------------------------

struct BTreeMap<O: Ordering> {
    block: BlockT<'static, O>,
    len:   usize,
}

impl<O: Ordering> BTreeMap<O> {
    fn new() -> Self {
        let mut block = BlockT::new();
        let root = block.insert_root(BNode::leaf(&[]));
        block.set_data(BTreeMeta { root, height: 0 });
        Self { block, len: 0 }
    }

    fn get(&self, k: &u64) -> Option<u64> {
        let w: TreeWalker<O, Cursor<'static, '_, O>> = search(&self.block, k);
        match w.nw.current() {
            BNode::Leaf(n) => n.keys.iter().position(|&key| key == *k).map(|i| n.values[i]),
            _ => None,
        }
    }

    ///split-driven insert (as the btree example: retry from the top after a split).
    fn insert(&mut self, k: u64, v: u64) -> Result<(), InsertErr>
    where for<'w> TreeWalker<O, CursorMut<'static, 'w, O>>:
            TreeWalk<'static, CursorMut<'static, 'w, O>, BlockT<'static, O>> {
        enum Step {
            Done,
            Placed,
            Full,
        }
        loop {
            let mut w: TreeWalker<O, CursorMut<'static, '_, O>> = search(&mut self.block, &k);
            let step = match w.nw.current_mut() {
                BNode::Leaf(n) if n.keys.contains(&k) => {
                    let pos = n.keys.iter().position(|&key| key == k).unwrap();
                    n.values[pos] = v;
                    Step::Done
                }
                BNode::Leaf(n) if n.keys.len() < DEGREE => {
                    let pos = n.keys.iter().position(|&key| k < key).unwrap_or(n.keys.len());
                    n.keys.insert(pos, k);
                    n.values.insert(pos, v);
                    Step::Placed
                }
                _ => Step::Full,
            };
            match step {
                Step::Done => return Ok(()),
                Step::Placed => {
                    self.len += 1;
                    return Ok(());
                }
                Step::Full => {}
            }
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
}

// ---------------------------------------------------------------------------
// validation
// ---------------------------------------------------------------------------

///reference DFS in the block's ordering — the walk must match it exactly.
fn ref_seq<O: Ordering>(b: &BlockT<'static, O>) -> Vec<u16> {
    let mut out = Vec::new();
    rec(b, b.p2v(b.data().root()), &mut out);
    return out;

    fn rec<O: Ordering>(b: &BlockT<'static, O>, v: u16, out: &mut Vec<u16>) {
        let kids: Vec<u16> = match b.vget(v) {
            BNode::Internal(n) => n.children.clone(),
            BNode::Leaf(_) => Vec::new(),
        };
        match O::ORDER {
            Order::Pre => {
                out.push(v);
                for c in kids {
                    rec(b, c, out);
                }
            }
            Order::In => {
                let bnd = kids.len().min(DEGREE / 2);
                for c in &kids[..bnd] {
                    rec(b, *c, out);
                }
                out.push(v);
                for c in &kids[bnd..] {
                    rec(b, *c, out);
                }
            }
            Order::Post => {
                for c in kids {
                    rec(b, c, out);
                }
                out.push(v);
            }
        }
    }
}

///structural DFS: stored parent fields name the true parent (the STORES_PARENTS
///contract — the reparent matrix's output), separators == child mins, leaf keys
///sorted, and the reachable count (== occupied — no node lost to a fixup).
fn check_tree<O: Ordering>(b: &BlockT<'static, O>) -> usize {
    fn rec<O: Ordering>(
        b: &BlockT<'static, O>,
        v: u16,
        parent: Option<u16>,
        leaf_keys: &mut Vec<u64>,
    ) -> usize {
        let node = b.vget(v);
        if let Some(p) = parent {
            assert_eq!(node.parent_field(), p, "node vaddr {v}: stale parent field");
        }
        match node {
            BNode::Leaf(n) => {
                assert!(n.keys.windows(2).all(|w| w[0] < w[1]), "leaf unsorted");
                leaf_keys.extend_from_slice(&n.keys);
                1
            }
            BNode::Internal(n) => {
                assert_eq!(n.keys.len() + 1, n.children.len(), "key/child arity");
                for i in 0..n.keys.len() {
                    assert_eq!(
                        n.keys[i],
                        child_min(b, n.children[i + 1]),
                        "separator != child min"
                    );
                }
                let mut c = 1;
                for k in &n.children {
                    c += rec(b, *k, Some(v), leaf_keys);
                }
                c
            }
        }
    }
    let mut leaf_keys = Vec::new();
    let count = rec(b, b.p2v(b.data().root()), None, &mut leaf_keys);
    assert!(leaf_keys.windows(2).all(|w| w[0] < w[1]), "global leaf key order broken");
    count
}

///`TreeWalk::first`+`next` / `last`+`prev` must visit every node exactly once, in
///the reference DFS order, at strictly increasing phys (the layout IS the ordering).
fn check_walk<O: Ordering>(b: &BlockT<'static, O>)
where for<'w> TreeWalker<O, Cursor<'static, 'w, O>>:
        TreeWalk<'static, Cursor<'static, 'w, O>, BlockT<'static, O>> {
    let expect = ref_seq(b);
    let mut w = TreeWalker::new(Cursor::from(b));
    let mut got = Vec::new();
    let mut phys = Vec::new();
    if w.first().is_some() {
        loop {
            got.push(b.p2v(w.nw.state().pos));
            phys.push(w.nw.state().pos);
            if w.next().is_none() {
                break;
            }
        }
    }
    assert_eq!(got, expect, "next() diverged from reference DFS");
    assert!(phys.windows(2).all(|p| p[0] < p[1]), "walk order != slot order: {phys:?}");

    let mut w = TreeWalker::new(Cursor::from(b));
    let mut rev = Vec::new();
    if w.last().is_some() {
        loop {
            rev.push(b.p2v(w.nw.state().pos));
            if w.prev().is_none() {
                break;
            }
        }
    }
    rev.reverse();
    assert_eq!(rev, expect, "prev() diverged from reference DFS");
}

fn block_get<O: Ordering>(b: &BlockT<'_, O>, k: &u64) -> Option<u64> {
    let w: TreeWalker<O, Cursor<'_, '_, O>> = search(b, k);
    match w.nw.current() {
        BNode::Leaf(n) => n.keys.iter().position(|&key| key == *k).map(|i| n.values[i]),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// the tortures
// ---------------------------------------------------------------------------

///~240 keys through the split driver: ascending, descending, and a coprime-stride
///scatter — appends, leftmost inserts (the in-order hop trigger), and mid gaps.
fn map_torture<O: Ordering>()
where
    for<'w> TreeWalker<O, Cursor<'static, 'w, O>>:
        TreeWalk<'static, Cursor<'static, 'w, O>, BlockT<'static, O>>,
    for<'w> TreeWalker<O, CursorMut<'static, 'w, O>>:
        TreeWalk<'static, CursorMut<'static, 'w, O>, BlockT<'static, O>>,
{
    let mut m = BTreeMap::<O>::new();
    let mut keys: Vec<u64> = Vec::new();
    keys.extend((0..60u64).map(|i| i * 7 + 1)); //ascending: 1, 8, ..., 414
    keys.extend((0..60u64).rev().map(|i| 500 + i * 3)); //descending: 677..500
    keys.extend((0..120u64).map(|i| 1000 + (i * 37) % 500)); //37 coprime 500: distinct
    assert_eq!(keys.len(), 240);

    for &k in &keys {
        m.insert(k, k * 10 + 1).unwrap();
    }
    assert!(
        m.block.data().height >= 2,
        "no multi-level splits: height={}",
        m.block.data().height
    );
    assert_eq!(m.len, 240);

    assert_eq!(check_tree(&m.block), m.block.occupied(), "reachable node count != occupied");
    check_walk(&m.block);
    for &k in &keys {
        assert_eq!(m.get(&k), Some(k * 10 + 1), "get({k})");
    }
    for k in [2u64, 4, 421, 999, 2000] {
        assert_eq!(m.get(&k), None, "phantom get({k})");
    }

    //overwrites don't grow
    let len = m.len;
    m.insert(keys[0], 12345).unwrap();
    assert_eq!(m.len, len);
    assert_eq!(m.get(&keys[0]), Some(12345));
}

///hand-assembled two-level tree via `TreeWalkMut::insert_child` (slides + wiring +
///per-ordering placement, no split driver): leftmost, mid-gap, and append anchors.
fn hand_assembled<O: Ordering>()
where
    for<'w> TreeWalker<O, Cursor<'static, 'w, O>>:
        TreeWalk<'static, Cursor<'static, 'w, O>, BlockT<'static, O>>,
    for<'w> TreeWalker<O, CursorMut<'static, 'w, O>>:
        TreeWalk<'static, CursorMut<'static, 'w, O>, BlockT<'static, O>>,
{
    let mut block = BlockT::<O>::new();
    let root = block.insert_root(BNode::internal());
    block.set_data(BTreeMeta { root, height: 1 });
    {
        let mut w = TreeWalker::new(CursorMut::from(&mut block));
        w.insert_child(&40, (), BNode::leaf(&[(40, 400), (42, 421)])).unwrap();
        w.insert_child(&20, (), BNode::leaf(&[(20, 201)])).unwrap();
        w.insert_child(&35, (), BNode::leaf(&[(35, 351), (37, 372)])).unwrap();
        w.insert_child(&30, (), BNode::leaf(&[(30, 303), (33, 331)])).unwrap();
    }
    assert_eq!(check_tree(&block), block.occupied());
    check_walk(&block);
    for (k, v) in
        [(20u64, 201), (30, 303), (33, 331), (35, 351), (37, 372), (40, 400), (42, 421)]
    {
        assert_eq!(block_get(&block, &k), Some(v), "get({k})");
    }
    assert_eq!(block_get(&block, &22), None);
    assert_eq!(block_get(&block, &28), None);
    assert_eq!(block_get(&block, &50), None);
}

#[test]
fn pre_map() {
    map_torture::<PreOrder>();
}

#[test]
fn in_map() {
    map_torture::<InOrder>();
}

#[test]
fn post_map() {
    map_torture::<PostOrder>();
}

#[test]
fn pre_hand_assembled() {
    hand_assembled::<PreOrder>();
}

#[test]
fn in_hand_assembled() {
    hand_assembled::<InOrder>();
}

#[test]
fn post_hand_assembled() {
    hand_assembled::<PostOrder>();
}
