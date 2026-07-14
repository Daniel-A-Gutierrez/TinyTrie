# Structure
The most recent top level entries are towards the top.

# Review

## Block Module
Why is there a try insert at? 
ah i see its reused for before and after, but all 3 take a physical address, which a caller shouldn't know. 
Fix : before and after take virtual and map it. 

Strategy for some reason is mem::taken, then the 'handle_insertion' is given to the strategy.
The strategy then gets to run a post insert check, gets put back , and we return. 

Ah, the ai was worried about insertion triggering a strategy change (ie pluripotent). 
Thats not a concern for the block level api, the arena does that based on the NotFound variant and other things.
Fix : need to read strategy module first

## Strategy Module 
block comment at the top is a lotta yapping.
insert budget is const rn, not sure about that
    an auto consumer shouldnt care. the strategy would though.
    a block level consumer ... might? 

assert_cap_pow2(cap) is probably overhead , if its constrained to debug asserts thats fine for now. 
instead it should be verified by tests... maybe we could make a Pow2<T>(usize) that has T impl bitshift etc. 
so we can guarantee it statically.

The InsertDelta<T> type seems pretty incomplete. 
I suppose the expected 'readdress' flow is find_slot -> fail -> bounce back 'address exhaust'. 

There's not a shove variant either, for when a item is shoved from one block to another. 

Fixes : 
I think strategy exists at the wrong level. Right now block.insert -> strategy -> feedback & operations
I think block stays more primitive, just block.insert -> operate & feedback
arena.insert(self,block_id) -> read strategy from block -> operate -> feedback. 

### impl Block Strategy
Each seems to store its budget
we can set it
they handle insertion, removal, and do a post_insert_check

handle removal has a default impl 

#### handle_removal 
return early if block is fully dense, no point in shifting to stride.
if removed value was on aligned slot also no-op
first-right is allowed to be anchor it seems - wait no that returns early
right_in and left_in do a bounds check for our first values
look to see if a neighboring aligned slot is some, if so we can shift it towards the removed slot to free up 
the none. 

### Growth Strategy
each struct independently impls new_block? why not have this on growth strategy? 
all they take is cap as an arg
Fix : that

### Default handle insertion
right now none of the strategies actually do anything special they all just call this. 
It starts from the result of find_slot :: Found
if its append or prepend we do push_front or push_back
prepend is pretty underdeveloped right now, it doesnt insert None on stride like append does. 
FoundAt(phys) currently doesn't do any sort of shifting. it just says 'free' and gives it back. 

Fix : make it symmetrical. 

### Post insert check
currently does nothing.
Fix : i dont think this is necessary? 

### Comments
I think a lot of these are ... practically wrong/useless, some are good. 

# Interfaces

as for the block primitives
try_insert_before(idx)
try_insert_after(idx)
split_end(&mut arena, block_id, range, split_strategy) -> (block_id)
split_mid(&mut arena, block_id, range, split_strategy) -> ([block_id;3])
shift(change to addr_shift)
spread() //moves every item to phys<<1, doubles v_offset. 
remove(idx)
get(virtual)

//not sold on these 4, but something like it. 
range(pos, amt, dir) -> iter //over a rage of addresses. not reversible. 
range_mut(pos, amt, dir)
cursor() //freely positionable 
cursor_mut() //supports try_insert_before and try_insert_afer 
iter() //never yields an element twice. 
iter_mut()

Use through arena.blocks[i] exposes the raw interface, 
arena itself holds the methods for automatic handling, and stores a queue of the last say, 16 insert hints given. 

Arena meanwhile has 
insert_before(block_id, block_idx, val)
insert_after(block_id, block_idx, val)
remove(block_id, block_idx) 
get(block_id,block_idx)
iter() //supports fwd, rev
cursor() //supports seek, advance, etc.
range() //same as for block

Summary of nontrivial insert cases
- block is full
    - auto : split depending on strategy
    - manual : reject inserts
- out of addresses 
    - cause is append/prepend heavy work in random block, or prepend in append, append in prepend
    - for manual, either split the block or readdress. 
