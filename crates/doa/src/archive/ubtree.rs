
/*
plan : 
impl Node + Ordered Node for Inode 
create concrete walker type thats height aware and impl TreeWalkerMut<Ordering, RawBlockType> on it
ditto for probe
tree nav + trie pos on both?
Then we should get a TreeBlock<Inner, O> that we can get the walker and probe from.
Those should be capable of insert/lookup/remove

current success criteria : store PtrUnion<Iptr,Lptr> in a Inode Treeblock, 
probe it, get the value as an Lptr from map(&k)->V on the raw node type. 
*/

///in a b+tree theres 1 more key per value for inodes
///DEGREE = max children. 4 (not 3): a full node has 4 children, mid = 4>>1 = 2, so split
///gives 2+2 — both halves have ≥2 children (a valid median; DEGREE=3 gives a 1-child half,
///a degenerate 0-key node whose median-gap placement underflows).
#[derive(Clone)]
pub(crate) struct INode<K: Sized + Ord, I: BlockIndex, L: BlockIndex> {
    pub(crate) keys:        [K; 3],
    pub(crate) leaves:      [PtrUnion<I, L>; 4],
    pub(crate) nchildren:   u8, //occupied child slots. nkeys = nchildren-1. 0 = fresh.
    ///debug-only node height (0 = terminal/leaf, >0 = internal level). read solely by
    ///`SlotDebug::debug_render` to pick SlicePtr vs child-vaddr. never read by logic, so a
    ///stale value only affects debug output. set at construction/split sites.
    pub(crate) debug_height: u32,
}

impl<K: Ord, I: BlockIndex, L: BlockIndex> INode<K, I, L> {
    pub(crate) const DEGREE: usize = 4;

    fn nkeys(&self) -> usize { self.nchildren.saturating_sub(1) as usize }

    fn keys_slice(&self) -> &[K] { &self.keys[..self.nkeys()] }

    fn leaves_slice(&self) -> &[PtrUnion<I, L>] { &self.leaves[..self.nchildren as usize] }

    ///child vaddrs (the union's `internal` arm) for occupied child slots; `None` for the
    ///rest. reads the internal arm — valid at internal levels (see `ChildNodeIter`);
    ///terminal nodes hold `SlicePtr`s there, so callers must be height-aware.
    pub(crate) fn children_array(&self) -> [Option<I>; 4] {
        const { assert!(Self::DEGREE == 4) }
        let mut a = [None; 4];
        for i in 0..self.nchildren as usize {
            a[i] = Some(unsafe { self.leaves[i].internal });
        }
        a
    }

    ///route k to the child slot whose range contains it, return that child PtrUnion.
    ///value semantics: consumer reads .terminal.ptr for the LPtr at the bottom level,
    ///.internal for the child inode ptr. idx = match binary_search { Ok(i)=>i+1, Err(i)=>i }.
    fn map(&self, k: &K) -> Option<PtrUnion<I, L>> {
        let idx = match self.keys_slice().binary_search(k) {
            Ok(i) => i + 1,
            Err(i) => i,
        };
        (idx < self.nchildren as usize).then_some(self.leaves[idx])
    }
}

impl<K: Sized + Ord, I: BlockIndex, L: BlockIndex> SlotDebug<I> for INode<K, I, L> {
    fn debug_render(&self, tr: &Translator<I>) -> Vec<String> {
        let nc = self.nchildren as usize;
        let terminal = self.debug_height == 0;
        (0..nc)
            .map(|i| {
                if terminal {
                    //terminal: leaves hold SlicePtr<L>.
                    let sp = unsafe { self.leaves[i].terminal };
                    format!("L{}:{}", sp.ptr.as_usize(), sp.len.as_usize())
                } else {
                    //internal: leaves hold child vaddrs -> phys.
                    let cv = unsafe { self.leaves[i].internal };
                    format!("{}", tr.v2p(cv))
                }
            })
            .collect()
    }
}

