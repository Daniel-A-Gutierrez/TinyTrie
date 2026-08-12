# Wrapping Offsets
len=8, occ=8:
translator: inner=0 outer=0 shift=1 rot=0
[0, 2, 4, 6, 8, 10, 12, 14]

full (len=16, occ=16):
translator: inner=15 outer=0 shift=0 rot=0
[15, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]

v2p subs inner last, p2v adds inner first

wrapping implies a break in the ordering of elements - a cursor cant just go to "none" once it hits phys 0, itd have to wrap if we want to allow wrapping. 
also , spread from 8 to 16 with offset 1 introduces a wrap. 

i think any nonzero offset in either field creates wrapping. good to know.
Uniform will avoid wraps...for now, but ill have to work around them sooner or later i think. 
Find slot gets even more complex. So does none-slide. 

avoiding it means vaddr + inner_offset < LEN? nah. 


# Rotating Non Max Cap Blocks
for context - we're using 4 bit ptrs , max val 15 
8 is the max size for shift = 1, since v2p(8)=v2p(0). but, a block cant reach len=9 while shift > 0, not by growing and spreading. shift = 0 => len = 16.

so fundamentally, `len << shift` cant overflow. if it does we lose bits we'd otherwise need to rotate so rotation doesnt work.


# Waaait shit
The rotation where elements past midpoint go onto the open in-between spaces is wrong, it violates the ordering invariant.
0,128,1,129,2 ...
if at=129, then the left half puts 128 at phys 1, when in ordering it should be *after* everything there. 
so its insolvent. cant be done. 

we can maintain ptrs or maintain ordering, not both, unless we split at midpoint. 

splitting at midpoint is only realistic for leaf nodes, for trees itd require fixing root at midpont which is expensive because 
it causes whole subtrees to jump left to right when they split. 

the whole point of this is to maintain ordering, so we have to give up on maintaining pointers between at..midpoint 
but, to insert we need a valid tree in the block. 

the subtree is valid though through rotation after a split, its just...not ordered well.
but the items themselves get in the way of finding open slots to shove them into.
if at < midpoint and we're splitting there,
one thing we can maybe do is split right off from midpoint+(midpoint-at) instead, 
then dump (at..midpoint+(midpoint-at)) over and fixup the pointers. 
all the outbound pointers undergo a rotation, all the inbound ones get rewritten, all the internal ones get rewritten as well.

THAT is a job for a walker. 
We need other split primitives then - the current one that doesnt maintain ordering is bunk.
We need split_and_rotate_hollow(&mut self, at:usize ) -> `Block, Vec<(Option<T>,vaddr)>` 
and split_and_rotate_mid(&mut self) -> Block //splits at midpoint. 

# Back to wrapping
either we avoid it or lean into it. 
if we lean into it, find_slot and none_slide get harder, and the cursor impls don't get to know where to stop as easily.
We also cant hand out slices, since they may cross the wrap point. 

if we lean away from it, we need to figure out... how. 
pluripotent is the main concern. 
uniform never touches offset. 

so what we want to avoid is... well, just do the calc with isizes and see if the result is > P::MAX or < 0. 
or is it P::MAX >> shift >= len 
if i dont want inner offset to wrap then  (inner_offset + len ) << shift <= P::MAX... nah that seems wrong. 
phys < inner_offset ? len < inner_offset ? P::MAX >> shift > len + inner_offset ? actually that one seems good. idk about outer offset tho. 
i think that would be checked on push_back and push_front to prevent wrapping. 

nah we've just gotta accept wrapping i think, it wasnt very hard to get it working with splitting, and we also have split_and_spread now. 
as long as we had offsets wrapping was always a concern, its too tricky to work around. 

# Remaining experiments

successive fill + spread + split + fill + split 
- at midpoint (done already)
- restricted cap=8, first split should be a split_and_spread, successive splits should maintain cap
- restricted cap with inner offset
- outer offset != 0 (does it really matter?)
- split_and_hollow , splits around v2p(midpoint), excluding the closest n slots which get returned separately. 
    - this is necessary for when 'at' isnt a midpoint and cap = PTR::MAX + 1, otherwise the larger half cant spread via rotate and maintain ordering. 
    - the carved out portion needs to be manually fixed up by the caller before its added to one of the two children. 