- block not found in budget
    - theres a dense region, caused by strategy misalignment
    - auto : resolves based on strategy by splitting/growing/spreading
Optimization Misses : 
- sequential block has floaters at the hot end prevent an easy append/prepend on the block
    - auto fix : shove items off into another block 
- sequential inserts in the middle of any type of block except pluripotent
    - auto fix : carefully split a pluripotent block out of the middle

Things that can go wrong : 
- couldnt find a space nearby (within budget)
    - auto : strategy dependent, assuming were not out of address space...
        - append : depending on location
            - beginning (Front) : not possible, would just be a push_front if we're not out of addresses. 
            - middle : split_mid at position, would help to know if the last few inserts were sequential or random.
                - if sequential : pluripotent block only gets a few addresses around the last insertion site. 
                - if random : pluripotent block gets half a halfptr::max addresses centered on the middle of the last insertions. 
            - end : not possible, would be a push_back if we're not out of addrsses.
        - prepend : mirror append
        - random : 
            - beginning or end : split off pluripotent block from last several elements. 
            - middle : 
                - occupancy high => 
                    - if cap < max grow & spread
                    - else split in 2
                - occupancy < 75% => look at prior inserts for pattern
                    - sequential : split off pluripotent with a few elements near last insertion
                    - random : bad luck, 
                        - if cap < max : grow & spread
                        - else : split off a random block from the dense region, spread it. 
        
        - pluripotent : budget is the entire block so it cant fail. 
            - limited address space and max size to halfptr::max
            - math guarantees we can do halfptr::max appends/prepends before we hit len=halfptr::max
            - middle inserts push elements off the end to other blocks, but appending and prepending are always legal.
            - budget has to be the whole thing so that prepends/appends at any spot can clear out floaters. 
            - we have to trigger growth on len > 3/4 cap.
                - if cap >= halfptr it grows and changes strategy
                - else grow & spread & shift down 1
            - initialized with at most 1/4 halfptr::max() elements (64 for u16). 
    - out of address space 
        - append : (hint location)
            - beginning : split off a pluripotent node from the frontmost say, 64 elements. 
            - middle (has to be near end to trigger address space) : split off pluripotent node from hint
            - end : split off empty pluripotent node from end
        - prepend (mirror append) 
        - rand : look at occupancy
            - high : split in 2
            - <75% : same as above case i guess , look at prior inserts for a pattern, (sorted or rev_sorted), 
                - if there isnt one just split off a pluripotent node near hint, give it like 128 elements.
                - if there is one try splitting off a pluripotent node with a few elements near the hint location. 
    

One alternative method for remedying 'slot not found , low density'  : density inversion, or hole punching
density inversion : 
    cant be done in place. 
    if src[0].is_some{dst.push[src[0]]}
    for i in 0..src.len-1
        if src[i+1].is_some() {dst.push(src[i])}
        if src[i].is_some() { dst.push(None) } //kinda like a spread, but sparse regions get dense.
        //growth = len - occupancy, so new len is 2*len - occupancy instead of 2 * len for spread. 
hole punch : 
//take everything up to or after idx, move it left/right by amt, leave None's where it was. 
//more of a tool for the manual block managers.


# An idea about nibble tries integration
1. itd be better to call structures that make a single block a subtree , a forest. 
2. if we preorder the nodes, 'leftmost child' descent just becomes a linear walk while prefix_len is decreasing. 
3. in a forest, inodes could store u8s and leaves could be flattened and store (block_id), pointing at 0.
4. whether a inode ptr is a pointer to a leaf or not can be indicated based on the relative positions. 
    If the nodes are preordered its impossible for them to point backwards, so ptr <= current implies leaf ptr. 
    Still need to store 'terminal'. If we're avoiding the 'leaf' ptr per node, (really stingy ngl) we could try enforcing leaf=current for terminal inodes. 
    Nah that all doesnt get us enough space, except the terminal one. A single 2 tier tree with 17 nodes could fill up a 256 slot leaf node array. 
    If they cohabitate with the inodes we get the same enum problem we have right now, but it saves a read/hop. 
    we could store 4 leaves in an fnode plus parent pointers, for 20 bytes and a branch per hop. That probably fits in an inode. 