///read-only height-tracking probe for the UBTree (`TreeBlock`). Holds `&'b TreeBlock`;
///`try_route` returns None at `P::MIN` (terminal — don't descend into leaves), else asks
///the node to route. `descend` decrements height. Sits above `Tree`.
pub(crate) struct InodeProbe<'b, 'a, Inner: BlockMutTrait<'a> + 'a, O: Ordering>
where Inner::T: Node<'a, Inner::P, O> {
    tree: &'b TreeBlock<'a, Inner, O>,
    pos: Inner::P,
    height: Inner::P,
}

///positioned cursor over `INode::leaves[0..nchildren]` yielding each child's
///`.internal` ptr (IPtr). Only called at internal levels (height>0).
struct ChildNodeIter<'a, I: BlockIndex, L: BlockIndex> { leaves: &'a [PtrUnion<I, L>], idx: usize }

impl<'a, I: BlockIndex, L: BlockIndex> NodeIterBase<'a, I> for ChildNodeIter<'a, I, L> {
    fn position(&self) -> usize { self.idx }
    fn len(&self) -> usize { self.leaves.len() }
    fn cap(&self) -> usize { self.leaves.len() }
    fn prev(&mut self) { self.idx = self.idx.saturating_sub(1); }
    fn next(&mut self) { self.idx = (self.idx + 1).min(self.leaves.len()); }
    fn seek(&mut self, p: usize) { self.idx = p.min(self.leaves.len()); }
}

impl<'a, I: BlockIndex, L: BlockIndex> NodeIter<'a, I> for ChildNodeIter<'a, I, L> {
    fn current(&self) -> I { unsafe { self.leaves[self.idx].internal } }
}

///mut cursor over `INode::leaves[0..nchildren]` — `current_mut` reborrows each
///child's `.internal` (IPtr) mutably. `NodeIterMut` (not `NodeIter`): hands out
///`&mut I` per call, tied to `&mut self`, not stored.
struct ChildNodeIterMut<'a, I: BlockIndex, L: BlockIndex> { leaves: &'a mut [PtrUnion<I, L>], idx: usize }

impl<'a, I: BlockIndex, L: BlockIndex> NodeIterBase<'a, I> for ChildNodeIterMut<'a, I, L> {
    fn position(&self) -> usize { self.idx }
    fn len(&self) -> usize { self.leaves.len() }
    fn cap(&self) -> usize { self.leaves.len() }
    fn prev(&mut self) { self.idx = self.idx.saturating_sub(1); }
    fn next(&mut self) { self.idx = (self.idx + 1).min(self.leaves.len()); }
    fn seek(&mut self, p: usize) { self.idx = p.min(self.leaves.len()); }
}

impl<'a, I: BlockIndex, L: BlockIndex> NodeIterMut<'a, I> for ChildNodeIterMut<'a, I, L> {
    fn current_mut(&mut self) -> &mut I { unsafe { &mut self.leaves[self.idx].internal } }
}

impl<K: Ord, I: BlockIndex, L: BlockIndex> OrderedNode<I, InOrder> for INode<K, I, L> {
    ///InOrder: parent sits between child[half-1] and child[half]. New child lands in
    ///the gap between lower (parent if child_idx==half, else child[child_idx-1]) and
    ///upper (child[child_idx]); return After(lower) — find_slot searches forward.
    fn insert_position(&self, this: I, child_idx: usize) -> RelTo<I> {
        let half = Self::DEGREE / 2;
        let nc = self.nchildren as usize;
        if nc == 0 { return RelTo::After(this); }                          //fresh: anchor at parent
        if child_idx == 0 { return RelTo::Before(unsafe { self.leaves[0].internal }); }
        let lower = if child_idx == half { this } else { unsafe { self.leaves[child_idx - 1].internal } };
        RelTo::After(lower)
    }
}


