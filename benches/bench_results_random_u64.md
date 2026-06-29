
─── Insertion (keys/sec) ───
                                   100           10000         1000000
LinkedListU64                  122.14M             0.0             0.0
LinkedList                      55.63M             0.0             0.0
SortedVecU64                    54.30M             0.0             0.0
BTreeMap                        43.58M          17.15M           3.64M
BTreeMapU64                     43.18M             0.0             0.0
CTreeOpt                        41.04M          11.35M           3.70M
CTree                           40.61M          11.32M           3.75M
HashMapU64                      39.15M             0.0             0.0
NibbleTrie                      38.26M             0.0             0.0
NibbleOpt                       37.67M             0.0             0.0
HashMap                         37.52M          24.80M          13.08M
DynTrie                         35.59M             0.0             0.0
CTreeFixed                      27.44M           8.46M           2.73M
CTreeFixedOpt                   27.31M           8.12M           2.74M
BitTrie                         20.73M           8.35M             0.0
SortedVec                       14.37M           1.35M             0.0
PolyTrie                        10.53M             0.0             0.0
FixedLen                         7.78M             0.0             0.0

─── Iter backward (keys/sec) ───
                                   100           10000         1000000
LinkedList                     962.11M             0.0             0.0
BTreeMap                       725.81M         798.69M          45.85M
CTree                          534.35M         565.96M         397.41M
CTreeOpt                       529.71M         569.24M         461.80M
CTreeFixed                     493.59M         547.18M         364.30M
CTreeFixedOpt                  492.71M         549.03M         365.06M
DynTrieOpt                     147.54M             0.0             0.0
NibbleTrie                     147.07M             0.0             0.0
DynTrie                        146.17M             0.0             0.0
StackedTrie4                   143.36M             0.0             0.0
StackedTrie2                   142.91M             0.0             0.0
NibbleOpt                      140.82M             0.0             0.0
BitTrie                        133.30M         102.95M             0.0
FixedLen                       119.57M             0.0             0.0
FixedLenOpt                    119.52M             0.0             0.0

─── Iter forward (keys/sec) ───
                                   100           10000         1000000
SortedVec                        2.37G           4.78G             0.0
CTreeOpt                         1.22G           1.62G         833.19M
CTree                            1.21G           1.62G         729.42M
CTreeFixed                     957.45M           1.15G         599.24M
CTreeFixedOpt                  954.92M           1.19G         601.16M
LinkedList                     948.14M             0.0             0.0
BTreeMap                       935.02M           1.04G          46.28M
StackedTrie4                   153.01M             0.0             0.0
StackedTrie2                   152.66M             0.0             0.0
NibbleOpt                      149.85M             0.0             0.0
NibbleTrie                     148.71M             0.0             0.0
BitTrie                        134.16M         105.38M             0.0
DynTrie                        112.27M             0.0             0.0
DynTrieOpt                     111.65M             0.0             0.0
FixedLenOpt                     94.99M             0.0             0.0
FixedLen                        93.54M             0.0             0.0

─── Iter fwd index (keys/sec) ───
                                   100           10000         1000000
StackedTrie2                   197.63M             0.0             0.0
StackedTrie4                   193.83M             0.0             0.0
NibbleTrie                     193.82M             0.0             0.0
NibbleOpt                      189.66M             0.0             0.0
FixedLen                       110.00M             0.0             0.0
FixedLenOpt                    109.88M             0.0             0.0

─── Iter rev index (keys/sec) ───
                                   100           10000         1000000
NibbleOpt                      193.52M             0.0             0.0
NibbleTrie                     188.92M             0.0             0.0
StackedTrie4                   185.90M             0.0             0.0
StackedTrie2                   179.02M             0.0             0.0
FixedLen                       172.15M             0.0             0.0
FixedLenOpt                    166.95M             0.0             0.0

─── Lookup (keys/sec) ───
                                   100           10000         1000000
FixedLen                       286.77M             0.0             0.0
NibbleUnchecked                284.38M             0.0             0.0
NibbleOptUnchecked             284.05M             0.0             0.0
FixedLenOpt                    275.62M             0.0             0.0
SortedVecU64                   232.68M             0.0             0.0
BTreeMapU64                    170.82M             0.0             0.0
NibbleOpt                      146.41M             0.0             0.0
NibbleTrie                     146.19M             0.0             0.0
DynTrie                        145.79M             0.0             0.0
CTreeFixed                     145.63M          20.71M           5.30M
BTreeMap                       145.57M          22.80M           3.39M
DynTrieOpt                     145.17M             0.0             0.0
CTree                          144.36M          23.44M           4.55M
CTreeOpt                       143.97M          23.51M           4.79M
CTreeFixedOpt                  138.79M          20.38M           5.39M
StackedTrie4                   131.48M             0.0             0.0
StackedTrie2                   123.91M             0.0             0.0
HashMap                         98.39M          96.69M          12.35M
HashMapU64                      95.64M             0.0             0.0
BitTrie                         70.83M          25.07M             0.0
SortedVec                       27.45M           8.98M             0.0
LinkedListU64                   13.82M             0.0             0.0
LinkedList                      10.23M             0.0             0.0

─── Memory (bytes/key) ───
                                   100           10000         1000000
HashMap                           21.9            27.9            35.7
CTree                             22.2            22.9            22.8
CTreeOpt                          22.2            22.9            22.8
BTreeMap                          24.0            27.0            27.1
SortedVec                         40.0            40.0             0.0
CTreeFixed                        40.3            37.4            37.2
CTreeFixedOpt                     40.3            37.4            37.2
DynTrieOpt                        42.4             0.0             0.0
DynTrie                           52.5             0.0             0.0
LinkedList                        56.0             0.0             0.0
NibbleOpt                         57.8             0.0             0.0
StackedTrie2                      60.3             0.0             0.0
BitTrie                           61.4            78.6             0.0
StackedTrie4                      65.5             0.0             0.0
NibbleTrie                        67.8             0.0             0.0
FixedLenOpt                       82.5             0.0             0.0
FixedLen                          87.5             0.0             0.0

─── Optimize (keys/sec) ───
                                   100           10000         1000000
CTreeOpt                        34.38M          12.58M           3.45M
CTreeFixedOpt                   23.41M           9.16M           2.56M
NibbleOpt                       22.97M             0.0             0.0
DynTrieOpt                      21.30M             0.0             0.0
StackedTrie2                    20.22M             0.0             0.0
StackedTrie4                    19.83M             0.0             0.0
FixedLenOpt                      7.60M             0.0             0.0