## DOA Block_Idx type needs sign
I think i need to actually go back to signed. Otherwise when we prepend and hand back the new virtual address... whats it supposed to be? 
Even if i increase v_offset, 0 was already given out, we're not repointing it, the new one has to be -1. 
So we use signed ints , just not with wrapping. 

## NonGrowing Blocks
start at max size, no v_offset map or addr_shift on lookup, we just track those on insert to decide where to put the new item. 
better lookup performance, more aggressive memory utilization, blocks can still learn they just never resize/spread. 
repoints are still done on splits, iteration performance will be horrible for low fill random blocks. 
i could call this 'SOA' instead of 'DOA' lol.

## Find Slot
address limit is the hard wall, capacity limit is a 'push_back/push_front'. 
Our search is aligned-stride * budget..aligned+stride*budget
out min/max WOULD be 0,len , but if those are at the address limits we don't want to bother. 
0 is at the limit if v_offset==PTR::MAX + 1
len is at the limit if buf.len()-v_offset == PTR::MAX + 1
so if we set variables OVERFLOW = PTR::MAX as isize + 1 , 
our left bound of the search is the max between (aligned+budget*stride) and minimum assignable address, which is the physical address for PTR::min(). 
    since PTR::Min is a power of 2 its aligned to any none_stride thats a power of 2 with magnitude < it (any practical case).
    its only assignable if v_offset < -(PTR::min() as isize)
    -V_offset
ugh this is ugly. 
just precheck these in the append / prepend cases, have them fail if that address space is exhausted. 
our rightbound is the min of aligned+budget*stride and 
im thinking for simplicity and correctness's sake, the block uses isize/usize but enforces that the PTRs it hands out are representable as PTR. 

search outward from align(position)+1 both left and right stepping by stride up to budget times
if left or right hits the end of the address space , it returns none
if right hits the end it returns append
if left hits the front it returns prepend
if left or right finds a None , it returns the position of the none

with less branches : 
max = min(align(PTR::MAX-stride)+1 , aligned.saturating_add(budget*stride))
min = max(align(PTR::MIN+stride)+1+v_offset, aligned.saturating_sub(budget*stride))
let left = min..aligned.iter().rev()
let right = aligned..max.iter.rev()
let longer = left.len > right.len {left} else {right}
//eh, lots of setup for a couple searches. lets just go one at a time. 
//and check the result after. 

right = aligned+1..align(PTR::max).step_by(stride).take(budget)
r_res = right.position(|i| buf[i].is_none())
left =  ..aligned.step_by(stride).rev().take(budget)
if v_offset == PTR::MAX + 1, {left.skip(1)}; 


# Lookup math
Going to just use a vecdeque instead of a circular array but i need to nail down the math to preserve addresses. 
Currently push_front makes the new item 0 and everything after it goes up by 1.
I need to store the number of negative elements i guess and offset the virtual address by that amount. 
so if i push front 3 times, virt_offset is 3 , and the expected virtual address is -3? 

How does address overflow/underflow work? 
Say i never prepend, append up to i16::max, len overflows i16 to -32768, offset is 0, but the direct byte representation is legal as a usize isnt it? 
But with a virtual offset if i DO prepend now, it becomes ambiguous? Furthermore iteration breaks doesnt it? 
Alright thats fine i guess. 
No pushing past ptr::max(), it requires a repoint. 
Lets call that address wrapping. 
Our repoint in this case would be just moving virtual_offset to some big negative number, then telling all the ptrs to add that, so the address space 0-32767 -> -32768->0
or the reverse if prepend is the concern. 
Maybe we just use u16s instead and start with offset 32768 -> 0? 
Different strategy configs can use a different virtual offset - append could be 256, prepend could be 65380, random could be 32768.  

