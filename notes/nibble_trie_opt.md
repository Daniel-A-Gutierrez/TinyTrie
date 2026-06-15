# OKOK big opportunities left for optimization 

## node stacking
Another fundamental weakness of this structure is its sparse layout. BTree wastes its space on pointers, 
we waste them on empty slots in our Nodes. If we assume though that each node only has 2-4 children on average
we can change Node from :  
``` rust
#[derive(Copy, Clone)]
struct Node<PTR: TrieIndex, LEN: TrieIndex> {
    children: [PTR; 16],
    prefix_len: LEN,
    leaf_mask: u16,
    leaf: PTR,
    offset: u64,  // bit 63 = terminal, bits 0-62 = raw buf offset
}
//to this (or something close with const generics). 
// with len=u16, stak=2, ptr=u32 wed have 76 bytes, smaller than the current one, but insertion will slow down significantly. 
#[derive(Copy, Clone)]
struct Node<PTR: TrieIndex, LEN: TrieIndex, STAK>
where StackSize<STAK> : ValidStackSize {
    prefix_len: [LEN;STAK],
    leaf_mask: [u16;STAK],
    occupancy: [u16;STAK],
    terminal : u8 //bitmask.
    children: [PTR; 16],
}

// valid values of stack are 1-8. We implement ValidStackSize for Stack<1> Stack<2> etc. 
//stack is a generic empty struct. 
```

on insertion , when a node is split we check whether the children of either of the nodes occupancy masks would overlap. 

*important* whenever a co-occupant node would overlap its parent at all, it needs to be split off. 
So if 2 nodes co occupy, grandparent has a child at 0 and child (co_occupant) wants to insert a leaf at 0 as well, child needs to get its own Node and get kicked out from its parent's node. 

Before allocating a new node for a split, we first check if this node has suitable occupancy. It makes no sense for a node to have 0 children due to the above assertion, so 0 can serve as a sentinel value in occupancy mask. 

The child value in this case within the node, points to the nodes own address. 
It's up to the traverser to increment some value - lets say header_idx - when it traverses to a node with the same address as its parent. 

Since the trie would now have an occupancy mask, we'd no longer need 0 as a sentinel value in our index array. Saves a bit of space at small sizes. 

Maybe im being too stingy and we just store leaves : [PTR;STAK] too and keep some insertion performance. 

## fixed size input 
dont need to store offset or an indirect index, leaves can point straight at the key/value's index.
With the node stacking occupancy masks, we gain the potential to store the value type INLINE within the node, no back buffer at all. That would definitely be a different variant of the trie, but its exciting. 
We'd have to do a manual enum thing, basically an untagged union going off of the leaf mask. 

## forgetting insertion order
IF we were to leave behind the ability to get each item by the order it was inserted in (which lets current nibble_trie work like a database column), we can sort Trie.index alongside Trie.buf. This should dramatically 
improve cache performance. 

## A skirt? (linked list at the bottom). 
Trie.index can store (prev : PTR, (buf_ptr : u64,len) , next : PTR) and for in order iteration we can just use that like a linked list, after we seek to the correct position using our tree.
We'd need to maintain 0 as a sentinel value though. Also len but thatd change on each insert automatically to just point at whatever was added last. 

## POST STATS ANALYSIS
most noes only have 1 or 2 children, but nibble trie performs best when the data is very dense. 

Implement a version of nibble trie that takes a 'fixed length' . 
Each string stored takes up the full 'max length' in buf. 
We pad it with zeros up to 'max length' on insertion , and on lookup either expect it to be the 'max length' or null terminated. We don't copy it and null terminate it ourselves, thats the consumers responsibility. 
When returning it, we do a scan from the back to find the first nonzero byte and use that to calculate the length for the &[u8] . 
Thus we no longer need an indirection buffer to tell us the start and length of a stored string. 
To function as a map we store a Vec<T> alongside it, and when we optimize, move the key and value in tandem so they wind up in the same order in the final buffers. 
If a leaf points at '5' , the mapped value is at vec[5], and key_to_slice(buf[5*max_len..(5+1)*max_len]).
If we optimize periodically, say each time our len is a power of two, we should get pretty decent iteration performance. 
I dont think we need to store 'offset' in each node anymore, since offset can be calculated from the value of leaf. 
We also don't store the index buffer, just the Vec<T> for the map. 

## Stacking v2
Integrate stacking into the optimization algorithm preliminarily, then if it works well, insert.

Currently optimize attempts to reorder nodes and values, but doesn't touch the index, effectively making it a 3 way map where (key,insertion order,value) are all preserved.
We're going to let it forget insertion order, and sort value alongside buf so leaf[1] -> buf[1st string]. 

So when trie.buf is sorted, trie.index is sorted alongside it. 

