use super::*;
use crate::INode;
use crate::InodeProbe;
use crate::InodeWalker;
use crate::UBTree;
use crate::InOrder;
use crate::block::BlockTrait;
use crate::block::PluripotentBlock;
use crate::translator::AddressTranslator;
use crate::leafblock::{PtrUnion, SlicePtr};

///terminal PtrUnion carrying an LPtr (the value type the UBTree stores).
fn tp(ptr: u16) -> PtrUnion<u32, u16> {
    PtrUnion { terminal: SlicePtr { ptr, len: 1 } }
}
fn ptr_of(p: PtrUnion<u32, u16>) -> u16 { unsafe { p.terminal }.ptr }

///single terminal node (height 0): insert/get/remove on the root's bucket arrays.
#[test]
fn ubtree_single_node() {
    let mut t: UBTree<u64> = UBTree::new();
    for &k in &[10u64, 50, 30, 70, 20] {
        t.insert(k, tp(k as u16));
        let root = t.tree.root();
        println!("\n== insert k={k} (root vaddr={root:?}, root phys={}, height={}) ==\n{:#?}",
            t.tree.inner().v2p(root), t.tree.meta(), t.tree);
    }
    for &k in &[10u64, 50, 30, 70, 20] {
        assert_eq!(ptr_of(t.get(&k).expect("get")), k as u16, "k={k}");
    }
    //range query: 25 falls in [20,30) -> bucket 20
    assert_eq!(ptr_of(t.get(&25).unwrap()), 20);
    //remove separator 30; 30 now falls in [20,50) -> bucket 20
    assert_eq!(ptr_of(t.remove(&30).unwrap()), 30);
    assert_eq!(ptr_of(t.get(&30).unwrap()), 20);
}

///many inserts: exercises root promotion (height growth) and internal splits (sibling
///splits under a non-full root, recursively) via the bottom-up Walker::insert driver.
///300 inserts / get / remove / re-get. (Previously #[ignore]'d awaiting the moved-ptr
///fixup; the bottom-up clone-based split landed — see SPLIT_PLAN.md.)
#[test]
fn ubtree_insert_many() {
    let mut t: UBTree<u64> = UBTree::new();
    let keys: Vec<u64> = (0..300u64).map(|i| i * 13 + 7).collect();
    for (i, &k) in keys.iter().enumerate() {
        eprintln!("[INS] i={i}");
        t.insert(k, tp((k % 1000) as u16));
    }
    for &k in &keys {
        assert_eq!(ptr_of(t.get(&k).expect("get")), (k % 1000) as u16, "k={k}");
    }
    for (i, &k) in keys.iter().enumerate() {
        if i % 2 == 0 {
            assert_eq!(ptr_of(t.remove(&k).unwrap()), (k % 1000) as u16, "rm k={k}");
        }
    }
    for (i, &k) in keys.iter().enumerate() {
        if i % 2 == 0 {
            assert!(t.get(&k).is_none() || ptr_of(t.get(&k).unwrap()) != (k % 1000) as u16, "k={k} gone");
        } else {
            assert_eq!(ptr_of(t.get(&k).expect("get")), (k % 1000) as u16, "k={k} survives");
        }
    }
}

///get_mut returns a mut ref into the matched bucket; updating it is visible to get.
#[test]
fn ubtree_get_mut() {
    let mut t: UBTree<u64> = UBTree::new();
    t.insert(100, tp(1));
    *t.get_mut(&100).unwrap() = tp(9);
    assert_eq!(ptr_of(t.get(&100).unwrap()), 9);
}

///Pluripotent inode block: I=IPtr=u32 (block ptr), L=LPtr=u16 (leaf ptr).
///CAP 4096 <= Pluripotent::CAP_LIMIT (1<<16); power of two.
type Blk = PluripotentBlock<'static, INode<u32, u32, u16>, InOrder, u32, 4096>;
//NB: aliased `InodeTree`, not `Tree` — `Tree` (the trait) lives in the type
//namespace, so a `type Tree` alias would shadow it and break `tree.probe`.
type InodeTree = TreeBlock<'static, Blk, InOrder>;

fn inode(nchildren: u8, debug_height: u32) -> INode<u32, u32, u16> {
    INode {
        keys:      [0u32; 3],
        leaves:    [PtrUnion { internal: 0u32 }; 4],
        nchildren,
        debug_height,
    }
}

