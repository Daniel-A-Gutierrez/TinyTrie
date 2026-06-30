
─── Insertion (keys/sec) ───
                                   100           10000
LinkedList                       8.32M             0.0
SortedVec                        3.89M          814.9K
BTreeMap                         3.42M          14.80M
BitTrie                          2.22M          12.83M
HashMap                          1.77M          16.71M
DynTrie                          1.40M             0.0
NibbleTrie                       1.38M             0.0
CTree                              0.0             0.0

─── Iter backward (keys/sec) ───
                                   100           10000
BTreeMap                       957.80M         876.58M
BitTrie                        145.73M         153.22M
CTree                              0.0             0.0

─── Iter forward (keys/sec) ───
                                   100           10000
SortedVec                        2.54G           4.75G
BTreeMap                       841.57M         898.93M
BitTrie                        152.05M         157.17M
CTree                              0.0             0.0

─── Iter fwd index (keys/sec) ───
                                   100           10000

─── Iter rev index (keys/sec) ───
                                   100           10000

─── Lookup (keys/sec) ───
                                   100           10000
FixedLen                       225.48M             0.0
FixedLenOpt                    224.43M             0.0
SortedVec                      215.95M          11.98M
NibbleUnchecked                189.88M             0.0
NibbleOptUnchecked             189.29M             0.0
BTreeMap                       134.71M          17.81M
CTree                          129.90M             0.0
CTreeOpt                       129.72M             0.0
NibbleOpt                      112.80M             0.0
NibbleTrie                     112.44M             0.0
DynTrieOpt                     109.00M             0.0
DynTrie                        108.42M             0.0
StackedTrie4                    95.77M             0.0
StackedTrie2                    94.36M             0.0
HashMap                         88.32M          59.50M
BitTrie                         60.41M          27.93M
LinkedList                      12.93M             0.0

─── Memory (bytes/key) ───
                                   100           10000
SortedVec                         40.0            40.0
DynTrieOpt                        43.9             0.0
DynTrie                           47.4             0.0
HashMap                           50.4            62.1
NibbleOpt                         52.8             0.0
LinkedList                        56.0             0.0
NibbleTrie                        56.3             0.0
BitTrie                           61.4            78.6
BTreeMap                          73.4            71.6

─── Optimize (keys/sec) ───
                                   100           10000

