
─── Insertion (keys/sec) ───
                                   100           10000         1000000
LinkedList                      56.14M             0.0             0.0
NibbleTrie                      41.47M             0.0             0.0
DynTrie                         37.56M             0.0             0.0
NibbleOpt                       37.48M             0.0             0.0
BitTrie                         21.78M           8.35M             0.0
BTreeMap                        19.36M           6.65M          921.1K
HashMap                         17.86M          14.97M             0.0
SortedVec                       14.66M           1.35M             0.0
CTree                           13.96M           4.62M          580.5K
CTreeOpt                        13.96M           4.51M          606.3K
PolyTrie                        10.53M             0.0             0.0
FixedLen                         6.39M             0.0             0.0

─── Iter backward (keys/sec) ───
                                   100           10000         1000000
BTreeMap                       899.72M         777.11M          31.06M
CTree                          404.36M         391.50M          24.48M
CTreeOpt                       403.20M         490.06M         275.66M
NibbleTrie                     150.51M             0.0             0.0
BitTrie                        143.61M         102.95M             0.0

─── Iter forward (keys/sec) ───
                                   100           10000         1000000
SortedVec                        2.50G           4.78G             0.0
BTreeMap                       895.69M         912.50M          47.46M
CTree                          894.29M         571.55M          26.62M
CTreeOpt                       893.67M           1.05G         454.60M
NibbleTrie                     165.75M             0.0             0.0
BitTrie                        146.00M         105.38M             0.0

─── Iter fwd index (keys/sec) ───
                                   100           10000         1000000
NibbleTrie                     210.49M             0.0             0.0

─── Iter rev index (keys/sec) ───
                                   100           10000         1000000
NibbleTrie                     195.27M             0.0             0.0

─── Lookup (keys/sec) ───
                                   100           10000         1000000
NibbleTrie                     165.29M             0.0             0.0
HashMap                         79.09M          56.96M             0.0
BitTrie                         77.40M          25.07M             0.0
BTreeMap                        43.53M          10.12M           1.83M
SortedVec                       31.95M           8.98M             0.0
CTree                           28.55M           9.07M           1.26M
CTreeOpt                        28.50M           9.19M           1.09M

─── Memory (bytes/key) ───
                                   100           10000         1000000
SortedVec                         40.0            40.0             0.0
HashMap                           50.4            62.1             0.0
BTreeMap                          53.1            58.5            58.6
CTree                             58.5            59.0            58.8
CTreeOpt                          59.4            59.0            58.8
BitTrie                           61.4            78.6             0.0
NibbleTrie                        67.8             0.0             0.0

─── Optimize (keys/sec) ───
                                   100           10000         1000000
CTreeOpt                        13.78M           3.91M          563.4K