What about when we add a negative number that isnt a prepend? 
If its a move its a move, we tell the arena owner some items were moved. 

So our lookup stays as lookup(virtual) -> buf[(virtual - v_offset)>>addr_shift]


# More ideas
if the 0th position represents a root then its fine to store something there, because no other nodes can point to it. 
thats an optional optimization that can be provided

it also occurs to me, insert can return an enum

InsertDelta {
    Moved (new, amount, dir),                   //increment or decrement following/preceeding amount PTRs.
    BShiftLeft (amount),            //rare case when bias changes Sequential -> Random . applies to whole block.
    BShiftRight (amount),           //rare case when bias changes Random -> Sequential . applies to whole block. 
    BlockSplit (left_block_id, last, right_block_id) //with circular arrays a repoint per arena PTR isnt necessary, only per block ptr. end stays left, everything after goes right.  
}

it also occurs to me, if the things in a block point to eachother (common case), if the arena is provided a function to get a &mut[PTR] to the internal PTRs of whatever is stored there, it can handle readdresses
and shifts internally. 

Also a clever tree structure could store 1 block pointer per subtree and limit the subtrees size so it always fits in 1 block. That way when a block splits instead of having to update
block_size ptrs it only updates 1. 

Actually nah , thatd require a bonafide circular array instead of my 2 vec solution. [0,2,4,6] -> [0,2] , ![4,6] .
The right block either leaves half of itself empty or repoints. 
Honestly thats a pretty strong motivator for a sort of circular array - no repoint on block split. 
It doesnt even need to be a 'real' circular array, we just need a start virtual address offset. For the example above the right buf's would be 4.

So address translation becomes (virt + virt_offset) >> addr_shift
If we do that, we can actually recenter our data when we spread, so it takes up equal space in rev and fwd arrays. 

there'll be a neat trick with the offsets in sequential heavy workloads - if rev or array are untouched , instead of splitting we can just swap the vecs and offset by -last. 

also, to ease the 'whole block readress' ing , i can provide functions that take an extractor that yields PTRs to elements within the block and apply the bitshift internally. 
Alternatively, i can provide a 'block_iter' that just iterates over the contents of a block, and the consumer can apply the shift. 

Double alternatively, blocks always stick to the strategy theyre born with but they dont inherit them from their parent - 
their parent decides what their strategy will be based on the behavior that creates them. 

No, i dont think i can get away from it. Datastructures utilizing the arena should be careful about how they handle cross block references.
If there's parent pointers, each thing in the arena needs to fixup each of its children, and we can follow the pointers to fixup this blocks item's parents.
If the datastructure puts no bounds and stores (block_idx, arena_idx) whole in its items, worst case scenario we have to scan the whole arena and fixup everything that points at this block idx. 

Actually its worse, when a random-block splits out a append-block, it'll also need to do a bitshift on the parents, if it shifts elements into the append block.
As a last alternative, we just leave the address-exhausted mini block as a stub.  
Or maybe we don't even wait for a realloc. If we try inserting at the end and cant find space in our budget, we immediately make a child block and put it there. Same for prepend. 
also whats our budget? ptr_width spaces separated by 2s? 

## Utility Functions
remap_internal //update block-internal pointers , requires extractor
update_parents //requires parent pointers or a means of deriving the parent from a node
update_children //updates parent pointers of children in arena

hey if the 'root' always lives at 0, and that position can be referenced from outside the block, it wont be affected by shifts so thats free. 
neat. 
makes me wanna get rid of the nonzero ptr thing. 

so basically if you guarantee the only inbound pointer to a block is to its 0th element, you get the invariant that insertions to this block won't affect any other existing blocks. 


