
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
CTree                           13.53M           3.74M          534.3K
CTreeOpt                        13.09M           3.81M          532.8K
PolyTrie                        10.53M             0.0             0.0
FixedLen                         6.39M             0.0             0.0

─── Iter backward (keys/sec) ───
                                   100           10000         1000000
BTreeMap                       899.72M         777.11M          31.06M
CTree                          405.84M         476.69M         304.10M
CTreeOpt                       398.84M         478.38M         309.32M
NibbleTrie                     150.51M             0.0             0.0
BitTrie                        143.61M         102.95M             0.0

─── Iter forward (keys/sec) ───
                                   100           10000         1000000
SortedVec                        2.50G           4.78G             0.0
CTreeOpt                       895.91M           1.05G         486.26M
BTreeMap                       895.69M         912.50M          47.46M
CTree                          886.68M           1.01G         488.36M
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
CTree                           28.84M           8.85M           1.26M
CTreeOpt                        28.82M           8.62M           1.17M

─── Memory (bytes/key) ───
                                   100           10000         1000000
SortedVec                         40.0            40.0             0.0
HashMap                           50.4            62.1             0.0
BTreeMap                          53.1            58.5            58.6
CTree                             59.1            58.5            58.8
CTreeOpt                          59.1            58.5            58.8
BitTrie                           61.4            78.6             0.0
NibbleTrie                        67.8             0.0             0.0

─── Optimize (keys/sec) ───
                                   100           10000         1000000
CTreeOpt                        12.06M           3.60M          528.5K