impl<'a, K, I, L> Node<'a, I, InOrder> for INode<K, I, L>
where
    K: Ord + Copy + Default + 'a,
    I: BlockIndex,
    L: BlockIndex,
{
    type K = K;
    type V = PtrUnion<I, L>;

    fn try_route<'s>(&'s self, k: &K) -> Option<usize> where 'a: 's {
        if self.nchildren == 0 { return None; }
        Some(route_idx(self, k))
    }

    fn lookup<'s>(&'s self, query: &K) -> Option<impl NodeIter<'s, I>> where 'a: 's {
        let idx = route_idx(self, query);
        (idx < self.nchildren as usize).then_some(ChildNodeIter { leaves: self.leaves_slice(), idx })
    }

    fn keys<'s>(&'s self) -> impl NodeIter<'s, &'s K> where 'a: 's {
        SliceNodeIter { slice: self.keys_slice(), idx: 0 }
    }

    fn children<'s>(&'s self) -> impl NodeIter<'s, I> where 'a: 's {
        ChildNodeIter { leaves: self.leaves_slice(), idx: 0 }
    }

    fn children_mut<'s>(&'s mut self) -> impl NodeIterMut<'s, I> where 'a: 's {
        ChildNodeIterMut { leaves: &mut self.leaves[..self.nchildren as usize], idx: 0 }
    }

    fn sibling_ptrs(&mut self) -> Option<(&mut I, &mut I)> { todo!() }
    fn parent_ptr(&mut self) -> Option<&mut I> { todo!() }
    fn self_ptr(&mut self) -> Option<&mut I> { todo!() }
    fn remove_child(&mut self, _k: &K, _child_idx: usize) { todo!() }
    fn degree() -> usize { Self::DEGREE }
    ///leaf-safe: uses `nchildren` directly (the default's `children()` reads the union's
    /// `.internal` arm, which is a `SlicePtr` on terminal nodes — UB to read). valid for both.
    fn is_full(&self) -> bool { self.nchildren as usize >= Self::DEGREE }

    fn update_child(&mut self, child_idx: usize, new_p: I) { self.leaves[child_idx].internal = new_p; }

    fn clear_child(&mut self, _child_idx: usize) { todo!() }

    ///logical split: self keeps left half, returns (right half, separator).
    ///thin wrapper over `split_off`: `let (sep, right) = self.split_off(self.nchildren as usize >> 1); (right, sep)`.
    ///mid = child_count >> 1; separator = keys[mid-1] (boundary key, promoted up).
    fn split(&mut self) -> (Self, Self::K) {
        let mid = self.nchildren as usize >> 1;
        let (sep, right) = self.split_off(mid);
        (right, sep)
    }

    ///non-mutating right-half extraction (zero-waste override): build the right half from
    ///self's arrays without altering self, so self stays full & wired until the right half
    ///is placed & wired. mirrors `split_off` minus the self shrink.
    fn right_half(&self) -> (Self, Self::K) {
        let mid = self.nchildren as usize >> 1;
        let n = self.nchildren as usize;
        let sep = self.keys[mid - 1];
        let right_n = n - mid;
        let mut right = INode::empty();
        if right_n > 1 {
            right.keys[..right_n - 1].copy_from_slice(&self.keys[mid..n - 1]);
        }
        right.leaves[..right_n].copy_from_slice(&self.leaves[mid..n]);
        right.nchildren = right_n as u8;
        right.debug_height = self.debug_height;
        (right, sep)
    }

    ///shrink self to the left half in place: just drop nchildren to mid (keys/leaves beyond
    ///mid are stale but unread). call AFTER the right half is placed & wired.
    fn truncate_to_left_half(&mut self) {
        self.nchildren = (self.nchildren as usize >> 1) as u8;
    }

    ///insert (k, payload) in order; split-when-full, return the overflow to propagate.
    ///maps the `Payload` arm to the `PtrUnion` storage (the union IS the unification of
    ///value-bucket and child ptr): `Value(v) => v` (already a PtrUnion, terminal arm set by
    ///caller); `Child(p) => PtrUnion{internal: p}` (wrap the child vaddr).
    ///  let payload = match payload { Payload::Value(v)=>v, Payload::Child(p)=>PtrUnion{internal:p} };
    ///  if !is_full: insert_bucket(k, payload); None.
    ///  else: let (sep, right) = self.split_off(nchildren>>1);
    ///        if k < sep { self.insert_bucket(k, payload) } else { right.insert_bucket(k, payload) };
    ///        Some(Overflow{ right, sep }).
    /// both halves have room post-split (DEGREE>=3 — TODO: bump DEGREE 2 -> >=3, resize
    /// keys/leaves arrays and the `children_array` const assert).
    fn insert(&mut self, k: Self::K, payload: Payload<Self::V, I>) -> Option<Overflow<Self, Self::K>> {
        let v = match payload { Payload::Value(v) => v, Payload::Child(p) => PtrUnion { internal: p } };
        if !self.is_full() { self.insert_bucket(k, v); return None; }
        let mid = self.nchildren as usize >> 1;
        let (sep, mut right) = self.split_off(mid);
        if k < sep { self.insert_bucket(k, v) } else { right.insert_bucket(k, v) }
        Some(Overflow { right, sep })
    }
}


