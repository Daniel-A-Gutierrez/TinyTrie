# Summary Of Optimizations And Their Impacts 

## BTree

Baseline : 
Random txt : 95B/key, 7M lookup, 560M iter, 2.7M insert

Random U64 : 67.1B/key, 5.7 Lookup, 32M iter, 1.8M ins

Interesting that fwd iteration was so good for variable length keys here.

Baseline includes simd, leaf node linked list

Node Rebalancing : 
Insertion (ru64) : 1.8M-557K ouch, fwd iter 32-23M, lookup 1.8M-1.1M, memory 67-58

Node Rebalancing 2 : 
Iter 22->32, lookup 1.1 to  1.8, insertion 557k -> 911k
Separate tree nav for read only code paths from insert.

Optimize fn on btree
Line 1252 
let old_next = self.leaves[child_idx].next.map(|nz| nz.get().as_usize() - 1);
        let old_next = self.leaves[child_idx].get_next();
May be responsible for a slowdown from 911k insert to 580k

Gap arena realloc
580ins to 530, fwd : 26M to 488M


Benchmarking overhaul 
Iter regression 480M to 240M
Memory from 59B to 36B
Insertion at 628K

Fix generalization so simd is used properly

insert : 1.55M to   2.73
iter : 235M
lookup : 5.3M
Insert : back to 1.38M


┌─────────────────────────┬────────────────────────────────────┬────────────────┬───────────────┬───────────────┬────────────────┬──────┐
│         Commit          │            What changed            │     Insert     │      Fwd      │      Bwd      │     Lookup     │ Mem  │
├─────────────────────────┼────────────────────────────────────┼────────────────┼───────────────┼───────────────┼────────────────┼──────┤
│ BTreeMap                │ baseline                           │ 3.64M          │ 46.28M        │ 45.85M        │ 3.39M          │ 27.1 │
├─────────────────────────┼────────────────────────────────────┼────────────────┼───────────────┼───────────────┼────────────────┼──────┤
│ reorg benchmarks        │ CTree debuts                       │ 1.80M          │ 31.5M         │ 29.9M         │ 5.70M          │ 67.1 │
├─────────────────────────┼────────────────────────────────────┼────────────────┼───────────────┼───────────────┼────────────────┼──────┤
│ node rebalancing        │ rebalance nodes in B+ tree         │ 557K           │ 23.0M         │ 22.8M         │ 1.11M          │ 58.8 │
├─────────────────────────┼────────────────────────────────────┼────────────────┼───────────────┼───────────────┼────────────────┼──────┤
│ rebalance optimization  │ tune rebalancing heuristics        │ 911K           │ 31.5M         │ 31.7M         │ 1.81M          │ 58.8 │
├─────────────────────────┼────────────────────────────────────┼────────────────┼───────────────┼───────────────┼────────────────┼──────┤
│ tiny_btree optimize     │ add optimize() + CTreeOpt          │ 581K / 606Kᴼ   │ 26.6M / 455Mᴼ │ 24.5M / 276Mᴼ │ 1.26M / 1.09Mᴼ │ 58.8 │
├─────────────────────────┼────────────────────────────────────┼────────────────┼───────────────┼───────────────┼────────────────┼──────┤
│ gap arena realloc       │ spread-on-realloc, adjacent splits │ 534K / 533Kᴼ   │ 488M / 486Mᴼ  │ 304M / 309Mᴼ  │ 1.26M / 1.17Mᴼ │ 58.8 │
├─────────────────────────┼────────────────────────────────────┼────────────────┼───────────────┼───────────────┼────────────────┼──────┤
│ benchmarking overhaul   │ bench refactor, add CTreeFixed     │ 629K / 570Kᴼ   │ 238M / 237Mᴼ  │ 236M / 233Mᴼ  │ 1.38M / 1.47Mᴼ │ 58.7 │
├─────────────────────────┼────────────────────────────────────┼────────────────┼───────────────┼───────────────┼────────────────┼──────┤
│ Generalized CTree       │ trait-based storage strategy       │ 629K / 570Kᴼ   │ 238M / 237Mᴼ  │ 236M / 233Mᴼ  │ 1.38M / 1.47Mᴼ │ 58.7 │
├─────────────────────────┼────────────────────────────────────┼────────────────┼───────────────┼───────────────┼────────────────┼──────┤
│ buncha benchmarking     │ testing different parameters       │ 3.70M / 3.71Mᴼ │ 734M / 826Mᴼ  │ 494M / 503Mᴼ  │ 4.93M / 4.87Mᴼ │ 22.8 │
├─────────────────────────┼────────────────────────────────────┼────────────────┼───────────────┼───────────────┼────────────────┼──────┤
│ update fanout           │ 16 way fanout                      │ 3.82M / 3.86Mᴼ │ 909M / 912Mᴼ  │ 524M / 526Mᴼ  │ 4.58M / 4.74Mᴼ │ 22.8 │
├─────────────────────────┼────────────────────────────────────┼────────────────┼───────────────┼───────────────┼────────────────┼──────┤
│ refactor into workspace │ workspace restructure              │ 3.82M / 3.86Mᴼ │ 909M / 912Mᴼ  │ 460M / 486Mᴼ  │ 4.58M / 4.74Mᴼ │ 22.8 │
└─────────────────────────┴────────────────────────────────────┴────────────────┴───────────────┴───────────────┴────────────────┴──────┘

