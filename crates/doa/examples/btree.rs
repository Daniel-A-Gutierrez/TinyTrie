//! B+ tree consumer over doa's open surface.
//! u16 pointers, `Uniform` block, `PreOrder` layout.
//!
//! All wiring is consumer-side: splits are preemptive (a full node on the
//! descent path splits before anything descends into it), driven with
//! `open_child`/`open_parent`/`open_parent_child` + raw block inserts and the
//! crate-returned fixups. Held addresses never survive an open uncorrected.

use arrays::tiny_array::TinyArray;
use doa::Fixup;
use doa::PreOrder;
use doa::Rel;
use doa::blocks::{BlockTrait, UniformBlock};
use doa::metadata::{Ancestry, ChildPos, DoubleSlide, Fixable, GrewFixup, HasRoot, Pos, PosAncestry,
                    SwapFixup};
use doa::store::NoneSlide;
use doa::translator::Translator;
use doa::treeblock::{search, walker};
use doa::walker::{BlockExhausted, Node, NodeCursor, NodeWalker, NodeWalkerMut, OpenFixups,
                  PreOrderWalk, TreeWalk, TreeWalker};

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

    ///fresh internal node pre-wired with its first child = `child0` (the root
    /// promotion / burst shape — the one wire with no separator).
    fn new_parent(child0: u16) -> Self {
        let mut children = TinyArray::new();
        children.push(child0);
        BNode::Internal(INode { keys: TinyArray::new(), children })
    }

    ///drain the right half out (returned); self keeps the left half. leaf: the
    /// right half's min copies up later (leaves keep all keys); internal: the
    /// boundary separator moves with the drain.
    fn split(&mut self) -> Self {
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
                BNode::Leaf(r)
            }
            BNode::Internal(n) => {
                let cc = n.children.len();
                let mid = cc >> 1;
                let mut r = INode { keys: TinyArray::new(), children: TinyArray::new() };
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
                BNode::Internal(r)
            }
        }
    }
}

impl Node for BNode {
    type K = u64;
    type V = u64;
    type A = u16;
    const DEGREE: usize = DEGREE;
    const STORES_PARENTS: bool = false; //nodes carry no parent fields
}

///root position + tree height (B+ leaves sit at depth == height).
#[derive(Clone, Copy, Debug, Default)]
struct BTreeMeta {
    root:   Pos,
    height: u32,
}

impl Fixable<u16> for BTreeMeta {
    fn grew_fix(&mut self, fix: GrewFixup, _tr: &Translator<u16>) {
        fix.fix_pos(&mut self.root);
    }
    fn swap_fix(&mut self, fix: SwapFixup, _tr: &Translator<u16>) {
        if fix.affects_pos(self.root) {
            fix.fix_pos(&mut self.root);
        }
    }
    fn slide_fix(&mut self, fix: NoneSlide, _tr: &Translator<u16>) {
        if fix.affects_pos(self.root) {
            fix.fix_pos(&mut self.root);
        }
    }
    fn two_slide(&mut self, fix: DoubleSlide, _tr: &Translator<u16>) {
        if fix.affects_pos(self.root) {
            fix.fix_pos(&mut self.root);
        }
    }
}