impl<'b, 'a, Inner, O> TreePos<Inner::P> for InodeProbe<'b, 'a, Inner, O>
where
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn position(&self) -> Inner::P { self.pos }
    fn set_position(&mut self, p: Inner::P) { self.pos = p; }
    fn height(&self) -> Inner::P { self.height }
    fn set_height(&mut self, h: Inner::P) { self.height = h; }
}

impl<'b, 'a, Inner, O> TreeRoute<'b, 'a, TreeBlock<'a, Inner, O>> for InodeProbe<'b, 'a, Inner, O>
where
    'a: 'b,
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn try_route(&self, k: &<Inner::T as Node<'a, Inner::P, O>>::K) -> Option<usize> {
        if self.height == Inner::P::MIN { return None; }
        self.block().get(self.position()).try_route(k)
    }

    fn block(&self) -> &Inner { self.tree.inner() }
}

impl<'b, 'a, Inner, O> Probe<'b, 'a, TreeBlock<'a, Inner, O>> for InodeProbe<'b, 'a, Inner, O>
where
    'a: 'b,
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn new(tree: &'b TreeBlock<'a, Inner, O>) -> Self {
        Self { tree, pos: tree.root(), height: *tree.meta() }
    }

    fn descend<'s>(&'s mut self, child_idx: usize) where 'a: 's {
        self.set_position(self.child_at(child_idx));
        self.height = self.height.wrapping_sub(Inner::P::ONE);
    }
}

///owning-mut height-tracking walker for the UBTree (`TreeBlock`). Holds `&'b mut
///TreeBlock` (the whole tree — so it can update root/meta on a split), the ancestor
///stack for `TreeNav`, and a `height` counter seeded from `TreeBlock::meta`; `descend`
///pushes lineage, steps, and decrements height.
pub(crate) struct InodeWalker<'b, 'a, Inner: BlockMutTrait<'a> + 'a, O: Ordering>
where Inner::T: Node<'a, Inner::P, O> {
    tree: &'b mut TreeBlock<'a, Inner, O>,
    pos: Inner::P,
    height: Inner::P,
    stack: VecDeque<(Inner::P, usize)>,
}

impl<'b, 'a, Inner, O> TreePos<Inner::P> for InodeWalker<'b, 'a, Inner, O>
where
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn position(&self) -> Inner::P { self.pos }
    fn set_position(&mut self, p: Inner::P) { self.pos = p; }
    fn height(&self) -> Inner::P { self.height }
    fn set_height(&mut self, h: Inner::P) { self.height = h; }
}

impl<'b, 'a, Inner, O> TreeRoute<'b, 'a, TreeBlock<'a, Inner, O>> for InodeWalker<'b, 'a, Inner, O>
where
    'a: 'b,
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn try_route(&self, k: &<Inner::T as Node<'a, Inner::P, O>>::K) -> Option<usize> {
        if self.height == Inner::P::MIN { return None; }
        self.block().get(self.position()).try_route(k)
    }

    fn block(&self) -> &Inner { self.tree.inner() }
}