the rebalancing was a big performance hit, let me try disabling it in the current code and see what that gets us. 

┌─────────────────────────┬────────────────────────────────────┬────────────────┬───────────────┬───────────────┬────────────────┬──────┐
│ test no rebalancing     │                                    │ 3.6M/3.7M      │ 600M          │ 560M          │ 4.57M          │ 28   │
└─────────────────────────┴────────────────────────────────────┴────────────────┴───────────────┴───────────────┴────────────────┴──────┘

Wow thats a dramatic decrease on the forward iteration, but not on the backward iteration... lookup unaffected, insertion unaffected. odd.


Words key type — search strategy comparison (1M keys, CTree / CTreeOptᴼ):

┌─────────────────────┬────────────────────────────┬────────────────┬───────────────┬───────────────┬────────────────┬──────┐
│       Config        │        What changed        │     Insert     │      Fwd      │      Bwd      │     Lookup     │ Mem  │
├─────────────────────┼────────────────────────────┼────────────────┼───────────────┼───────────────┼────────────────┼──────┤
│ with previews       │ SIMD preview               │ 2.49M / 2.04Mᴼ │ 450M / 480Mᴼ  │ 296M / 313Mᴼ  │ 5.38M / 5.02Mᴼ │ 50.7 │
├─────────────────────┼────────────────────────────┼────────────────┼───────────────┼───────────────┼────────────────┼──────┤
│ without (linear)    │ linear scan stored keys    │ 2.54M / 2.22Mᴼ │ 456M / 503Mᴼ  │ 316M / 303Mᴼ  │ 6.65M / 6.34Mᴼ │ 50.7 │
├─────────────────────┼────────────────────────────┼────────────────┼───────────────┼───────────────┼────────────────┼──────┤
│ without (binary)    │ binary search stored keys  │ 2.35M / 1.94Mᴼ │ 477M / 486Mᴼ  │ 286M / 301Mᴼ  │ 3.88M / 3.37Mᴼ │ 50.7 │
└─────────────────────┴────────────────────────────┴────────────────┴───────────────┴───────────────┴────────────────┴──────┘

Previews are a ~24% lookup regression vs linear scan (5.38M→6.65M). Binary search is worst of all (3.88M). Linear scan wins for short varlen keys at N=16.

KeyRef inlining (Inline+Owned, 24-byte KeyRef, no key_buf):