Ok so recap : 
Blocks have utility functions for making remapping pointers easy
0 ptr is valid, and should be used (makes strategy switch cheap)
the data handed back by insert has been decided
blocks are going to store a start offset 
maybe the consumer should have a way to manually split a block? 
Actually i guess rather than 0 never moving its whatevers at block.virtual_offset. Have to persist that across shift changes. 
    for example, if idx 128..256 is split out with 128 as a new root for a new block, we dont want to repoint all of them, so the new block has virtual_offset -128 even though its physical position is 0.
    whatever pointed at it now points at block_x, 128. I think that means we can't move it or shift it over either. Or when we do, do we shift virtual offset? No that ruins everything else besides root. 
    I think when we insert our cursor just has to refuse to cross 0 when looking for open spaces. 
    Or maybe thats yet another insert function, or a flag, whether its ok to move the root or not. 
    Because thats desireable for a linked list - itd just wants to point at the minimum element. 
Trees stored with preorder can be split out simply with a range. 
So the interface might look like arena.blocks.split(new_root (vaddr)) 
So should blocks not split automatically but instead return an error on fail to alloc? 
Or maybe we just have arena.insert, block.insert, block.try_insert. Consumers that want granularity can have it. 
Try insert doesn't split, it returns an err if theres not enough space, whereas insert does it automatically. 

## Ways this might be used 

Nodes store (block_id, arena_idx)
    - doesnt care about block boundaries, just inserts. 
    - readdressing *expensive* , probably prefer to avoid it and just have stub blocks, unless there's parent pointers. 
        - even so , they lose the invariant that changes to one block don't affect others.
Only roots store block_id, descendants store only arena_idx and borrow block_id from ancestor
    - wants to control block splitting , 
    - readdressing requires an extractor but not too expensive, 
    - may use parent pointers - if they do, moving becomes something that can be done with extractors and utilities

## Pseudocode

Arena
read
insert
iter
blocks

Block
insert -> Enum(Moved,Readdressed,Split)
get -> &T
get_mut -> &mut T 
remove
try_insert //dont autosplit  
try_insert__fixed_root
force_split( at )

//block utilities

## Readdressing strategies
- random -> append
    - pack everything left, set addr_shift to 0
    - make a new block to the right with append strategy (this one becomes a stub but no readdress necessary)
    - move the few elements on the left right (moving as few as 7 elements frees up 7/8 of the space).
        - still, altering addr_shift means we either repoint or we move the items. 
        - also it doesnt help for the strategy unless we move virtual_address , which requires a repoint anyway.
- append -> random
    - no way around it, need a repoint to everything pointing to items in this block because we're going to massively increase addr-shift. 
    - perhaps append should, once in a while, insert a None at the end just so if we do hit 1 random insert its unlikely to cause a strategy shift. 
    - we can even store the none stride as 16 so its quick to find an open slot for insertion, since theyll be 16 aligned if theyre not taken. 
    - in the case where that doesnt work we can increase addr_shift by 1 and spread as we double cap. Repoint + spread, very expensive. 

I want to try to have blocks grow to their max size, or at least half of it. 
I think append blocks should start at 0, and when they exhaust address space, rollover into reverse. 
To avoid the reallocating and copying, itll immediately resize rev to its max addressable size. 

I think rationally it also depends on the size of the current block- maybe at some point its 'too expensive' to repoint.
So when block is small - readdr, change strategies
When block is large - just make a new one (random->sequential only). Or maybe 
in the append case we can split it in 2 and spread both. Biig repoint but the left half will avoid it in the future. 

if a consumer wants to never repoint , we just eat the n^2 insertion and make a new block when we run out of address space. 
This feels like a try_insert thing - basically if its 'free' do it, if it requires a move, split, or readdress, thats a different return type, and the action is not committed.

so then, insert has a bunch of fucking variants that give the user control over various facets of how it works. bleh. 
all this is handled easily internally with parent->child ptrs and extractors but thats overhead. 

Maybe for readdress we just say 'this block changed strategies , heres a function that maps ptrs to items in it to their new value'. 
It presumes the ability to iterate over things that point into this block in order so hopefully thats cheap but its the consumers problem. 
If they only point to root then the function will do nothing.

Insert default behavior : 
    move, shift, alter strategy, dont move root of block
    tell the caller afterward what was done and the address of the inserted thing
insert< const move_root = false > 
    maybe i can use const generics to parameterize the behavior of insert