impl<'b, 'a, Inner, O> TreeNav<Inner::P> for InodeWalker<'b, 'a, Inner, O>
where
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn pop(&mut self) -> Option<(Inner::P, usize)> { self.stack.pop_back() }
    fn push(&mut self, parent: Inner::P, child_idx: usize) { self.stack.push_back((parent, child_idx)); }
    fn parent(&self) -> Option<(Inner::P, usize)> { self.stack.back().copied() }
    fn ascend(&mut self) {
        if let Some((pv, _)) = self.pop() {
            self.set_position(pv);
            self.height = self.height.wrapping_add(Inner::P::ONE);
        }
    }
    ///in-order (depth-first) successor. layout: left children (child[0..half]), parent,
    ///right children (child[half..k]); the parent sits between child[half-1] and
    ///child[half]. so an internal node's successor is the leftmost of its first right
    ///child (child[half]); if it has no right children it's the last in its subtree and
    ///we go up. a terminal's successor is found by ascending to the nearest ancestor
    ///where we came up from a non-last child.
    fn next(&mut self) {
        let half = <Inner::T as Node<'a, Inner::P, O>>::degree() / 2;
        eprintln!("[next] before pos={:?} h={:?} parent={:?}", self.pos, self.height, self.parent());
        if self.height > Inner::P::MIN {
            let k = self.block().get(self.pos).children().len();
            if k > half {
                self.descend(half);
                while self.height > Inner::P::MIN { self.descend(0); }
                return;
            }
        }
        loop {
            let Some((pv, j)) = self.pop() else { return; };
            self.set_position(pv);
            self.height = self.height.wrapping_add(Inner::P::ONE);
            let k = self.block().get(pv).children().len();
            //came up from child[j] of pv. pv's in-order: child[0..half], pv, child[half..k].
            let last_left = if k > half { j == half - 1 } else { j == k - 1 };
            if last_left { return; }                  //successor is pv (the parent)
            if k > half && j == k - 1 { continue; }   //last right child -> keep ascending
            self.descend(j + 1);                     //else leftmost of the next child
            while self.height > Inner::P::MIN { self.descend(0); }
            return;
        }
    }
    ///in-order (depth-first) predecessor, mirror of next. an internal node's predecessor
    ///is the rightmost of its last left child; a terminal's is the rightmost of the
    ///previous child, or its parent if it's the first right child, or the parent's
    ///predecessor if it's the first child.
    fn prev(&mut self) {
        let half = <Inner::T as Node<'a, Inner::P, O>>::degree() / 2;
        loop {
            if self.height > Inner::P::MIN {
                let k = self.block().get(self.pos).children().len();
                if k > 0 {
                    let last_left = if k > half { half - 1 } else { k - 1 };
                    self.descend(last_left);
                    while self.height > Inner::P::MIN {
                        let k2 = self.block().get(self.pos).children().len();
                        if k2 > half { self.descend(k2 - 1); } else { break; }
                    }
                    return;
                }
            }
            let Some((pv, j)) = self.pop() else { return; };
            self.set_position(pv);
            self.height = self.height.wrapping_add(Inner::P::ONE);
            if j == half { return; }    //came from first right child -> predecessor is pv
            if j > 0 {
                self.descend(j - 1);    //rightmost of the previous child
                while self.height > Inner::P::MIN {
                    let k2 = self.block().get(self.pos).children().len();
                    if k2 > half { self.descend(k2 - 1); } else { break; }
                }
                return;
            }
            //j == 0: came from first child -> predecessor is pv's predecessor (loop).
        }
    }
    fn right(&mut self) { todo!() }
    fn left(&mut self) { todo!() }
}