┌───────────────────────┼───────────────────────────────────────────────────────┼────────────┼──────────────┼──────────────┼──────────────┬──────┐
│ Config                │ What changed                                          │ Insert     │ Fwd          │ Bwd          │ Lookup       │ Mem  │
├───────────────────────┼───────────────────────────────────────────────────────┼────────────┼──────────────┼──────────────┼──────────────┼──────┤
│ with interning        │ BufKey interning + inline ≤14B                        │ 3.28/3.36ᴼ │ 644M/706Mᴼ   │ 87M/438Mᴼ    │ 6.27M/6.89Mᴼ │ 35.6 │
├───────────────────────┼───────────────────────────────────────────────────────┼────────────┼──────────────┼──────────────┼──────────────┼──────┤
│ no interning          │ Inline ≤22B only, Owned for long                      │ 3.14/2.96ᴼ │ 486M/443Mᴼ   │ 290M/196Mᴼ   │ 5.88M/6.77Mᴼ │ 45.2 │
├───────────────────────┼───────────────────────────────────────────────────────┼────────────┼──────────────┼──────────────┼──────────────┼──────┤
│ PackedKeySlots (init) │ PackedKeySlots, branch-free prefix scan; iter rebuilds│ 2.80M      │ 117M         │ 96M          │ 6.28M        │ 41.5 │
│                       │ Vec<u8> per step                                      │            │              │              │              │      │
├───────────────────────┼───────────────────────────────────────────────────────┼────────────┼──────────────┼──────────────┼──────────────┼──────┤
│ vec-per-node keys     │ Vec-per-node keys; iter ↑18×, lookup regresses        │ 2.54M      │ 283M         │ 266M         │ 5.36M        │ 38.4 │
├───────────────────────┼───────────────────────────────────────────────────────┼────────────┼──────────────┼──────────────┼──────────────┼──────┤
│ SmallVec              │ SmallVec for vlen keys; good perf trade, memory ↑     │ 2.32M      │ 245M         │ 236M         │ 5.12M        │ 44.2 │
├───────────────────────┼───────────────────────────────────────────────────────┼────────────┼──────────────┼──────────────┼──────────────┼──────┤
│ + reserve capacity    │ Reserve capacity on node split; lookup ↑ to 7.63M     │ 2.42M      │ 265M         │ 251M         │ 7.63M        │ 45.3 │
├───────────────────────┼───────────────────────────────────────────────────────┼────────────┼──────────────┼──────────────┼──────────────┼──────┤
│ + GLM optimizations   │ 9 easy optimizations (dedup/cache/allocs); lookup ↓   │ 2.57M      │ 250M         │ 250M         │ 4.95M        │ 45.3 │
├───────────────────────┼───────────────────────────────────────────────────────┼────────────┼──────────────┼──────────────┼──────────────┼──────┤
│ + recursive rebalance │ Recursive rebalancing (limit=3); big insert+lookup win│ 3.21M      │ 258M         │ 261M         │ 8.06M        │ 45.3 │
└───────────────────────┴───────────────────────────────────────────────────────┴────────────┴──────────────┴──────────────┴──────────────┴──────┘

Recursive rebalancing limit tuned: 3 is best (2, 4 slower; 10 far slower). Memory unchanged at 45.3...

Looking at the table it may not seem very worth it but i don't think the original 'intern everything' approach was good, it relied heavily on optimization and 
we had no way of doing our gap arena efficiently with variable length keys.
Also the initial bonus to lookup from reserve capacity was probably anomalous because the lookup codepath was unchanged. 


Adding 3 bytes of padding between keys to attempt to lower the cost of rebalancing : 


─── Insertion (keys/sec) ───
                               1000000
StrBTree                         2.26M

─── Lookup (keys/sec) ───
                               1000000
StrBTree                         5.47M

─── Iter forward (keys/sec) ───
                               1000000
StrBTree                       239.18M

─── Iter backward (keys/sec) ───
                               1000000
StrBTree                       205.93M

─── Memory (bytes/key) ───
                               1000000
StrBTree                          42.9

Definitely not good. Insertion way slower. 


## Nibble Trie

### baseline
─── Insertion (keys/sec) ───
                                   100           10000         1000000
NibbleTrie                      28.58M           8.42M           5.50M

─── Iter backward (keys/sec) ───
NibbleTrie                     137.27M         118.06M          53.03M

─── Iter bwd index (keys/sec) ───
NibbleTrie                     119.51M         112.24M          62.21M

─── Iter fwd index (keys/sec) ───
NibbleTrie                     163.16M         145.39M          72.16M

─── Iter rev index (keys/sec) ───
NibbleTrie                     197.93M         155.21M          63.21M

─── Lookup (keys/sec) ───
NibbleTrie                      86.57M          12.23M          11.73M

