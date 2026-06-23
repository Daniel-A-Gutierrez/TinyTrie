
─── Insertion (keys/sec) ───
                                   100           10000
SortedVec                       21.13M          12.72M
DynTrie                         19.73M          10.84M
BTreeMap                        19.63M          12.79M
BitTrie                         14.15M          18.10M
HashMap                         14.09M          15.74M
CTree                            8.96M           3.43M
NibbleTrie                       8.62M           9.46M
PolyTrie                         5.79M           2.29M

─── Iter backward (keys/sec) ───
                                   100           10000
BTreeMap                       986.16M           1.18G
CTree                          473.97M         691.65M
BitTrie                        157.11M         250.87M
DynTrieOpt                     146.22M         139.91M
DynTrie                        146.03M         140.27M
NibbleTrie                     103.52M         156.10M
PolyTrie                        98.09M         100.74M
NibbleOpt                          0.0         158.74M

─── Iter forward (keys/sec) ───
                                   100           10000
SortedVec                        2.66G           7.23G
BTreeMap                       870.94M           1.29G
CTree                          741.61M         833.09M
BitTrie                        161.17M         226.11M
DynTrie                        116.08M         108.35M
DynTrieOpt                     115.49M         109.25M
PolyTrie                        95.41M          98.31M
NibbleTrie                      92.95M         153.13M
NibbleOpt                          0.0         153.57M

─── Iter fwd index (keys/sec) ───
                                   100           10000
NibbleTrie                     100.66M         188.89M
NibbleOpt                          0.0         190.14M

─── Iter rev index (keys/sec) ───
                                   100           10000
NibbleTrie                     125.32M         201.54M
NibbleOpt                          0.0         203.41M

─── Lookup (keys/sec) ───
                                   100           10000
HashMap                         47.09M          31.50M
DynTrie                         45.31M          15.59M
DynTrieOpt                      45.03M          15.63M
BTreeMap                        42.88M          16.43M
PolyTrie                        33.04M           6.79M
SortedVec                       27.83M          11.80M
NibbleUnchecked                 26.60M             0.0
NibbleTrie                      24.39M          13.00M
BitTrie                         24.20M          12.11M
CTree                           10.24M           3.26M
NibbleOpt                          0.0          12.46M
NibbleOptUnchecked                 0.0          15.55M

─── Memory (bytes/key) ───
                                   100           10000
FixedLenOpt                       79.1           138.8
FixedLen                          80.2           159.1
SortedVec                        116.7           249.7
HashMap                          127.1           271.8
DynTrieOpt                       129.3           281.0
LinkedList                       132.7           265.7
BTreeMap                         150.1           281.3
PolyOpt                          154.0           298.6
PolyTrie                         157.7           310.4
NibbleOpt                        160.0           307.2
StackedTrie2                     165.2           313.8
StackedTrie4                     175.4           326.9
BitTrie                          189.4           419.4
DynTrie                          190.7           435.5
CTree                            207.4           411.4
NibbleTrie                       221.4           462.0

─── Optimize (keys/sec) ───
                                   100           10000
DynTrieOpt                      13.30M           5.87M
PolyTrie                         5.02M           2.46M
NibbleOpt                          0.0           8.32M

