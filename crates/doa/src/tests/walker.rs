//! open-surface tests. preorder: a B+ consumer (Vec nodes, DEGREE 4 — many
//! splits/promotions) driving consumer-driven splits, validated structurally +
//! by walk order. in-order: a DEGREE=2 binary consumer for rotations + fixup
//! delivery across opens.
//! validation: walk positions strictly increasing and reachable == occupied
//! (the walk-==-slot-order canary) + per-node structure (separators re-derive
//! from child mins, leaves sorted). run targeted — full-crate runs OOM the IDE.

use crate::blocks::{BlockTrait, UniformBlock};
use crate::metadata::{Ancestry, ChildPos, DoubleSlide, Fixable, GrewFixup, HasRoot, Pos,
                      PosAncestry, SwapFixup};
use crate::store::{NoneSlide, Store};
use crate::translator::Translator;
use crate::treeblock::{search, walker};
use crate::walker::{BlockExhausted, Node, NodeCursor, NodeWalker, NodeWalkerMut, OpenFixups,
                    PreOrderWalk, InOrderWalk, TreeWalk, TreeWalker};
use crate::{Fixup, InOrder, PreOrder, Rel};

// ---------------------------------------------------------------------------
// shared meta
// ---------------------------------------------------------------------------

///root position + tree height (B+ leaves sit at depth == height).
#[derive(Clone, Copy, Debug, Default)]
struct Meta {
    root:   Pos,
    height: u32,
}