─── Memory (bytes/key) ───
NibbleTrie                        64.0           124.5           134.2

### undo stacking optimization

─── Insertion (keys/sec) ───
                               1000000
NibbleTrie                       7.52M

─── Lookup (keys/sec) ───
                               1000000
NibbleTrie                      12.84M

─── Iter forward (keys/sec) ───
                               1000000
NibbleTrie                      68.98M

─── Iter backward (keys/sec) ───
                               1000000
NibbleTrie                      59.85M

─── Iter fwd index (keys/sec) ───
                               1000000
NibbleTrie                      80.87M

─── Iter rev index (keys/sec) ───
                               1000000
NibbleTrie                      70.76M

─── Memory (bytes/key) ───
                               1000000
NibbleTrie                       121.6

good uplift across the board, especially on insert. 

### Implement Gap Arena for Index 
─── Insertion (keys/sec) ───
                               1000000
NibbleTrie                       4.28M

─── Lookup (keys/sec) ───
                               1000000
NibbleTrie                      12.63M

─── Iter forward (keys/sec) ───
                               1000000
NibbleTrie                     872.35M

─── Iter backward (keys/sec) ───
                               1000000
NibbleTrie                     413.71M

─── Iter fwd index (keys/sec) ───
                               1000000
NibbleTrie                     461.82M

─── Iter rev index (keys/sec) ───
                               1000000
NibbleTrie                     478.21M

─── Memory (bytes/key) ───
                               1000000
NibbleTrie                       186.3

iteration now rivals our optimized btree, and lookup crushes it. Memory usage is horrendous though. 
Unlike in the btree, the ordering of our index is strictly enforced. 
Thats probably a good optimization route for the btree too. 
We lost a lot of our insertion speed though. 

### Small iter optimizations
─── Insertion (keys/sec) ───
                               1000000
NibbleTrie                       4.41M

─── Lookup (keys/sec) ───
                               1000000
NibbleTrie                      12.46M

─── Iter forward (keys/sec) ───
                               1000000
NibbleTrie                     927.89M

─── Iter backward (keys/sec) ───
                               1000000
NibbleTrie                     938.03M

─── Iter fwd index (keys/sec) ───
                               1000000
NibbleTrie                     662.65M

─── Iter rev index (keys/sec) ───
                               1000000
NibbleTrie                       1.01G

─── Memory (bytes/key) ───
                               1000000
NibbleTrie                       186.3


### Leaf Nodes
The nibble trie node arena takes up ~60% of its memory. 
Node occupancy likely falls as we get deeper in the tree. 
The index is already ordered by key as well. 
A leaf node which is dedicated to storing leaves may cut down the memory footprint. 
We'd have to give up on direct addressing - each slot gets to store just its discriminant byte, and the offset to it. 

aaab
baaa
baab
aaaa

would create a prefix trie like 
0: [prefix_len : 0, children : [a->1,b->2 ...]]
1: [prefix_len : 3, children : [a->leaf4, b->leaf1 ...]]
2: [prefix_len : 3, children : [a->leaf2, b->leaf3 ...]]

but a leaf node that stores only the discriminating byte could store all 4 in one node.
They have to be stored lexigraphically ordered, each needs a prefixlen, a nibble, a leafptr, and a terminal bit. 
The node should store a u8 len as well. 

{ children : [ {prefixlen, ptr} ; 8 ], terminal : [bool;16], nibbles : u64, len : u8 }
The layout in this case would be 

in this example nibbles are just whole bytes instead for simplicity's sake.
{ [(0, null), (3,leaf4), (3, leaf1), (0, null), (3, leaf2), (3,leaf3)] , terminal : 0, nibbles : [a,a,b,b,a,b], len : 4 }

so , how do we logically scan a flattened tree like this? 
prefix_len is our only real hint, key[prefix] == nibbles[i] .
any increase in prefix len means a increase in the depth of the node. 
also, how do we properly represent terminal nodes here? we have to OR the comparison with the bool in terminal. 

0 : store 'parent check' , & it with previous

basically, prefix_len MUST increase once we've found a match. 
if a node doesnt match, we don't care about any subsequent nodes with prefix_len  > it. 

