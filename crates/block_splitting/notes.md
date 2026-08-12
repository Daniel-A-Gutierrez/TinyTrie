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

# Rotating Non Max Cap Blocks
for context - we're using 4 bit ptrs
8 is the max size for shift = 1, since v2p(8)=v2p(0). but, a block cant reach len=9 while shift > 0, not by growing and spreading. shift = 0 => len = 16.

so fundamentally, `len << shift` cant overflow. if it does we lose bits we'd otherwise need to rotate so rotation doesnt work.