IN addition, new nodes will be packed into their parents if its occupancy mask allows. 
Rules for node stacking : 
- leaf must point to the 'youngest child''s leaf, to maintain the invariant that key[..prefix_len] is valid. 
    - this is pretty trivial with greedy stacking. 
- all 'cohabitating' nodes must be descendants of one another. 2 non-leaf siblings or cousins cant stack because they don't share a prefix.

to accomplish this, we're gonna work something into the depth first iteration present in optimize, and modify node's structure, and the way we index into nodes. 

We're going to reserve some amount of bits for cohabitating nodes - lets call it stackbits. its at most 3 (so 8 virtual nodes in a physical node). 

I'm not sure how to most elegantly express the relation between 3 bits and an array size of 8 without relying on cosnt generics, so I'll leave that bit to ai. 

```rust
#[derive(Copy, Clone)]
pub struct Node<PTR: TrieIndex, LEN: TrieIndex, const STAK : u8> {
    pub children: [PTR; 16],
    pub prefix_len: [LEN; SB],
    pub leaf_mask: [u16 ; SB],
    pub occupancy: [u16 ; SB],
    pub terminal : u8, //bitmask, instead of offset bit 63. 
    pub leaf: PTR,
    pub offset: u64,  // bit 63 = terminal, bits 0-62 = raw buf offset
}
```
note the inclusion of an occupancy mask - we can no longer assume that just because a child's value is not 0 or int::max that its a child of this node, we need to & it with the bit in this vnode's occupancy mask. 

so for STAK = 8, thats 3 bits, so when a node points to address 16, we /8 or bitshift>>3 for the index in arena, 
then %8 for the index within the node (to get prefix_len etc). 

The lower the vnode index, the 'older' the vnode is - 0 should be the oldest. 
the presence of a sentinel value in the physical node means none of the vnodes have taken it so its open for insertion, we just have to update the occupancy mask of the vnode after. 
If its not the sentinel value though we can't assume that it *is* a child of this vnode, we have to check the occupancy mask, and if its 0, move our vnode out of the physical node. It should just get an entirely new node.
The intent for right now is to confine the computational cost of this optimization to optimize as a sort of 'control' to see how much the cost of insertion rises before and after its implemented there.

### Optimize fn

We walk the trie, maintain 16 sets of node addresses, adding a physical address to a set corresponding to a position in the occupancy mask.
at each new node we visit we calculate its physical node's occupancy. 
Say its 00101.
We are looking for other occupancy masks that contain `**0*0`.
So either we do a 'not in set' query against far fewer (assuming low occupancy) things or we 
do a 'any in set' against many more things. 
Elaborating, if we put a nodes address in each set where its occupancy was 1, we're looking for any node thats a parent (i guess thats another set to maintain) thats NOT in both sets corresponding to the 2 set 1s in 00101 (set 0 and set 2). 
Alternatively if we put a node in a set wherever its occupancy is 0, we're upfront adding and removing it from many sets, but when we query where we can put a new node, its just an intersection of the 2 sets corresponding to where our new node's 1s are. 

lets estimate. 
P = avg occupancy of unstacked nodes 0-16 . the longer the data the lower this goes. 
D = avg depth of trie - again , the longer the data the stringier it gets

//1 tracking
check if root can be pasted onto 0 of its parents
    - it cant
root node - insert into (16*P sets)
continue 

Dth node - check if we can be pasted into any of D-1 parents
    16 sets on average having D*P/16 items
    we want intersect (Parents NOT IN ( P set intersections))
    a set intersection is O(min(len)) 
    so its min(D, P)*(DP/16)
    then we add it to all the sets where we have 1s which is P ops 
    which makes the whole thing
    `D*(min(D,P)*(DP/16)+P)`
    per item


//0 tracking
Dth node 
    16 sets on average having (16-P)/16 items
    we want to intersect P sets (where our new item's 1s are)
    then we add this node to 16-P sets
    `D*(P*(16-P)/16*D + 16-P)`

lets test some values : 
p=2, d=4 : 4*(2*.5+2) = 12 , 4*(2*(14)/16*4 + 16-2) = 81 (64 of which is just adding it to sets).
p=4, d=8 : 8*(4*32/16+4) = 68,  8*(4*12/16*8 + 12) 36*8=272
p=8, d=8 : 8*(8*4+8) = 320, 8*(8/2*8 +8) = 320
p=12, d=8 : 8*(8*96/16+12) =480 , 8*(12*4/16*8 + 4) = 304
and theres no point at p=16

since the basic assumption of even pursuing stacking is that the nodes are sparse, it makes sense to pursue
1 tracking since it works better when nodes are sparse. 

during iteration we'll maintain a parent set, and a set for each position in occupancy mask containing the addresses of nodes (with available space) with 1s at that position. 


## Use size::MAX instead of 0 as sentinel value
that way we don't have to insert a default just to have a valid structure. 