depth = children[0].prefix_len
i = 0 
L = query.len
let child = None
if depth >= L { return None }
for i in 0..len {
        let d  = children[i].prefix_len;
        if d < depth { break; }
        if d > depth { continue; }
        if (query[depth] == nibbles[i]) {
                //if terminal[i] && depth == L-1 { child= children[i]; } //terminal flag unnecessary? rather, not terminal is stored in the nonzeroness of the ptr
                child = children[i]
                if i+1==len {break} 
                let next_depth = children[i+1].prefix_len
                if next_depth <= depth || next_depth >= L {break}
                depth = next_depth
        }
}
if child.is_some() && index[child].key==query {return Some(index[child])} else {return None}
ok 
iteration doesnt touch this, index is sorted so there's no need. 

struct FlatNode {
        children : TinyArray< (Option< Nonzero< Ptr>>, LEN), 16> // len is children.len
        nibbles : u64,
}

Splitting : When its full, how do we split it up? 

find all the things at depth of child[0] : make a regular node, insert them as children, 
make leafnodes from their children and point the new node at them. 

worst case scenario : a chain. every child is a child of the previous one. 
either way we have to use child[0], leaf nodes cant point at other nodes. just have to hope we don't have chains > 16 elements very often. 


insertion : 

once we get to a leaf node, take the first pointer you can find down to index, 
scan it and compare from prefix_len..end between the stored keys and the new one to find what position the new one should be at in the leaf. 
We're also looking for the next none after that point, so we know how many pointers we have to shift. 
Worst case scenario the shift spans past the end of the leaf, so we need to traverse back up the tree and to the next neighbor and shift more things over.
check at what point it diverges from the immediately previous child (starting from its prefix_len), thats our new childs prefix length. 
take the nibble out, bitshift the bits from 4*i in nibbles right by 4 bits. ill let ai figure out the specifics of the bit operations. 
remember, theres a tricky edge case, when we have a new leftmost 

With leafnodes in the logic but not actually being made in insert : 


─── Insertion (keys/sec) ───
                               1000000
NibbleTrie                       4.28M

─── Lookup (keys/sec) ───
                               1000000
NibbleTrie                      11.94M

─── Iter forward (keys/sec) ───
                               1000000
NibbleTrie                     864.83M

─── Iter backward (keys/sec) ───
                               1000000
NibbleTrie                     953.67M

─── Iter fwd index (keys/sec) ───
                               1000000
NibbleTrie                     636.36M

─── Iter rev index (keys/sec) ───
                               1000000
NibbleTrie                     887.88M

─── Memory (bytes/key) ───
                               1000000
NibbleTrie                       186.3

With FNodes

─── Insertion (keys/sec) ───
                               1000000
NibbleTrie                       3.85M

─── Lookup (keys/sec) ───
                               1000000
NibbleTrie                      11.62M

─── Iter forward (keys/sec) ───
                               1000000
NibbleTrie                     489.75M

─── Iter backward (keys/sec) ───
                               1000000
NibbleTrie                     923.44M

─── Iter fwd index (keys/sec) ───
                               1000000
NibbleTrie                     424.55M

─── Iter rev index (keys/sec) ───
                               1000000
NibbleTrie                     997.54M

─── Memory (bytes/key) ───
                               1000000
NibbleTrie                       257.6


ok shoving fnodes and inodes into an enum is wasting a ton of space.

Also working on btree, 

=== TinyTrie Benchmark Suite ===
Tests:    insert
Sizes:    1000000
Structs:  IntBTree
Keys:     SeqU64
4s per bench · sequential per size

[n = 1000000]
  generating keys (SeqU64)... ✓ (0 byte keys, 1000000 u64 keys)
  insertion:
    IntBTree: 1 iters in 39.31s (39.31s/iter) ✓


─── Insertion (keys/sec) ───
                               1000000
IntBTree                         25.4K

Insertion on reverse sorted keys is terrible!
Turns out the particular way the agent handled this, if we don't have space after a leaf to split into it reallocs and spreads,
but since the capacity doubles without the number of elements doubling (in forward sorted keys) we get a ton of empty space at the end, 
then waste the space in the middle (everything else remains at 50% fill). 

But in the reverse case its just a sorted vec inserting at the beginning over and over. 