///store a PtrUnion<IPtr,LPtr> in an INode TreeBlock, probe it, get the value as an
///LPtr from `map(&k)->V` on the raw node. Builds a 2-level tree (root height 1 ->
///child height 0) via the mut walker's insert_child, then reads back via the probe.
#[test]
fn probe_maps_key_to_lptr() {
    //root: tree height 1 (one internal level above the leaves), fresh.
    let mut tree: InodeTree = TreeBlock::new(inode(0, 1), 1u32);

    //child: terminal-parent (the walker/probe stop at it — height 0 after one
    //descent), 2 leaves, 1 separator key. k < 100 -> leaves[0] (LPtr 10);
    //k >= 100 -> leaves[1] (LPtr 20).
    let mut child = inode(2, 0);
    child.keys[0] = 100;
    child.leaves[0] = PtrUnion { terminal: SlicePtr { ptr: 10u16, len: 1u16 } };
    child.leaves[1] = PtrUnion { terminal: SlicePtr { ptr: 20u16, len: 1u16 } };

    //walker positioned at the root (not descended — a fresh root has no child to
    //descend into, try_route would route to an empty slot). insert_child arena-places
    //the child; the consumer (here, the test) wires root.leaves[0] = child_v directly.
    {
        let mut walker: InodeWalker<'_, 'static, Blk, InOrder> =
            InodeWalker::new(&mut tree);
        //.ok() drops the Err (INode has no Debug; unwrap on Result would need it).
        let child_v = walker.insert_child(0, child).ok().expect("insert_child");
        tree.inner.get_mut(tree.root).leaves[0].internal = child_v;
        tree.inner.get_mut(tree.root).nchildren = 1;
    }

    //probe k=50: root routes to its only child (height 1->0), stops, current=child.
    let probe: InodeProbe<'_, 'static, Blk, InOrder> = tree.probe(&50u32);
    let pu = probe.current().map(&50u32).expect("50 routes to a leaf");
    assert_eq!(unsafe { pu.terminal }.ptr, 10u16, "k=50 -> leaves[0]");

    let probe: InodeProbe<'_, 'static, Blk, InOrder> = tree.probe(&100u32);
    assert_eq!(unsafe { probe.current().map(&100u32).unwrap().terminal }.ptr, 20u16);

    let probe: InodeProbe<'_, 'static, Blk, InOrder> = tree.probe(&200u32);
    assert_eq!(unsafe { probe.current().map(&200u32).unwrap().terminal }.ptr, 20u16);
}
#[test]
fn ubtree_root_split_only() {
    let mut t: UBTree<u64> = UBTree::new();
    for i in 0..16u64 {
        let k = i * 10;
        t.insert(k, tp(i as u16));
        let root = t.tree.root();
        println!("\n== insert k={k} (root vaddr={root:?}, root phys={}, height={}) ==\n{:#?}",
            t.tree.inner().v2p(root), t.tree.meta(), t.tree);
    }
    for i in 0..16u64 {
        assert_eq!(ptr_of(t.get(&(i*10)).expect("get")), i as u16, "i={i}");
    }
}

///subtree extremity walks: from a height-2 root, leftmost/rightmost of the root's
///only (internal) child must resolve to that child's two terminal children, not the
///child itself. pure reads — the walker stays at the root.
#[test]
fn leftmost_rightmost_desc() {
    let mut tree: InodeTree = TreeBlock::new(inode(0, 2), 2u32);
    // root(h2) -> internal(h1) -> term_l, term_r (h0). insert_child anchors After(parent)
    // for a fresh (nc=0) parent, so neither insert triggers a slide/fixup.
    let internal_v = {
        let mut w = InodeWalker::new(&mut tree);
        w.insert_child(0, inode(0, 1)).ok().expect("internal")
    };
    tree.inner.get_mut(tree.root).leaves[0] = PtrUnion { internal: internal_v };
    tree.inner.get_mut(tree.root).nchildren = 1;
    let (term_l, term_r) = {
        let mut w = InodeWalker::new(&mut tree);
        w.descend(0);
        let tl = w.insert_child(0, inode(0, 0)).ok().expect("tl");
        let tr = w.insert_child(1, inode(0, 0)).ok().expect("tr");
        (tl, tr)
    };
    tree.inner.get_mut(internal_v).leaves[0] = PtrUnion { internal: term_l };
    tree.inner.get_mut(internal_v).leaves[1] = PtrUnion { internal: term_r };
    tree.inner.get_mut(internal_v).nchildren = 2;
    let root_v = tree.root;
    let (lm, rm, pos) = {
        let w = InodeWalker::new(&mut tree);
        (w.leftmost_desc(internal_v), w.rightmost_desc(internal_v), w.position())
    };
    assert_eq!(lm, term_l, "leftmost desc");
    assert_eq!(rm, term_r, "rightmost desc");
    assert_eq!(pos, root_v, "extremity walk moved the walker");
}

///debug-layout demo: successive in-block inserts of INodes, printing the block's
///`Debug` view after each. watch the translator (shift ticks down, len doubles on
///spread) and each inode's child vaddr mapped to its physical slot (`i:[phys,...]`).
/// this test is not an example of how to build a well formed tree. 
#[test]
fn debug_layout_demo() {
    use crate::block::{BlockMutTrait, BlockTrait, OpenSlot, UniformBlock};
    //Uniform<InOrder> over u16: INIT_SHIFT=15, INIT_CAP=2; root at MIDPOINT=32768 (phys 1).
    //CAP 16 keeps the layout readable; I=L=u16 so children_array holds in-block vaddrs.
    type Blk = UniformBlock<'static, INode<u32, u16, u16>, InOrder, u16, 16>;
    let mk_inode = |child: u16, nchildren: u8| INode::<u32, u16, u16> {
        keys:      [0u32; 3],
        leaves:    [PtrUnion { internal: child }; 4],
        nchildren,
        debug_height: 1, //these are internal nodes pointing at a child
    };

    let mut block: Blk = BlockMutTrait::new();
    let root = block.insert_root(mk_inode(0, 0));
    println!("\n== insert_root (root phys={}) ==\n{:#?}", block.v2p(root), block);

    let mut prev = root;
    for step in 1..=6u16 {
        //child 0 of the new node points at the previously inserted node -> the debug
        //view shows that child vaddr mapped to its physical slot.
        let slide = block.find_slot(prev, true, Some(root)).expect("slot");
        let slot: OpenSlot = block.slide_none(slide, Some(root));
        let vaddr = block.insert(mk_inode(prev, 1), slot);
        println!("\n== insert_child #{step} (phys={}, child phys={}) ==\n{:#?}", block.v2p(vaddr), block.v2p(prev), block);
        //consistency: the new vaddr resolves and the pin never moved.
        assert!(block.v2p(root) == block.translator().v2p(root), "root moved");
        assert_eq!(block.occupied(), 1 + step as usize);
        prev = vaddr;
    }
}