try_insert
    don't allow moves, shifts, anything, if there would be side effects we err.
    If its free, insert and return Ok(place)

Other strategy : pluripotent.
addr_shift + log2(cap) = ptr_width/2 . 
So for a u8 with cap 4, wed be 0,4,8,12 , then 0,2,4,6,8,10.... 
Basically, we wont run out of address space until we have ~1-2*sqrt(MAX) elements. At that point, based on the number of appends/prepends we got, we decide what strategy to adopt. 
Another neat thing - this captures the overprovisioned pointer case. a i32 will be able to stay pluripotent and maintain good insert for random and sequential without ever triggering readdress. 
It just needs to adjust addr_shift when it spreads. 

lets try and nail down these biases. 

Append : 
repurpose rev as 2nd half once fwd fills (use virt_offset)
insert 1 None every ~ slots
addr_shift is 0

Prepend : 
When rev fills, first points to the end of the fwd array, prepend moves it back. 

actually both of these are the same, we just have a first and last, then use wrapping add when we append/prepend. 

recap ; 
pluripotent strategy type
insert is generic over some const params (moveable root)
try_insert only inserts if its trivial, otherwise returns an Err Enum with the necessary operation before doing anything.
block exposes 'split' 
    still dont know what that looks like
block has helpers for repointing that use callbacks on the items to get pointers

plan : 
complete circular double ended array
write functions that perform shift/pack
write insert strategy functions (sequential, random, pluripotent)
    actually, i think these are just different initializations, the options and parameters are the same. 
Actually in random, we could potentially save the 'addr_shift' change until we actually run out of internal storage, and insert a None on every other end hit when appending.
That lets us reuse the 'none stride' that append/prepend would have. 
Ok this is seeming less insane!


# Refinement v3

Sparse/Dense/Packed, Technically its Dense not sparse or continuous

insert takes a position and attempts to insert an element such that the ordering would be preserved if it were inserted at that position.
Insert(hint) -> iter< item=ptr > { left_end, left_start, right_end, right_start, current }

Adaptive Structure - blocks are initially optimized for random insert, but repeated append/prepend shift layout and addressing to favor it.
Logical addressing - Initially address space is utilized fully - for u8 with cap of 16, our stride is 256>>4. 
When capacity increases, stride decreases so that we can utilize addresses between the previous ones. 
maybe address scale is a better term? 

Block< PTR > { 
    capacity : PTR,
    addr_shift : u8
    ...
}

block.new() {
    capacity = 1;
    addr_shift = PTR::width() ;
}

block.grow( append : bool) {
    capacity <<= 1;
    if (append) {
        addr_shift >> (addr_shift.trailing_zeroes() >> 1)
        //every pointer now needs remapping. 
        //address space linearizes in 4-5 steps instead of 16. 
    }
    else {
        addr_shift >>= 1;
    }
}

block.phys_address(logical : ptr) -> ptr {
    let phys = logical >> addr_shift
}

block.logical_address(phys : ptr) -> ptr {
    logical = phys << addr_shift
}

so assuming we spread : 
cap = 1, shift=8 (u8)
addresses [0->0]
cap = 2, shift=7
[0->0, 1->255]
cap = 4, shift=6
[0,64,128,196]
cap=8, shift=5
[0,32,64,128,160,192,224]

so the relation log2(cap)+shift=ptr_with is preserved. 

if log_cap+shift > ptrwidth
cap = 4, shift = 8
[0,256,512,768]
we overrun our address space

if log_cap + shift < ptr_width
cap = 4, shift = 0
[0,1,2,3]
we run out of physical space first

when shift changes, things need to move. 
0. When it grows, thats smooshing elements to the left, possibly colliding them. Its possible but it requires a mass repoint, unless we can guarantee there were no collisions.
    - when can we guarantee that? in the cap=4 shift=6 case, if cap stays 4 but shift=>7, we'd have to guarantee that 64 and 196 were empty...
    - maybe when len< cap/2 , we do a 'preemptive repoint' and move things off odd even indeces? 
    - the iter we give back needs to yield None for ptrs that werent moved, or Some for the new placement, and scans over the entire block. 
    - then we can shrink shift and condense (phys = phys/2). 