impl HasRoot<u16> for BTreeMeta {
    fn root(&self) -> Pos {
        self.root
    }
    fn set_root(&mut self, root: Pos) {
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

///min key of the subtree rooted at addr `c` — its leftmost leaf's first key.
fn child_min(b: &BlockT<'_>, mut c: u16) -> u64 {
    loop {
        match b.aget(c) {
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

fn node_full(b: &BlockT<'_>, pos: Pos) -> bool {
    match b.get(pos) {
        BNode::Internal(n) => n.children.is_full(),
        BNode::Leaf(n) => n.keys.is_full(),
    }
}

///apply an open's fixups to a held addr — an open's slides/grow rename whatever
/// moved, so a held addr is only valid post-fixup.
fn fix_addr(block: &BlockT<'_>, fixups: &OpenFixups, a: &mut u16) {
    //addrs are grow-stable by construction — only the slides remap them
    let tr = block.translator();
    if fixups.slides.affects_addr(*a, tr) {
        fixups.slides.fix_addr(a, tr);
    }
}

///B+ wire: insert `addr` as child `child_idx` of the node at `par`. separators
///re-derive from child mins — `keys[i] = min(children[i+1])` holds afterward
///(a new leftmost takes the OLD leftmost's min as its separator).
fn wire_child(block: &mut BlockT<'_>, par: Pos, child_idx: ChildPos, addr: u16) {
    let m = child_min(block, addr);
    let old_left = {
        let BNode::Internal(n) = block.get(par) else { panic!("wire_child: not internal") };
        (child_idx == 0 && n.children.len() > 0)
            .then(|| child_min(block, *n.children.get(0)))
    };
    let BNode::Internal(n) = block.get_mut(par) else { panic!("wire_child: not internal") };
    n.children.insert_at(child_idx.0, addr);
    if child_idx == 0 {
        if let Some(sep) = old_left {
            n.keys.insert_at(0, sep);
        }
    } else {
        n.keys.insert_at(child_idx.0 - 1, m);
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
    fn block(&self) -> &BlockT<'block> {
        self.b
    }
    fn position(&self) -> Pos {
        self.state.pos
    }
    fn is_root(&self) -> bool {
        self.state.ancestry.is_empty()
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
    fn child(&self, idx: ChildPos) -> u16 {
        match self.current() {
            BNode::Internal(n) => *n.children.get(idx.0),
            BNode::Leaf(_) => panic!("child: leaf"),
        }
    }
    fn children(&self) -> impl DoubleEndedIterator<Item = (ChildPos, u16)>
                             + ExactSizeIterator
                             + '_ {
        let cc = self.child_count();
        (0..cc).map(|i| (ChildPos(i), self.child(ChildPos(i))))
    }
    ///descent slot for `k`; `None` at a leaf (descent terminates). B+ separators
    /// bound children 1.. and are equal-right, so the slot is the first
    /// separator greater than k, clamped to the last child.
    fn lookup(&self, k: &u64) -> Option<ChildPos> {
        match self.current() {
            BNode::Leaf(_) => None,
            BNode::Internal(n) => {
                let keys = n.keys.as_slice();
                let cc = n.children.len();
                let p = keys.iter().position(|&key| key > *k).unwrap_or(keys.len());
                Some(ChildPos(p.min(cc.saturating_sub(1))))
            }
        }
    }
    fn descend<'b>(&'b mut self, child: ChildPos) -> &'b BNode
    where 'block: 'b {
        let addr = self.child(child);
        let pos = self.b.a2p(addr);
        let parent = self.state.pos;
        self.state.ancestry.push(parent, child);
        self.state.pos = pos;
        self.b.get(pos)
    }
}

impl<'block, 'walker> NodeWalker<'block, BlockT<'block>> for Cursor<'block, 'walker> {
    fn ascend<'b>(&'b mut self) -> (&'b BNode, ChildPos)
    where 'block: 'b {
        let a = self.state.ancestry.pop().expect("ascend: at root");
        self.state.pos = a.parent;
        (self.b.get(a.parent), a.child)
    }
    fn parent(&self) -> Option<(Pos, ChildPos)> {
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
    fn block(&self) -> &BlockT<'block> {
        self.b
    }
    fn position(&self) -> Pos {
        self.state.pos
    }
    fn is_root(&self) -> bool {
        self.state.ancestry.is_empty()
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
    fn child(&self, idx: ChildPos) -> u16 {
        match self.current() {
            BNode::Internal(n) => *n.children.get(idx.0),
            BNode::Leaf(_) => panic!("child: leaf"),
        }
    }
    fn children(&self) -> impl DoubleEndedIterator<Item = (ChildPos, u16)>
                             + ExactSizeIterator
                             + '_ {
        let cc = self.child_count();
        (0..cc).map(|i| (ChildPos(i), self.child(ChildPos(i))))
    }
    ///descent slot for `k`; `None` at a leaf (descent terminates).
    fn lookup(&self, k: &u64) -> Option<ChildPos> {
        match self.current() {
            BNode::Leaf(_) => None,
            BNode::Internal(n) => {
                let keys = n.keys.as_slice();
                let cc = n.children.len();
                let p = keys.iter().position(|&key| key > *k).unwrap_or(keys.len());
                Some(ChildPos(p.min(cc.saturating_sub(1))))
            }
        }
    }
    fn descend<'b>(&'b mut self, child: ChildPos) -> &'b BNode
    where 'block: 'b {
        let addr = self.child(child);
        let pos = self.b.a2p(addr);
        let parent = self.state.pos;
        self.state.ancestry.push(parent, child);
        self.state.pos = pos;
        self.b.get(pos)
    }
}

impl<'block, 'walker> NodeWalker<'block, BlockT<'block>> for CursorMut<'block, 'walker> {
    fn ascend<'b>(&'b mut self) -> (&'b BNode, ChildPos)
    where 'block: 'b {
        let a = self.state.ancestry.pop().expect("ascend: at root");
        self.state.pos = a.parent;
        (self.b.get(a.parent), a.child)
    }
    fn parent(&self) -> Option<(Pos, ChildPos)> {
        self.state.ancestry.last().map(|a| (a.parent, a.child))
    }
}

impl<'block, 'walker> Fixable<u16> for CursorMut<'block, 'walker> {
    fn grew_fix(&mut self, fix: GrewFixup, tr: &Translator<u16>) {
        self.state.grew_fix(fix, tr);
    }
    fn swap_fix(&mut self, fix: SwapFixup, tr: &Translator<u16>) {
        self.state.swap_fix(fix, tr);
    }
    fn slide_fix(&mut self, fix: NoneSlide, tr: &Translator<u16>) {
        self.state.slide_fix(fix, tr);
    }
    fn two_slide(&mut self, fix: DoubleSlide, tr: &Translator<u16>) {
        self.state.two_slide(fix, tr);
    }
}

impl<'block, 'walker> NodeWalkerMut<'block, BlockT<'block>> for CursorMut<'block, 'walker> {
    type Snapshot = PosAncestry;

    fn save(&self) -> Self::Snapshot {
        self.state.clone()
    }
    fn load(&mut self, snap: Self::Snapshot) {
        self.state = snap;
    }
    fn set_position(&mut self, pos: Pos) {
        self.state.pos = pos;
    }

    fn block_mut(&mut self) -> &mut BlockT<'block> {
        self.b
    }
    fn set_child(&mut self, up: usize, child: ChildPos, addr: u16) {
        let target = match up {
            0 => self.state.pos,
            n => self.state.ancestry.stack[self.state.ancestry.len() - n].parent,
        };
        match self.block_mut().get_mut(target) {
            BNode::Internal(n) => *n.children.get_mut(child.0) = addr,
            BNode::Leaf(_) => panic!("set_child: leaf"),
        }
    }
    fn clear_child(&mut self, _up: usize, _child: ChildPos) {
        panic!("clear_child: binary rotations only");
    }
    fn set_parent(&mut self, _addr: u16) {} //nodes store no parent fields
}

// ---------------------------------------------------------------------------
// the map — consumer-driven splits
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

    ///preemptive descent: a full node on k's path splits before anything
    /// descends into it (its parent had room — checked before descending). a
    /// split shifts indices ⇒ restart. the split flows are inlined: the opens
    /// consume the walker, releasing the block borrow for the drain/wire phase.
    pub fn insert(&mut self, k: u64, v: u64) -> Result<(), BlockExhausted> {
        loop {
            let mut w: TreeWalker<PreOrder, CursorMut<'_, '_>> = walker(&mut self.block);
            let mut restart = false;
            loop {
                if node_full(w.nw.block(), w.nw.position()) {
                    if w.nw.is_root() {
                        //split the root: a fresh parent above it + Y (the
                        //drained half). preorder: NR before R, Y before R's
                        //mid-child's subtree (internal) / right after R (childless)
                        let mut r_a = w.nw.block().p2a(w.nw.position()); //held across the opens
                        let cc = w.nw.child_count();
                        if cc == 0 {
                            //childless (leaf) root: sequential — NR before R, then
                            //Y right after R; each step leaves a walkable tree
                            let (nr_open, fixups) = w.open_parent()?;
                            fix_addr(&self.block, &fixups, &mut r_a);
                            self.block.insert(nr_open, BNode::new_parent(r_a));
                            //NR held as an ADDR: the second open below can slide or
                            //spread, and `nr_open`'s pos would go stale (§2 addendum)
                            let mut nr_a = self.block.p2a(nr_open.0);
                            self.block.data_mut().set_root(nr_open.0);
                            let h = self.block.data().height() + 1;
                            self.block.data_mut().set_height(h);
                            //Y after R — a fresh walker routes through NR down to R
                            let mut w2: TreeWalker<PreOrder, CursorMut<'_, '_>> =
                                walker(&mut self.block);
                            w2.nw.descend(ChildPos(0)); //walker: -> R
                            let (y_open, fixups) = w2.open_here(Rel::After)?;
                            fix_addr(&self.block, &fixups, &mut r_a);
                            fix_addr(&self.block, &fixups, &mut nr_a);
                            let y = self.block.get_mut(self.block.a2p(r_a)).split();
                            self.block.insert(y_open, y);
                            let y_a = self.block.p2a(y_open.0);
                            let y_min = child_min(&self.block, y_a);
                            let BNode::Internal(n) =
                                self.block.get_mut(self.block.a2p(nr_a)) else {
                                panic!("insert: fresh parent is not internal")
                            };
                            n.children.push(y_a);
                            n.keys.push(y_min);
                        } else {
                            //internal root: one atomic two-anchor pass — NR before
                            //R + Y before R's mid-child's subtree, both slides
                            //computed before either moves
                            let mid = cc >> 1;
                            let ((nr_open, y_open), fixups) =
                                w.open_parent_child(ChildPos(mid), Rel::Before)?;
                            fix_addr(&self.block, &fixups, &mut r_a);
                            let y = self.block.get_mut(self.block.a2p(r_a)).split();
                            self.block.insert(y_open, y);
                            self.block.insert(nr_open, BNode::new_parent(r_a));
                            let y_a = self.block.p2a(y_open.0);
                            let y_min = child_min(&self.block, y_a);
                            let BNode::Internal(n) = self.block.get_mut(nr_open.0) else {
                                panic!("insert: fresh parent is not internal")
                            };
                            n.children.push(y_a);
                            n.keys.push(y_min);
                            self.block.data_mut().set_root(nr_open.0);
                            let h = self.block.data().height() + 1;
                            self.block.data_mut().set_height(h);
                        }
                    } else {
                        //split child idx of the parent (preorder: Y lands before
                        //X's mid-child's subtree — inside X's old span, so Y's
                        //children visit after it)
                        let (_, idx) = w.nw.ascend(); //walker: -> the parent (has room)
                        let mut x_a = w.nw.child(idx); //held across the open
                        let mut par_a = w.nw.block().p2a(w.nw.position());
                        w.nw.descend(idx); //walker: -> X
                        let cc = w.nw.child_count();
                        let (open, fixups) = if cc == 0 {
                            w.open_here(Rel::After)? //childless X: Y right after it
                        } else {
                            w.open_child(ChildPos(cc >> 1), Rel::Before)? //Y before X's child[mid]
                        };
                        fix_addr(&self.block, &fixups, &mut x_a);
                        fix_addr(&self.block, &fixups, &mut par_a);
                        let y = self.block.get_mut(self.block.a2p(x_a)).split();
                        self.block.insert(open, y);
                        let y_a = self.block.p2a(open.0);
                        let par_pos = self.block.a2p(par_a);
                        wire_child(&mut self.block, par_pos, idx + 1, y_a);
                    }
                    restart = true;
                    break;
                }
                if w.nw.is_leaf() {
                    //guaranteed non-full (checked above): overwrite or place
                    let pos = w.nw.position();
                    let BNode::Leaf(n) = w.nw.block_mut().get_mut(pos) else {
                        panic!("insert: not a leaf")
                    };
                    if let Some(p) = n.keys.as_slice().iter().position(|&key| key == k) {
                        *n.values.get_mut(p) = v; //overwrite: no len change
                    } else {
                        let at = n
                            .keys
                            .as_slice()
                            .iter()
                            .position(|&key| k < key)
                            .unwrap_or(n.keys.len());
                        n.keys.insert_at(at, k);
                        n.values.insert_at(at, v);
                        self.len += 1;
                    }
                    break;
                }
                let c = w.nw.lookup(&k).expect("insert: internal node routes");
                w.nw.descend(c); //walker: -> the routed child
            }
            if !restart {
                return Ok(());
            }
        }
    }

    pub fn remove(&mut self, k: &u64) -> Option<u64> {
        let v = {
            let mut w: TreeWalker<PreOrder, CursorMut<'_, '_>> = search(&mut self.block, k);
            let pos = w.nw.position();
            let BNode::Leaf(n) = w.nw.block_mut().get_mut(pos) else { return None };
            let at = n.keys.as_slice().iter().position(|&key| key == *k)?;
            let v = n.values.remove(at);
            n.keys.remove(at);
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
        "map demo: ok (100 keys, consumer-driven splits + multi root promotions, get/overwrite/remove/iter)"
    );
}

///hand-assembled two-level tree: root inode + five leaves placed via
///`insert_leaf` (open + block insert + wire, preorder anchors — no split
///flow). 40/30/25/20 are gap-0 inserts (parent-adjacent anchor); 35 is a
///mid-gap insert (descend + subtree-edge anchor); the final 20 insert lands
///on the out-of-run slide case — the None sits two slots right of the anchor,
///so the fixup walk runs from below the anchor and restores its state instead
///of walking back through the pointers it just rewrote.
fn insert_leaf(block: &mut BlockT<'_>, k: u64, leaf: BNode) -> Result<(), BlockExhausted> {
    let w: TreeWalker<PreOrder, CursorMut<'_, '_>> = walker(&mut *block);
    //gap by key: before the first child whose min exceeds k, else append.
    //collect addrs first; the min-scan reads the block through the walker.
    let cc = w.nw.child_count();
    let addrs: Vec<u16> = (0..cc).map(|i| w.nw.child(ChildPos(i))).collect();
    let idx = addrs.iter().position(|&a| k < child_min(w.nw.block(), a)).unwrap_or(cc);
    //preorder anchor: gap 0 (or no children) → the root itself, After; mid →
    //the routed child's subtree edge, Before; append → the last child's, After
    let (open, _) = if idx == 0 || cc == 0 {
        w.open_here(Rel::After)?
    } else if idx < cc {
        w.open_child(ChildPos(idx), Rel::Before)?
    } else {
        w.open_child(ChildPos(cc - 1), Rel::After)?
    };
    block.insert(open, leaf);
    let a = block.p2a(open.0);
    let root = block.data().root();
    wire_child(block, root, ChildPos(idx), a);
    Ok(())
}

fn two_level_demo() -> Result<(), BlockExhausted> {
    let mut block = BlockT::new();
    let root = block.insert_root(BNode::internal());
    block.set_data(BTreeMeta { root, height: 1 });

    insert_leaf(&mut block, 40, BNode::leaf(&[(40, 400), (42, 421)]))?;
    insert_leaf(&mut block, 30, BNode::leaf(&[(30, 303), (33, 331)]))?;
    insert_leaf(&mut block, 35, BNode::leaf(&[(35, 351), (37, 372)]))?;
    insert_leaf(&mut block, 25, BNode::leaf(&[(25, 251)]))?;
    insert_leaf(&mut block, 20, BNode::leaf(&[(20, 201)]))?;

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
        "two-level demo: ok (open x5, all preorder anchor kinds, in-run + out-of-run slides, placement, cross-leaf get)"
    );
    Ok(())
}

fn main() {
    map_demo();
    two_level_demo().unwrap();
    println!("btree example: all checks passed");
}