impl<'b, 'a, Inner, O> Walker<'b, 'a, TreeBlock<'a, Inner, O>> for InodeWalker<'b, 'a, Inner, O>
where
    'a: 'b,
    Inner: BlockMutTrait<'a> + 'a,
    O: Ordering,
    Inner::T: Node<'a, Inner::P, O>,
{
    fn new(tree: &'b mut TreeBlock<'a, Inner, O>) -> Self {
        let pos = tree.root();
        let height = *tree.meta();
        Self { tree, pos, height, stack: VecDeque::new() }
    }

    fn block_mut(&mut self) -> &mut Inner { self.tree.inner_mut() }

    fn root(&self) -> Inner::P { self.tree.root() }
    fn set_root(&mut self, root: Inner::P) { *self.tree.root_mut() = root; }
    fn bump_height(&mut self) {
        *self.tree.meta_mut() = self.tree.meta().wrapping_add(Inner::P::ONE);
    }
    fn root_height(&self) -> Inner::P { *self.tree.meta() }

    fn descend<'s>(&'s mut self, child_idx: usize) where 'a: 's {
        let cur = self.position();
        let child = self.child_at(child_idx);
        self.push(cur, child_idx);
        self.set_position(child);
        self.height = self.height.wrapping_sub(Inner::P::ONE);
    }
}

// ─── UBTree: union b+tree, the inode half ─────────────────────────────────────
// arena over a UniformBlock of INodes; ptr upkeep only. values are PtrUnions the
// consumer points at external leaf storage. terminal INodes store range-bucket
// ptrs (separator keys bound ranges); internal INodes route by separator to child
// inodes. insert splits proactively top-down (full root grows height via
// `insert_root`; full child splits into a sibling via `insert_child` + manual
// parent wiring). remove does not rebalance (underflow left).

const UB_CAP: usize = 4096;
pub(crate) type UBInner<K> = UniformBlock<'static, INode<K, u32, u16>, InOrder, u32, UB_CAP>;
type UBBlock<K> = TreeBlock<'static, UBInner<K>, InOrder>;

impl<K, I, L> INode<K, I, L>
where
    K: Ord + Copy + Default,
    I: BlockIndex,
    L: BlockIndex,
{
    ///`Default` is `empty` — used by `promote_new_root` (`Node::default()`) for the new root.
    fn empty() -> Self {
        Self {
            keys: std::array::from_fn(|_| K::default()),
            leaves: std::array::from_fn(|_| PtrUnion { internal: I::MIN }),
            nchildren: 0,
            debug_height: 0,
        }
    }

    fn is_full(&self) -> bool { self.nchildren as usize >= Self::DEGREE }

    ///install/replace the bucket for separator `k`: if `k` is an existing separator,
    ///overwrite its bucket; else insert a new separator + bucket (shift right). caller
    ///guarantees the node is not full (proactive split). leaves[0] is the permanent
    ///underflow bucket (range below the first separator); it's never moved by inserts.
    ///nchildren = #buckets = #separators + 1, so the first insert goes 0 -> 2.
    fn insert_bucket(&mut self, k: K, v: PtrUnion<I, L>) {
        debug_assert!(!self.is_full(), "insert_bucket: node full");
        let nc = self.nchildren as usize;
        if nc == 0 {
            //leaves[0] is the underflow bucket (empty from init); leaves[1] is k's.
            self.keys[0] = k;
            self.leaves[1] = v;
            self.nchildren = 2;
            return;
        }
        let nk = nc - 1;
        match self.keys[..nk].binary_search(&k) {
            Ok(i) => self.leaves[i + 1] = v,
            Err(i) => {
                self.keys.copy_within(i..nk, i + 1);
                self.leaves.copy_within(i + 1..nc, i + 2);
                self.keys[i] = k;
                self.leaves[i + 1] = v;
                self.nchildren += 1;
            }
        }
    }

    ///remove separator `k` and its bucket (leaves[i+1]); the left bucket absorbs the
    ///range. returns the removed bucket if `k` was a separator, else None.
    fn remove_bucket(&mut self, k: &K) -> Option<PtrUnion<I, L>> {
        let nk = self.nkeys();
        let i = self.keys[..nk].binary_search(k).ok()?;
        let removed = self.leaves[i + 1];
        let nc = self.nchildren as usize;
        self.keys.copy_within(i + 1..nk, i);
        self.leaves.copy_within(i + 2..nc, i + 1);
        self.nchildren -= 1;
        Some(removed)
    }

    ///split at `mid`: self keeps leaves[0..mid] (nchildren=mid), right gets
    ///leaves[mid..n]. separator keys[mid-1] moves up to the parent (the boundary
    ///between the two halves). terminal and internal split identically here — the
    ///boundary key lives in the parent, not duplicated. returns (separator, right).
    fn split_off(&mut self, mid: usize) -> (K, INode<K, I, L>) {
        let n = self.nchildren as usize;
        let sep = self.keys[mid - 1];
        let right_n = n - mid;
        let mut right = INode::empty();
        if right_n > 1 {
            right.keys[..right_n - 1].copy_from_slice(&self.keys[mid..n - 1]);
        }
        right.leaves[..right_n].copy_from_slice(&self.leaves[mid..n]);
        right.nchildren = right_n as u8;
        self.nchildren = mid as u8;
        (sep, right)
    }
}