1. When it shrinks as cap grows, the elements can be spread across the new cap (phys=old_phys*2)
    - we can repoint requiring a repoint ST ptr=ptr>>(shift_delta)
    - if we have extra address space, we can do nothing. 
2. If it doesnt change but cap grows, we don't need to do anything. 
3. If it shrinks more than cap grows, the ptrs need to be shifted by the excess. 

So what precisely is the invariant we're maintaining ? virt=>phys remains the same? 
So when shift increases either the element moves or the pointer does... but that doesnt capture the condition where we do nothing because we have overprovisioned address space.
- virt=>arr[phys] remains the same, 
- max_block_size >= cap
- log2(cap) + shift = ptr_width  //optimal for random insertion, not a hard rule
- log2(cap) + shift <= ptr_width //if its too big, we can't address cap. but its fine if its smaller, optimal for appending. 
- log2(max_cap) < ptr_width //unable to address max_block size. 

So say in 3's case, we had a balanced cap4 array (shift=6 for u8), and we increased cap, but halve'd shift. 
196 used to point to the last (phys=3) element, if wed decreased shift by 1 instead it'd be the second to last, but now with it being 3, 
3<<3=24. 
The old indeces were
[0,64,128,196]
Our arrays new space is  
[0,8,16,24,32,40,48,56] . 
Shift decreased by 3 so all the ptrs <<3 .

I see so the spread compensates, instead of having to repoint we just double the physical index instead, thats why the math works out. 

If we overprovision our pointer, or arbitrarily limit the max size of a block to a smaller ptr's max value, we can potentially dodge having to repoint when we're out of address space. 
Say we limit the blocks size to u8::max() but use a u16 ptr, we can start with shift=8 and still append freely, just not increasing shift when we append and our block indeces will be [0,256,512,...u16::max()-255]

## Triggering Adaptive Shift

Lets just have block track the number of appends since realloc. 
If its > current_cap/2 , realloc/block split will favor an append heavy workload. 
Lets also set a general policy that blocks always fill up to block_max_size. 
We can have different initializers for a new arena that start with a block biased one way or another. 
Blocks inherit their parents bias. 
We can adapt towards a random insert strategy if appends are rare and log2(cap) + shift < ptr_width-1 . 
in that case, we know we can increase addr_shift by at least 1. 
For example, [0,2,4,6] -> [0,4,8,12,16,20,24,28] as shift 1=>2 cap 4=>8. 
This requires a repoint where new_ptr = old_ptr << delta_shift. 

# Refinement (v2)
Semicircular Array
Similar to a circular array but with the bound that begin <= 0 and end >= 0. 
Capacity originally uninit, begin/end point indicate range of initialized memory. 
Lookup : signed int -> unsigned int -> address (dont have to re-calculate signed form unsigned for iteration)
Generic over max capacity, capacity doubles when abs(begin) + end >= cap. 
Sparse by design, first and last point to terminal non-none elements. 
Supports append,prepend,insert.
Indices not necessarily sequential. 

Actually would it be simpler to have 2 vecs ? A fwd and reverse , one for positive ints one for negative, prepending is just pushing to rev?

Various methods for doubling capacity. 
- realloc 

## Insert semantics
Maybe 'insert' can default to 'insert before' , but insert(len) is valid. 
We scan left by stride from the given position, then push the elements back a space once we've found one, update ptrs and return the located spot.
The crux of this insert business is updating the pointing structure with the new addresses, as well as the address of the newly inserted element. 
If we return some mapping , the new element has to be placed first, so itd be (ptr, &[(ptr,ptr)]). 
If we instead take a cursor in, we do the modifications internally. 
Aaallternatively, we could return an iterator over the altered pointers... i like that. All we need is start,diection,amount. 
Of course, we don't know the old ones, so the pointing structure also has to be ordered. 

