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