///`Default` == `empty`; required by `Node: Default` (used by `promote_new_root`'s new root).
impl<K, I, L> Default for INode<K, I, L>
where
    K: Ord + Copy + Default,
    I: BlockIndex,
    L: BlockIndex,
{
    fn default() -> Self { Self::empty() }
}


pub struct UBTree<K: Ord + Copy + Default + 'static> {
    pub(crate) tree: UBBlock<K>,
}

impl<K: Ord + Copy + Default + 'static> UBTree<K> {
    pub fn new() -> Self {
        Self { tree: TreeBlock::new(INode::empty(), 0u32) }
    }

    pub fn get(&self, k: &K) -> Option<PtrUnion<u32, u16>> {
        let mut p = InodeProbe::new(&self.tree);
        while let Some(ci) = p.try_route(k) {
            p.descend(ci);
        }
        p.current().map(k)
    }

    pub fn get_mut(&mut self, k: &K) -> Option<&mut PtrUnion<u32, u16>> {
        let terminal_v = {
            let mut w = InodeWalker::new(&mut self.tree);
            while let Some(ci) = w.try_route(k) {
                w.descend(ci);
            }
            w.position()
        };
        let node = self.tree.inner_mut().get_mut(terminal_v);
        let nc = node.nchildren as usize;
        let nk = node.nkeys();
        let idx = match node.keys[..nk].binary_search(k) {
            Ok(i) => i + 1,
            Err(i) => i,
        };
        (idx < nc).then(|| &mut node.leaves[idx])
    }

    pub fn insert(&mut self, k: K, v: PtrUnion<u32, u16>) {
        // top-down proactive split: descend to the leaf for `k`; if it's full, recursively
        // split it and any full ancestors (split makes the path roomy); re-descend and insert
        // (infallible — the leaf has room). see SPLIT_PLAN.md + "Tree split invariants" in
        // crates/doa/CLAUDE.md.
        let mut walker = InodeWalker::new(&mut self.tree);
        while let Some(ci) = walker.try_route(&k) { walker.descend(ci); }
        if walker.current().is_full() {
            walker.split(&k, <u32 as Num>::MIN);
        }
        // re-descend from the root to the (now roomy) leaf — `k` may route into the leaf's new
        // right-half sibling; either way the target leaf has room.
        let root_v = walker.root();
        let rh = walker.root_height();
        walker.set_position(root_v);
        walker.set_height(rh);
        while walker.pop().is_some() {}
        while let Some(ci) = walker.try_route(&k) { walker.descend(ci); }
        let overflow = walker.current_mut().insert(k, Payload::Value(v));
        debug_assert!(overflow.is_none(), "UBTree::insert: leaf still full after split");
    }

    ///remove separator `k` and its bucket. no rebalancing — underfull nodes are left.
    pub fn remove(&mut self, k: &K) -> Option<PtrUnion<u32, u16>> {
        let terminal_v = {
            let mut w = InodeWalker::new(&mut self.tree);
            while let Some(ci) = w.try_route(k) {
                w.descend(ci);
            }
            w.position()
        };
        self.tree.inner_mut().get_mut(terminal_v).remove_bucket(k)
    }
}