anyway, push, insert(hint) 
insert's hint can be an existing ptr, or ptr +- 1. 
If buf[ptr] is some, the new element will take its spot. 
shifted elements go in the direction specified by the returned iterator... nah they cant. 
They have to go right. That makes the iterator tricky if its reverse. 
What if, insert always returns 2 ptrs + the iter - new thing, old thing, other things. 
The ordering could still be wrong though, insert at pos implies new< old, so it has to go right. 
Say the closest open spot is 10 spots left though, to fit the new one in 9 elements move left to make a spot the space before PTR, we use it, old doesnt move. 
So we don't have the invariant 'new thing takes old things spot'. So we dont need to return old thing, everything around shifts in the same direction. 

so insert(hint) -> ptr,iter< ptr>. the new location may be hint, it may not be. 

Actually then if i return an cursor< ptr> just at the new items location with direction and amount, that should be sufficient. 
That means insert can only do shifts though , it cant spread. 
A complete interface would always need to return 2 iters, or rather, a proper double ended iterator that advances the front and back separately. 


## Layout Strategies
How we use our address space and how we distribute items depend on how a block is populated. 
If we populate it by pushing, we never spread, we only realloc. So when insert triggers realloc, it needs to check how. 
I can have trees inherit the stride of their parent... address stride and null stride? 
When we trigger a realloc, if it was from append/prepend we increase null stride and decrease address stride. 
Do the opposite if it was from a insert in the middle of a block that couldnt find space quickly enough. 

null stride - when we spread we put 1 None element every 'null stride'. MIGHT NOT USE THAT THO
address stride - might be a better approach.  physical address = cap / (address_stride).
when cap doubles, physical address doubles, unless address stride also doubles. 
So when we want to leave the new memory available for append/prepend, we don't also increase address stride. 
Otherwise if we're spreading and reallocating, we DO want to leave gaps in the physical memory, so we spread the items out without
having to update pointers. 
Say our initial 'max addr' is 256. 
If we ever update it, we have to repoint everything in the block. 
Thats an option if we run out of address space and our block is still small.

For a block with 4 entries, the indexes would be 0,64,128,196 - that leaves us 64 appends before a u8 runs out of address space. 
starting from 2 itd be 0,128 . And 1 will always be an append/prepend, so i don't think we spend address space in that case. 

So basically, when we add to an extremity we modify cap and address_space (like that name better) in tandem, when we fill in the middle
we move elements but don't modify address space, only cap.
If we try to append and cap is at its limit but address_space isnt, 

Actually if we halve address stride without increasing cap, we have to repoint but we free up half our address space. 
Say our pointer is a u16, but we start with an address space of 255. 
As we append we dont increase address space, but cap does increase, so our actual indexes go to 512, 1024, 2048 up till we hit u16::max at 256 elements. 
At that point, we don't need to just realloc, we need to repoint also to shrink address stride. 
However up till that point, we didnt have to shift, spread, or repoint. 

Say on the other hand, we spread each time due to middle insertions filling up the array. 
We haven't needed to repoint though, until again, we run out of address space at N=255. 
If we dont use address strides lookups a bit faster, but every spread also repoints, however append/prepend never repoint. 

Its probably sensible to have address stride start at its max value, but adapt quickly if we get lots of appends/prepends. 
Better to repoint and get to a stride of 1 when theres few elements in the block. 
So, address-space starts at ptr::max but from each append/prepend past a size of say, 16, we square root it and repoint. 
If we happen to run out of address space down the line, we can always repoint.
Man my naming is terrible but im sleepy. 

We should also re-increase it aggressively from middle inserts. Append/prepend heavy workloads should only get optimized for if theyre 99% of the load.

come to think of it, btree balancing can also take advantage of that. 
For example, if we know our keys will be strictly increasing, (actually a sorted vec would make the most sense) , but anyway a btree could leave the right/left half empty when it splits and raises its root node. 
You could even do it on a per-node level and let rebalancing handle the 'edge case' of a middle insertion. 




# sparse ordered arena

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