impl Fixable<u16> for Meta {
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

impl HasRoot<u16> for Meta {
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

///apply an open's fixups to a held addr — the open's slides/grow rename whatever
/// moved; a held addr is only valid post-fixup.
fn fix_addr<'block, B: BlockTrait<'block, A = u16>>(block: &B, fixups: &OpenFixups, a: &mut u16) {
    //addrs are grow-stable by construction — only the slides remap them
    let tr = block.translator();
    if fixups.slides.affects_addr(*a, tr) {
        fixups.slides.fix_addr(a, tr);
    }
}

// ---------------------------------------------------------------------------
// preorder B+ consumer — Vec nodes, DEGREE 4 (force splits + promotions)
// ---------------------------------------------------------------------------

const DEGREE: usize = 4;

struct INode {
    keys:     Vec<u64>,
    children: Vec<u16>,
}
struct LNode {
    keys:   Vec<u64>,
    values: Vec<u64>,
}
enum BNode {
    Internal(INode),
    Leaf(LNode),
}

impl BNode {
    fn internal() -> Self {
        BNode::Internal(INode { keys: Vec::new(), children: Vec::new() })
    }
    fn leaf(pairs: &[(u64, u64)]) -> Self {
        let mut n = LNode { keys: Vec::new(), values: Vec::new() };
        for (k, v) in pairs {
            n.keys.push(*k);
            n.values.push(*v);
        }
        BNode::Leaf(n)
    }
    ///fresh internal node pre-wired with its first child = `child0`.
    fn new_parent(child0: u16) -> Self {
        BNode::Internal(INode { keys: Vec::new(), children: vec![child0] })
    }
    ///drain the right half out (returned); self keeps the left half.
    fn split(&mut self) -> Self {
        match self {
            BNode::Leaf(n) => {
                let mid = n.keys.len() >> 1;
                let r = LNode {
                    keys:   n.keys.split_off(mid),
                    values: n.values.split_off(mid),
                };
                BNode::Leaf(r)
            }
            BNode::Internal(n) => {
                let cc = n.children.len();
                let mid = cc >> 1;
                n.keys.remove(mid - 1); //the boundary separator moves out (re-derived at wire)
                let r =
                    INode { keys: n.keys.split_off(mid - 1), children: n.children.split_off(mid) };
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
    const STORES_PARENTS: bool = false;
}

type PBlock<'block> = UniformBlock<'block, BNode, u16, Meta, PreOrder>;

fn child_min(b: &PBlock<'_>, mut c: u16) -> u64 {
    loop {
        match b.aget(c) {
            BNode::Leaf(n) => return n.keys[0],
            BNode::Internal(n) => c = n.children[0],
        }
    }
}

fn node_full(b: &PBlock<'_>, pos: Pos) -> bool {
    match b.get(pos) {
        BNode::Internal(n) => n.children.len() == DEGREE,
        BNode::Leaf(n) => n.keys.len() == DEGREE,
    }
}

///B+ wire: insert `addr` as child `child_idx` of the node at `par`; separators
///re-derive from child mins (`keys[i] = min(children[i+1])`).
fn wire_child(block: &mut PBlock<'_>, par: Pos, child_idx: ChildPos, addr: u16) {
    let m = child_min(block, addr);
    let old_left = {
        let BNode::Internal(n) = block.get(par) else { panic!("wire_child: not internal") };
        (child_idx == 0 && !n.children.is_empty())
            .then(|| child_min(block, n.children[0]))
    };
    let BNode::Internal(n) = block.get_mut(par) else { panic!("wire_child: not internal") };
    n.children.insert(child_idx.0, addr);
    if child_idx == 0 {
        if let Some(sep) = old_left {
            n.keys.insert(0, sep);
        }
    } else {
        n.keys.insert(child_idx.0 - 1, m);
    }
}

fn root_state<'block, B: BlockTrait<'block, A = u16, BlockData = Meta>>(b: &B) -> PosAncestry {
    PosAncestry { pos: b.data().root, ancestry: Ancestry::default() }
}

struct PCursor<'block, 'walker> {
    b:     &'walker PBlock<'block>,
    state: PosAncestry,
}

impl<'block, 'a> From<&'a PBlock<'block>> for PCursor<'block, 'a>
where 'block: 'a
{
    fn from(b: &'a PBlock<'block>) -> Self {
        Self { b, state: root_state(b) }
    }
}

impl<'block, 'walker> NodeCursor<'block, PBlock<'block>> for PCursor<'block, 'walker> {
    fn block(&self) -> &PBlock<'block> {
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
            BNode::Internal(n) => n.children[idx.0],
            BNode::Leaf(_) => panic!("child: leaf"),
        }
    }
    fn children(&self) -> impl DoubleEndedIterator<Item = (ChildPos, u16)>
                             + ExactSizeIterator
                             + '_ {
        let cc = self.child_count();
        (0..cc).map(|i| (ChildPos(i), self.child(ChildPos(i))))
    }
    ///descent slot for `k`; `None` at a leaf. equal-right B+ separators.
    fn lookup(&self, k: &u64) -> Option<ChildPos> {
        match self.current() {
            BNode::Leaf(_) => None,
            BNode::Internal(n) => {
                let p = n.keys.iter().position(|&key| key > *k).unwrap_or(n.keys.len());
                Some(ChildPos(p.min(n.children.len().saturating_sub(1))))
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

impl<'block, 'walker> NodeWalker<'block, PBlock<'block>> for PCursor<'block, 'walker> {
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

struct PCursorMut<'block, 'walker> {
    b:     &'walker mut PBlock<'block>,
    state: PosAncestry,
}

impl<'block, 'a> From<&'a mut PBlock<'block>> for PCursorMut<'block, 'a>
where 'block: 'a
{
    fn from(b: &'a mut PBlock<'block>) -> Self {
        let state = root_state(b);
        Self { b, state }
    }
}

impl<'block, 'walker> NodeCursor<'block, PBlock<'block>> for PCursorMut<'block, 'walker> {
    fn block(&self) -> &PBlock<'block> {
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
            BNode::Internal(n) => n.children[idx.0],
            BNode::Leaf(_) => panic!("child: leaf"),
        }
    }
    fn children(&self) -> impl DoubleEndedIterator<Item = (ChildPos, u16)>
                             + ExactSizeIterator
                             + '_ {
        let cc = self.child_count();
        (0..cc).map(|i| (ChildPos(i), self.child(ChildPos(i))))
    }
    fn lookup(&self, k: &u64) -> Option<ChildPos> {
        match self.current() {
            BNode::Leaf(_) => None,
            BNode::Internal(n) => {
                let p = n.keys.iter().position(|&key| key > *k).unwrap_or(n.keys.len());
                Some(ChildPos(p.min(n.children.len().saturating_sub(1))))
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

impl<'block, 'walker> NodeWalker<'block, PBlock<'block>> for PCursorMut<'block, 'walker> {
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

impl<'block, 'walker> Fixable<u16> for PCursorMut<'block, 'walker> {
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

impl<'block, 'walker> NodeWalkerMut<'block, PBlock<'block>> for PCursorMut<'block, 'walker> {
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

    fn block_mut(&mut self) -> &mut PBlock<'block> {
        self.b
    }
    fn set_child(&mut self, up: usize, child: ChildPos, addr: u16) {
        let target = match up {
            0 => self.state.pos,
            n => self.state.ancestry.stack[self.state.ancestry.len() - n].parent,
        };
        match self.block_mut().get_mut(target) {
            BNode::Internal(n) => n.children[child.0] = addr,
            BNode::Leaf(_) => panic!("set_child: leaf"),
        }
    }
    fn clear_child(&mut self, _up: usize, _child: ChildPos) {
        panic!("clear_child: binary rotations only");
    }
    fn set_parent(&mut self, _addr: u16) {}
}

///the map — consumer-driven splits (preemptive descent; inlined split flows:
/// the opens consume the walker, releasing the block borrow for the drain/wire).
struct PMap {
    block: PBlock<'static>,
    len:   usize,
}

impl PMap {
    fn new() -> Self {
        let mut block = PBlock::new();
        let root = block.insert_root(BNode::leaf(&[]));
        block.set_data(Meta { root, height: 0 });
        Self { block, len: 0 }
    }

    fn get(&self, k: &u64) -> Option<u64> {
        let w: TreeWalker<PreOrder, PCursor<'_, '_>> = search(&self.block, k);
        match w.nw.current() {
            BNode::Leaf(n) => n.keys.iter().position(|&key| key == *k).map(|i| n.values[i]),
            _ => None,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn insert(&mut self, k: u64, v: u64) -> Result<(), BlockExhausted> {
        loop {
            let mut w: TreeWalker<PreOrder, PCursorMut<'_, '_>> = walker(&mut self.block);
            let mut restart = false;
            loop {
                if node_full(w.nw.block(), w.nw.position()) {
                    if w.nw.is_root() {
                        let mut r_a = w.nw.block().p2a(w.nw.position()); //held across the opens
                        let cc = w.nw.child_count();
                        if cc == 0 {
                            //childless root: sequential — NR before R, then Y after R
                            let (nr_open, fixups) = w.open_parent()?;
                            fix_addr(&self.block, &fixups, &mut r_a);
                            self.block.insert(nr_open, BNode::new_parent(r_a));
                            //NR held as an ADDR — the second open can slide/spread
                            //and `nr_open`'s pos would go stale (§2 addendum)
                            let mut nr_a = self.block.p2a(nr_open.0);
                            self.block.data_mut().set_root(nr_open.0);
                            let h = self.block.data().height() + 1;
                            self.block.data_mut().set_height(h);
                            let mut w2: TreeWalker<PreOrder, PCursorMut<'_, '_>> =
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
                            //internal root: atomic — NR before R + Y before R's child[mid]
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
                        //split child idx of the parent (Y before X's child[mid])
                        let (_, idx) = w.nw.ascend(); //walker: -> the parent (has room)
                        let mut x_a = w.nw.child(idx); //held across the open
                        let mut par_a = w.nw.block().p2a(w.nw.position());
                        w.nw.descend(idx); //walker: -> X
                        let cc = w.nw.child_count();
                        let (open, fixups) = if cc == 0 {
                            w.open_here(Rel::After)? //childless X: Y right after it
                        } else {
                            w.open_child(ChildPos(cc >> 1), Rel::Before)?
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
                    let pos = w.nw.position();
                    let BNode::Leaf(n) = w.nw.block_mut().get_mut(pos) else {
                        panic!("insert: not a leaf")
                    };
                    if let Some(p) = n.keys.iter().position(|&key| key == k) {
                        n.values[p] = v; //overwrite: no len change
                    } else {
                        let at = n.keys.iter().position(|&key| k < key).unwrap_or(n.keys.len());
                        n.keys.insert(at, k);
                        n.values.insert(at, v);
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
}

// ---- preorder validation ----

///walk positions: strictly increasing, and reachable == occupied (the
/// walk-==-slot-order canary).
fn validate(block: &PBlock<'_>) {
    let mut w: TreeWalker<PreOrder, PCursor<'_, '_>> = walker(block);
    let mut positions = vec![];
    if w.first().is_some() {
        loop {
            positions.push(w.nw.position());
            if w.next().is_none() {
                break;
            }
        }
    }
    assert!(
        positions.windows(2).all(|p| p[0] < p[1]),
        "walk order not strictly increasing: {positions:?}"
    );
    assert_eq!(positions.len(), block.occupied(), "reachable != occupied — ghost slots");

    let mut leaf_keys: Vec<u64> = vec![];
    for pos in 0..block.len() {
        let Some(node) = block.store().slot(Pos(pos)) else { continue };
        match node {
            BNode::Internal(n) => {
                assert_eq!(n.keys.len() + 1, n.children.len(), "inode shape at {pos}");
                for i in 0..n.children.len() - 1 {
                    assert_eq!(
                        n.keys[i],
                        child_min(block, n.children[i + 1]),
                        "separator {i} != child min at {pos}"
                    );
                    assert!(
                        child_min(block, n.children[i]) < child_min(block, n.children[i + 1]),
                        "children out of key order at {pos}"
                    );
                }
            }
            BNode::Leaf(n) => {
                assert!(n.keys.windows(2).all(|k| k[0] < k[1]), "leaf unsorted at {pos}");
                leaf_keys.extend_from_slice(&n.keys);
            }
        }
    }
    assert!(leaf_keys.windows(2).all(|k| k[0] < k[1]), "leaf key order across the block");
}

// ---------------------------------------------------------------------------
// in-order DEGREE=2 binary consumer — slot storage ([Option; 2])
// ---------------------------------------------------------------------------

struct BinNode {
    key:  u64,
    kids: [Option<u16>; 2],
}

impl Node for BinNode {
    type K = u64;
    type V = u64;
    type A = u16;
    const DEGREE: usize = 2;
    const STORES_PARENTS: bool = false;
}

type BBlock<'block> = UniformBlock<'block, BinNode, u16, Meta, InOrder>;

struct BCursorMut<'block, 'walker> {
    b:     &'walker mut BBlock<'block>,
    state: PosAncestry,
}

impl<'block, 'a> From<&'a mut BBlock<'block>> for BCursorMut<'block, 'a>
where 'block: 'a
{
    fn from(b: &'a mut BBlock<'block>) -> Self {
        let state = PosAncestry { pos: b.data().root, ancestry: Ancestry::default() };
        Self { b, state }
    }
}

impl<'block, 'walker> NodeCursor<'block, BBlock<'block>> for BCursorMut<'block, 'walker> {
    fn block(&self) -> &BBlock<'block> {
        self.b
    }
    fn position(&self) -> Pos {
        self.state.pos
    }
    fn is_root(&self) -> bool {
        self.state.ancestry.is_empty()
    }
    ///a binary node is a block-level leaf iff it has no kids (slot-based, not
    /// height-based — the binary meta's height is unused).
    fn is_leaf(&self) -> bool {
        self.current().kids.iter().all(|k| k.is_none())
    }
    fn child_count(&self) -> usize {
        self.current().kids.iter().filter(|k| k.is_some()).count()
    }
    ///slot-addressed: `child(i)` names the node in slot `i` (present by contract).
    fn child(&self, idx: ChildPos) -> u16 {
        self.current().kids[idx.0].expect("child: empty slot")
    }
    fn children(&self) -> impl DoubleEndedIterator<Item = (ChildPos, u16)>
                             + ExactSizeIterator
                             + '_ {
        let present: Vec<(usize, u16)> = self
            .current()
            .kids
            .iter()
            .enumerate()
            .filter_map(|(i, k)| k.map(|a| (i, a)))
            .collect();
        present.into_iter().map(|(i, a)| (ChildPos(i), a))
    }
    fn lookup(&self, k: &u64) -> Option<ChildPos> {
        if self.is_leaf() {
            return None;
        }
        Some(ChildPos(usize::from(*k > self.current().key)))
    }
    fn descend<'b>(&'b mut self, child: ChildPos) -> &'b BinNode
    where 'block: 'b {
        let addr = self.child(child);
        let pos = self.b.a2p(addr);
        let parent = self.state.pos;
        self.state.ancestry.push(parent, child);
        self.state.pos = pos;
        self.b.get(pos)
    }
}

impl<'block, 'walker> NodeWalker<'block, BBlock<'block>> for BCursorMut<'block, 'walker> {
    fn ascend<'b>(&'b mut self) -> (&'b BinNode, ChildPos)
    where 'block: 'b {
        let a = self.state.ancestry.pop().expect("ascend: at root");
        self.state.pos = a.parent;
        (self.b.get(a.parent), a.child)
    }
    fn parent(&self) -> Option<(Pos, ChildPos)> {
        self.state.ancestry.last().map(|a| (a.parent, a.child))
    }
}

impl<'block, 'walker> Fixable<u16> for BCursorMut<'block, 'walker> {
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

impl<'block, 'walker> NodeWalkerMut<'block, BBlock<'block>> for BCursorMut<'block, 'walker> {
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

    fn block_mut(&mut self) -> &mut BBlock<'block> {
        self.b
    }
    fn set_child(&mut self, up: usize, child: ChildPos, addr: u16) {
        let target = match up {
            0 => self.state.pos,
            n => self.state.ancestry.stack[self.state.ancestry.len() - n].parent,
        };
        self.block_mut().get_mut(target).kids[child.0] = Some(addr);
    }
    fn clear_child(&mut self, up: usize, child: ChildPos) {
        let target = match up {
            0 => self.state.pos,
            n => self.state.ancestry.stack[self.state.ancestry.len() - n].parent,
        };
        self.block_mut().get_mut(target).kids[child.0] = None;
    }
    fn set_parent(&mut self, _addr: u16) {}
}

///shared (read) cursor over the binary block — for walk-order checks.
struct BCursor<'block, 'walker> {
    b:     &'walker BBlock<'block>,
    state: PosAncestry,
}

impl<'block, 'a> From<&'a BBlock<'block>> for BCursor<'block, 'a>
where 'block: 'a
{
    fn from(b: &'a BBlock<'block>) -> Self {
        let state = PosAncestry { pos: b.data().root, ancestry: Ancestry::default() };
        Self { b, state }
    }
}

impl<'block, 'walker> NodeCursor<'block, BBlock<'block>> for BCursor<'block, 'walker> {
    fn block(&self) -> &BBlock<'block> {
        self.b
    }
    fn position(&self) -> Pos {
        self.state.pos
    }
    fn is_root(&self) -> bool {
        self.state.ancestry.is_empty()
    }
    fn is_leaf(&self) -> bool {
        self.current().kids.iter().all(|k| k.is_none())
    }
    fn child_count(&self) -> usize {
        self.current().kids.iter().filter(|k| k.is_some()).count()
    }
    fn child(&self, idx: ChildPos) -> u16 {
        self.current().kids[idx.0].expect("child: empty slot")
    }
    fn children(&self) -> impl DoubleEndedIterator<Item = (ChildPos, u16)>
                             + ExactSizeIterator
                             + '_ {
        let present: Vec<(usize, u16)> = self
            .current()
            .kids
            .iter()
            .enumerate()
            .filter_map(|(i, k)| k.map(|a| (i, a)))
            .collect();
        present.into_iter().map(|(i, a)| (ChildPos(i), a))
    }
    fn lookup(&self, k: &u64) -> Option<ChildPos> {
        if self.is_leaf() {
            return None;
        }
        Some(ChildPos(usize::from(*k > self.current().key)))
    }
    fn descend<'b>(&'b mut self, child: ChildPos) -> &'b BinNode
    where 'block: 'b {
        let addr = self.child(child);
        let pos = self.b.a2p(addr);
        let parent = self.state.pos;
        self.state.ancestry.push(parent, child);
        self.state.pos = pos;
        self.b.get(pos)
    }
}

impl<'block, 'walker> NodeWalker<'block, BBlock<'block>> for BCursor<'block, 'walker> {
    fn ascend<'b>(&'b mut self) -> (&'b BinNode, ChildPos)
    where 'block: 'b {
        let a = self.state.ancestry.pop().expect("ascend: at root");
        self.state.pos = a.parent;
        (self.b.get(a.parent), a.child)
    }
    fn parent(&self) -> Option<(Pos, ChildPos)> {
        self.state.ancestry.last().map(|a| (a.parent, a.child))
    }
}

///insert `k` as a leaf of the BST: descend by comparison until the target side
///slot is empty, open there (in-order anchors: left → before the node's region,
///right → after it), insert, wire the slot.
fn bst_insert(block: &mut BBlock<'_>, k: u64) -> Result<(), BlockExhausted> {
    let mut w: TreeWalker<InOrder, BCursorMut<'_, '_>> = walker(&mut *block);
    loop {
        let side = usize::from(k > w.nw.current().key);
        if w.nw.current().kids[side].is_none() {
            break; //this node gains k at `side`
        }
        w.nw.descend(ChildPos(side)); //walker: -> the occupied-side child
    }
    let side = usize::from(k > w.nw.current().key);
    assert!(
        side == 0 || w.nw.current().kids[0].is_some(),
        "bst_insert: right child of a leftless node — the in-order convention \
         requires the first child at slot 0"
    );
    let mut node_a = w.nw.block().p2a(w.nw.position()); //held across the open
    let (open, fixups) = if side == 0 {
        w.open_here(Rel::Before)? //left child: before the node's whole region
    } else {
        w.open_here(Rel::After)? //right child: after the node's gap
    };
    fix_addr(block, &fixups, &mut node_a);
    block.insert(open, BinNode { key: k, kids: [None, None] });
    let a = block.p2a(open.0);
    block.get_mut(block.a2p(node_a)).kids[side] = Some(a);
    Ok(())
}

fn bst_block(keys: &[u64]) -> BBlock<'static> {
    let mut block = BBlock::new();
    let root = block.insert_root(BinNode { key: keys[0], kids: [None, None] });
    block.set_data(Meta { root, height: 0 });
    for &k in &keys[1..] {
        bst_insert(&mut block, k).unwrap();
    }
    block
}

///reference in-order sequence over slot storage directly — the crate's nav
/// assumes packed ChildPos (rank == slot), which sparse one-kid nodes violate.
fn bst_keys(block: &BBlock<'_>) -> Vec<u64> {
    fn visit(block: &BBlock<'_>, a: u16, out: &mut Vec<u64>) {
        let n = block.aget(a);
        if let Some(l) = n.kids[0] {
            visit(block, l, out);
        }
        out.push(n.key);
        if let Some(r) = n.kids[1] {
            visit(block, r, out);
        }
    }
    let root_a = block.p2a(block.data().root());
    let mut out = vec![];
    visit(block, root_a, &mut out);
    out
}

///in-order walk keys + the strictly-increasing-positions check.
fn bst_walk(block: &BBlock<'_>) -> Vec<u64> {
    let mut w: TreeWalker<InOrder, BCursor<'_, '_>> = walker(block);
    let mut keys = vec![];
    let mut positions = vec![];
    if w.first().is_some() {
        loop {
            keys.push(w.nw.current().key);
            positions.push(w.nw.position());
            if w.next().is_none() {
                break;
            }
        }
    }
    assert!(
        positions.windows(2).all(|p| p[0] < p[1]),
        "in-order walk not strictly increasing: {positions:?}"
    );
    assert_eq!(positions.len(), block.occupied(), "reachable != occupied — ghost slots");
    keys
}

fn kid(block: &BBlock<'_>, pos: Pos, slot: usize) -> Option<u64> {
    block.get(pos).kids[slot].map(|a| block.aget(a).key)
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

fn map_torture(keys: Vec<u64>) {
    let mut m = PMap::new();
    for k in &keys {
        m.insert(*k, k * 10).unwrap_or_else(|_| {
            panic!(
                "insert({k}) exhausted: len {} cap {} occupied {} shift {} height {}",
                m.block.len(),
                m.block.cap(),
                m.block.occupied(),
                m.block.translator().shift(),
                m.block.data().height,
            )
        });
    }
    validate(&m.block);
    assert_eq!(m.len(), keys.len());
    for k in &keys {
        assert_eq!(m.get(k), Some(k * 10), "get({k})");
    }
    assert_eq!(m.get(&0), None); //0 never inserted (keys start at 1)
    assert!(m.block.data().height >= 2, "expected promotions");
}

#[test]
fn pre_map_ascending() {
    map_torture((1..=240).collect());
}

#[test]
fn pre_map_descending() {
    map_torture((1..=240).rev().collect());
}

#[test]
fn pre_map_stride() {
    map_torture((0..240u64).map(|i| i * 7 % 240 + 1).collect());
}

///hand-assembled two-level tree: all preorder anchor kinds (parent-adjacent,
/// subtree-edge Before, append After), in-run + out-of-run slides, placement.
#[test]
fn pre_hand_assembled() {
    let mut block = PBlock::new();
    let root = block.insert_root(BNode::internal());
    block.set_data(Meta { root, height: 1 });

    //insert a leaf at its key-ordered gap (open + block insert + wire)
    fn insert_leaf(block: &mut PBlock<'_>, k: u64, pairs: &[(u64, u64)]) {
        let w: TreeWalker<PreOrder, PCursorMut<'_, '_>> = walker(&mut *block);
        let cc = w.nw.child_count();
        let addrs: Vec<u16> = (0..cc).map(|i| w.nw.child(ChildPos(i))).collect();
        let idx = addrs.iter().position(|&a| k < child_min(w.nw.block(), a)).unwrap_or(cc);
        let (open, _) = if idx == 0 || cc == 0 {
            w.open_here(Rel::After).unwrap() //gap 0: right after the root
        } else if idx < cc {
            w.open_child(ChildPos(idx), Rel::Before).unwrap() //mid gap
        } else {
            w.open_child(ChildPos(cc - 1), Rel::After).unwrap() //append
        };
        block.insert(open, BNode::leaf(pairs));
        let a = block.p2a(open.0);
        let r = block.data().root();
        wire_child(block, r, ChildPos(idx), a);
    }

    insert_leaf(&mut block, 40, &[(40, 400), (42, 421)]);
    insert_leaf(&mut block, 30, &[(30, 303), (33, 331)]);
    insert_leaf(&mut block, 35, &[(35, 351), (37, 372)]);
    insert_leaf(&mut block, 25, &[(25, 251)]);
    insert_leaf(&mut block, 20, &[(20, 201)]); //the out-of-run slide case

    validate(&block);

    //preorder node order: root, then leaves in key order
    let mut w: TreeWalker<PreOrder, PCursor<'_, '_>> = walker(&block);
    w.first().unwrap();
    let mut order = vec![];
    loop {
        match w.nw.current() {
            BNode::Internal(n) => order.push(1000 + n.children.len() as u64),
            BNode::Leaf(n) => order.push(n.keys[0]),
        }
        if w.next().is_none() {
            break;
        }
    }
    assert_eq!(order, vec![1005, 20, 25, 30, 35, 40]);

    let m = PMap { block, len: 8 };
    for (k, v) in
        [(20, 201), (25, 251), (30, 303), (33, 331), (35, 351), (37, 372), (40, 400), (42, 421)]
    {
        assert_eq!(m.get(&k), Some(v), "get({k})");
    }
    assert_eq!(m.get(&22), None);
}

///rotate_right at the root: child 0 rises; the block root follows; the walker
///ends on the riser; the in-order sequence is unchanged.
#[test]
fn in_rotate_root() {
    let mut block = bst_block(&[50, 30, 70, 10, 40]);
    let want: Vec<u64> = {
        let mut w = vec![50, 30, 70, 10, 40];
        w.sort();
        w
    };
    assert_eq!(bst_walk(&block), want);

    let mut w: TreeWalker<InOrder, BCursorMut<'_, '_>> = walker(&mut block); //at 50
    w.rotate_right();

    assert_eq!(w.nw.current().key, 30, "walker ends on the riser");
    assert_eq!(block.get(block.data().root()).key, 30, "block root follows");
    assert_eq!(bst_walk(&block), want, "in-order sequence preserved");
    //30: kids [10, 50]; 50: kids [40, 70]
    let l_pos = block.data().root();
    assert_eq!(kid(&block, l_pos, 0), Some(10));
    assert_eq!(kid(&block, l_pos, 1), Some(50));
    let p_pos = block.a2p(block.get(l_pos).kids[1].unwrap());
    assert_eq!(kid(&block, p_pos, 0), Some(40));
    assert_eq!(kid(&block, p_pos, 1), Some(70));
}

///rotate_right with a leaf riser: the demoted node's slot 0 clears.
#[test]
fn in_rotate_leaf_riser() {
    let mut block = bst_block(&[50, 10, 70]);
    let want: Vec<u64> = {
        let mut w = vec![50, 10, 70];
        w.sort();
        w
    };
    assert_eq!(bst_walk(&block), want);

    let mut w: TreeWalker<InOrder, BCursorMut<'_, '_>> = walker(&mut block); //at 50
    w.rotate_right();

    assert_eq!(w.nw.current().key, 10, "leaf riser");
    assert_eq!(block.get(block.data().root()).key, 10);
    //both post-rotate nodes are one-kid (slot 0 empty) — packed-nav-walkable
    //never; use the reference sequence
    assert_eq!(bst_keys(&block), want);
    //10: [_, 50]; 50: [_, 70] (slot 0 cleared)
    let l_pos = block.data().root();
    assert_eq!(kid(&block, l_pos, 0), None);
    assert_eq!(kid(&block, l_pos, 1), Some(50));
    let p_pos = block.a2p(block.get(l_pos).kids[1].unwrap());
    assert_eq!(kid(&block, p_pos, 0), None);
    assert_eq!(kid(&block, p_pos, 1), Some(70));
}

///rotate_left mid-tree: not the root (no root move); the grandparent's entry
///repoints; the walker ends on the riser.
#[test]
fn in_rotate_left_mid() {
    let mut block = bst_block(&[30, 10, 50, 40, 70]);
    let want: Vec<u64> = {
        let mut w = vec![30, 10, 50, 40, 70];
        w.sort();
        w
    };
    assert_eq!(bst_walk(&block), want);

    let mut w: TreeWalker<InOrder, BCursorMut<'_, '_>> = walker(&mut block); //at 30
    w.nw.descend(ChildPos(1)); //walker: -> 50 (the right child)
    w.rotate_left();

    assert_eq!(w.nw.current().key, 70, "walker ends on the riser");
    assert_eq!(block.get(block.data().root()).key, 30, "root unchanged");
    assert_eq!(bst_walk(&block), want);
    //30: [10, 70]; 70: [50, _]; 50: [40, _]
    let r_pos = block.data().root();
    assert_eq!(kid(&block, r_pos, 0), Some(10));
    assert_eq!(kid(&block, r_pos, 1), Some(70));
    let r70_pos = block.a2p(block.get(r_pos).kids[1].unwrap());
    assert_eq!(kid(&block, r70_pos, 0), Some(50));
    assert_eq!(kid(&block, r70_pos, 1), None);
    let p50_pos = block.a2p(block.get(r70_pos).kids[0].unwrap());
    assert_eq!(kid(&block, p50_pos, 0), Some(40));
    assert_eq!(kid(&block, p50_pos, 1), None);
}

///held addresses across opens: apply the returned fixups and the addr still
///names its node. the build's gap-0 inserts shift the first leaf rightward, so
///the fixups are load-bearing.
#[test]
fn pre_fixup_delivery() {
    let mut block = PBlock::new();
    let root = block.insert_root(BNode::internal());
    block.set_data(Meta { root, height: 1 });

    //first leaf (key 40) — capture its addr and starting position
    let w: TreeWalker<PreOrder, PCursorMut<'_, '_>> = walker(&mut block);
    let (open, _) = w.open_here(Rel::After).unwrap();
    block.insert(open, BNode::leaf(&[(40, 400)]));
    let mut held = block.p2a(open.0);
    let start_pos = open.0;
    let r = block.data().root();
    wire_child(&mut block, r, ChildPos(0), held);

    //gap-0 inserts — each open slides, moving the 40-leaf rightward
    for (k, v) in [(30u64, 303u64), (25, 251), (20, 201)] {
        let w: TreeWalker<PreOrder, PCursorMut<'_, '_>> = walker(&mut block);
        let (open, fixups) = w.open_here(Rel::After).unwrap(); //gap 0
        fix_addr(&block, &fixups, &mut held);
        block.insert(open, BNode::leaf(&[(k, v)]));
        let a = block.p2a(open.0);
        let r = block.data().root();
        wire_child(&mut block, r, ChildPos(0), a);
    }

    let held_pos = block.a2p(held);
    assert_ne!(held_pos, start_pos, "the build never moved the held leaf?");
    let BNode::Leaf(n) = block.aget(held) else { panic!("held addr is not the 40 leaf") };
    assert_eq!(n.keys[0], 40, "held addr still names its node");
    validate(&block);
}