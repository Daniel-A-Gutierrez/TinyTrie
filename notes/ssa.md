# sparse sorted arena

- double ended, store 2 vecs, negative indexes get inverted to usizes and go into rev
- max size, indexable by PTR const generic. 
- 0 is invalid, so consumers can use Option< Nonzero < PTR >> to point into the arena
- realloc threshold, target_density
- when we insert and run out of space at one end, create a new vector, insert into it. 
    - maybe we have a 'tail' vec for both fwd and reverse. 
    - i think the problem is that our realloc/spreading is controlled by a single insertion, we're not balancing that out
    - how about we trigger spreading when we traverse some trigger number of elements and cant find a space? 
        - also, inserting at the beginning or end first tries shifting a start/end ptr. 


# BLOCKS
the arena is a Vec< Block > 
a ptr into the arena takes the top half as block_idx and the lower half as item_idx - halfptrs
blocks store { Vec< buf > , first, last, beginning, end }
A blocks max capacity is halfptr::max 
a block gets its full address range, but signed (sorta) - -32756 to 32756 including zero. 
we only use negatives when prepending, to avoid having to shift pointers of subsequent elements. 
we may have to realloc when we run out of address space - but even so, the memory won't be wasted, because the block won't grow in that case, we just make a new block. 

so append : increase end (unless we run into L or beginning), use new space
prepend : decrease beginning (unless we run into O or end), use new space.
insert : start from a position, skip by (free stride) looking for a vacancy, both forward and backward.
(disperse put vacancies on odd elements, no point in checking even ones). 

when we grow, block size always doubles, as do begin and end. first and last point to elements, not memory bounds.
spread can happen without growing, if the 'usable space' can double by moving begin and end instead of reallocing. 
spread is triggered when we attempt to insert but find no usable space within some amount of spaces, determined by the current len/capacity of the block.
grow is triggered when first and last need to be moved but overlap - spread can trigger this by attempting to double both, but append and prepend also trigger it by trying to move them directly.

we can possibly make the 'spread' adaptive, based on how often its triggered from an insertion vs how often a block triggers realloc from append/prepend. 
have a block store 'stride' . by default its 1, the minimum it can take. Make the max say, 16. when we realloc, if it was triggered by append/prepend, increase stride by 1. 
If it was triggered by a middle insertion, decrease stride by 1.
When we spread, we insert 1 None element every 'stride' . 
That way if we have a block thats completely dense because it was made entirely by append/prepend, we're not going to fill it with Nones that will never get used just because 1 insertion happened in the middle. 
It stays at 15/16 density, which is quite good. 
To be clear, the capacity still doubles when it grows, no matter what. 

Our Arena actually needs a Vec< (prev, Block, next) >, so we don't trigger cascading shifts when we need a new block. Thatd be horrible. 
The linked list is only needed when we're iterating across blocks, ptrs to a block still use its index in the arena. 

# insertion
ok, these need to go in order BUT we cant assume Item is ord. 
Our leaf nodes arent ord, for example, we cant compare them to find out where to put the item, the caller has to request a space. 
They also need to provide a iter_mut< Item = &mut PTR > double ended iterator to the pointing datastructure so we can update ptrs we shift. 
On insert we check if len=0, if so we just push, dont bother with the cursor and return. 
otherwise , we call next() on the iter to get the position we're going to be inserting before. 
I think maybe looking 'outward' first is the best approach? That way its symmetric and we have a better shot of expanding the usable space of the array. 

The cursor provided to us needs to yield a cursor of the same ptr... or we need distinct insertion methods.
Say something in the arena is pointed to by multiple things, to keep those up to date we need to update all of them, so 2 level iter. 
But if the structure guarantees that each item is only pointed to by at most 1 thing, we dont need a 2 level iter.

## Splitting
should blocks overflow into neighbors? 
When do we make a new block? 
I don't think we spread it, unless the split was triggered by a spread? 
So we make a new block, repoint half of the PTRs, copy over the items (on prepend, first half, on append, second half, on spread, second half but also spread both) to a new block, wire it into the linked list.
I think blocks *do not* overflow into neighbors. I don't think theres any benefit. New blocks arent made till the previous one was unable to split further. 
If it was made by append/prepending, itll be very dense and can basically be at a nice point where cap~=len, and it likely wont have to be inserted into anymore.
If it was made by random insertion, leave its space for it to fill. 


## Pointer Exhaustion
Im thinking instead of doing jank stuff like splitting a u32 into a u16 and a i16, we just have our PTR be (u16,i16).
The block vec use a linked list so u16 is fine , we can prepend without signed ints, but internally we want to be able to prepend in a block so we need i16.
Say we've exclusively appended into a block - itll be full from 0..2^15-1. We can't point to a higher address with an i16, so we need to split it, even if our max_size was u16::max. 
SO, if we set our max block size to i16::max, we need to worry about where we put begin and end in the new blocks after splitting - in append heavy work, it makes sense to put begin near i16::min, 
and the reverse for prepend heavy work. If neither, we try and balance it so begin = -end. 
We can tell if our work is 'append heavy' or prepend heavy by the value of stride, or the discrepancy of begin and end. 
In a block thats seen lots of appending the absolute value of end >> abs (begin), and stride will be high (anything above 3 tbh). 

## iteration
don't do a mod at each index, just do 1 or 2 loops 
we need start and end as their usize counterparts so 
let ustart = if start > 0 {start} else {len+start}
let uend = if end > 0 {end} else {len+end}
if start > end
    start..len , 0..end
else 
    start..end
curious though, given len is a power of 2, is mod cheap? 
is start>0{start}else{len+start} equivalent to start%len ? 

other than that we just iterate over the blocks, then the items in each block, till we hit block.next==None or block.prev==None, and we return None.

A tricky bit though - say we store our b+tree nodes in 1 arena, then leaves in another arena, and keys in a third.
For now i think we need a no-aliasing policy- each item in the arena can only be pointed to by one thing. 
In the nibble trie, leaf nodes and inodes abide that policy, its just keys that dont - many nodes may point to a single key. 
As for the 2d iter, perhaps just an iter over &[&mut PTR] would be best.  

## Stable Indeces on Repeated Spread
Idea for stable indeces across spread/grow - divide PTR by cap

1,2,3,4,5,6,7,8
spreads to 
1,0,2,0,3,0,4,0 ...

say our ptr type is u8, our cap is 4. 
The ptrs we give out are 
0,64,128,196

64>>6 = 1.  

what we want is i/cap, where cap is a power of 2
our stride is 2^max. 
since cap is 2^2, our stride is 2^8-2, in this case. 
when its 8, 2^3, our stride halves to 2^5 (32).  

there's a downside to this - previously we iterated through the block in sorted order and made sure everything went at an even space. 
The odd spaces will remain free, but odd spaces from before that werent filled will turn into even spaces that arent filled in the new buffer. 
Maybe worth it to avoid a linear scan over the whole thing, but each lookup is 1 extra instruction, a bitshift. 

This doesnt play nice with append/prepend heavy blocks either, youd run out of address space fast. 
Maybe we only do this when stride is 1 and we're growing from spread. 
Have spread's default be 3, so it takes 2 grows by spread without an append/prepend to take over. 

Maybe we need append_grow, prepend_grow, and spread_grow or something idk. 

## circular
I think we restrict 'begin' <= 0 to make the math simpler.
phys_idx = if i < 0 {len + i} else {i}