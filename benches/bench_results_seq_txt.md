
─── Insertion (keys/sec) ───
                                   100           10000         1000000
LinkedList                      53.23M             0.0             0.0
NibbleTrie                      30.59M             0.0             0.0
NibbleOpt                       29.67M             0.0             0.0
DynTrie                         27.68M             0.0             0.0
BitTrie                         18.91M             0.0             0.0
SortedVec                       17.74M             0.0             0.0
BTreeMap                        17.51M             0.0             0.0
HashMap                         17.29M             0.0             0.0
IntBTree                           13.89M           6.27M           1.28M
IntBTreeOpt                        12.23M           6.02M             0.0
StrBTree                  10.23M           6.06M             0.0
FixedLen                         9.90M             0.0             0.0
PolyTrie                         6.95M             0.0             0.0

─── Iter backward (keys/sec) ───
                                   100           10000         1000000
IntBTree                          546.77M         766.43M         527.65M

─── Iter forward (keys/sec) ───
                                   100           10000         1000000
SortedVec                        2.30G             0.0             0.0
IntBTree                            1.16G           1.45G         549.91M
LinkedList                     951.84M             0.0             0.0
IntBTreeOpt                       928.77M           1.05G             0.0
BTreeMap                       801.04M             0.0             0.0
NibbleOpt                      167.63M             0.0             0.0
StackedTrie4                   166.34M             0.0             0.0
NibbleTrie                     165.58M             0.0             0.0
StackedTrie2                   162.60M             0.0             0.0
BitTrie                        134.18M             0.0             0.0
DynTrieOpt                     123.80M             0.0             0.0
DynTrie                        122.61M             0.0             0.0
FixedLenOpt                    121.24M             0.0             0.0
FixedLen                       120.69M             0.0             0.0
PolyTrie                       117.04M             0.0             0.0
PolyOpt                        116.15M             0.0             0.0
StrBTree                  62.05M          64.49M             0.0

─── Iter fwd index (keys/sec) ───
                                   100           10000         1000000

─── Iter rev index (keys/sec) ───
                                   100           10000         1000000

─── Lookup (keys/sec) ───
                                   100           10000         1000000
FixedLenOpt                    240.34M             0.0             0.0
FixedLen                       239.03M             0.0             0.0
NibbleUnchecked                198.35M             0.0             0.0
NibbleOptUnchecked             183.29M             0.0             0.0
NibbleTrie                     129.06M             0.0             0.0
NibbleOpt                      127.12M             0.0             0.0
DynTrie                        124.18M             0.0             0.0
DynTrieOpt                     121.01M             0.0             0.0
StackedTrie4                   106.64M             0.0             0.0
StackedTrie2                   106.32M             0.0             0.0
HashMap                         82.70M             0.0             0.0
PolyTrie                        70.05M             0.0             0.0
PolyOpt                         69.17M             0.0             0.0
BitTrie                         56.89M             0.0             0.0
BTreeMap                        40.76M             0.0             0.0
IntBTreeOpt                        38.61M          16.08M             0.0
StrBTree                  36.09M          15.48M             0.0
IntBTree                           35.72M          15.06M           5.57M
SortedVec                       27.25M             0.0             0.0
LinkedList                      10.97M             0.0             0.0

─── Memory (bytes/key) ───
                                   100           10000         1000000
IntBTree                            131.8           169.3           115.7

─── Optimize (keys/sec) ───
                                   100           10000         1000000

