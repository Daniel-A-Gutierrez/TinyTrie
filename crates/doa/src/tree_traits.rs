use crate::index::BlockIndex;

enum EitherOr<A : Sized,B : Sized> {
    A(A),
    B(B),
    None
}

trait Node<K : Ord + Sized, V : Sized, P : BlockIndex> : Sized {
    fn map(&self, k: K) -> EitherOr<V,P>;
    fn keys(&self) -> impl NodeIter<Self,K>;
    fn vals(&self) -> impl NodeIter<Self,V>;
    fn ptrs(&self) -> impl NodeIter<Self,P>;
    fn keys_mut(&mut self) -> impl NodeIter<Self,K>; 
    fn vals_mut(&mut self) -> impl NodeIter<Self,V> ;
    fn ptrs_mut(&mut self) -> impl NodeIter<Self,P> ;
}

trait RevPtrNode<K : Ord + Sized, V : Sized, P : BlockIndex> : Node <K, V, P> {
    fn parent(&self) -> P;
}

///cant use default iter cuz we want to be lifetime exempt. 
trait NodeIter<N, T> {
    fn next(&mut self, node : &N,) -> &T;
    fn prev(&mut self, node : &N,) -> &T;
    fn seek(&mut self, p : usize);
    fn position(&self) -> usize;
}

trait TreeCursor<K : Ord + Sized, V : Sized, P : BlockIndex> {
    fn pop(&mut self) -> Option<usize>;
    fn push(&mut self);
    fn parent(&self) -> Option<usize>; 
    fn ascend(&mut self);
    fn descend(&mut self);
    fn current(&mut self)-> Option<(&K,&V)>;
    fn next(&mut self);
    fn prev(&mut self);
}

/*
case 1 : nodes store values
case 2 : leaves store values, nodes store ptrs
case 3 : nodes store ptrs and values, discriminated somehow.

we care because it matters for iteration and traversal. 
Should an iteration over pairs touch only leafnodes, or all nodes? 
cant assume num keys = num ptrs = num values. 

maybe i just ... do all of em.
also iteration depends on pre/post/in order. can impl treecursor generically for order maybe? 

height sensitive union stuff will have to rely on a different impl of cursor. 
if p and V are separate types though... no thats the right direction. the cursor drops the N bound, but 
we impl TreeCursor<K,V,P> for some DefaultTreeCursor<K,V,P,N:Node>

lets consider a b+tree first, then a btree, then a binary tree. 

the problem i see is references - if the cursor owns a &Block, not only can the block not change,
the items in it cant change. 

So its nearly useless as far as im concerned right now. 
If it owns a &mut, it alone can traverse and modify the parent tree. 
that could be fine but there's methods in the leafblock that want to update the parent - does that 
mean those have to take a &mut Cursor and use it? 
Or can the cursor handle handing out discrete mutable references somehow? 

furthermore if the leafblock splits and inserts a new child into a parent node which causes the parent node
to split, a regular iter_mut cant handle that. A default cursor cant either. Thats a btree problem. 

so we follow the cursor to get to position, check if the node has space, and if its gonna split that inode...
that insert needs to take the cursor as an arg to get the parent and position in the parent to update it. 

so leaf_block.split_node(parent_cursor) -> new nodes P, updated parent cursor and parent
let inode = parent_cursor.current_mut
if if inode can accomodate new P {insert easy}
else ... {
    let arena,position,ancestors = cursor.into_parts()
    arena.insert_after(position, ///...
    /*
    actually why cant a cursor_mut that holds &mut arena insert into the arena? 
    insert just has to take the new node.
    the node is made from current, which we have a &mut to. 
    we can do cursor.insert_after(&mut self, val)...
    so thats not a interface on the arena, its an interface on the cursor. 
    it would need to borrow a slice of the arena mutably, move inodes around, update the parent nodes,
    update itself (if a moved item was in Lineage it needs to move),
    then it returns. 

    ok so big progress. the blocks interface needs to be much simpler, its basically just storage/address mapping
    also a cursor needs to be made from a block by getting a key, or iterating. 
    since the point of the cursor is to be able to walk freely it needs the lineage, so it needs to start
    at the root. 

    furthermore, maybe a immutable cursor doesnt take the lineage so its alloc free, and can be made from P.
    is cursor a good name for that? the leafnode cursor wont need lineage either. 
    not allocating is a huge perf win for lookups.     
    */
}

the cursor needs to be taken to get a &mut Arena, Vec<(P,usize)>, the arena updated, then the P's in the cursor
updated and the cursor remade if its needed again. 
*/