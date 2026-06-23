
─── Insertion (keys/sec) ───
                                   100           10000         1000000
LinkedList                      56.14M             0.0             0.0
NibbleTrie                      41.47M             0.0             0.0
DynTrie                         37.56M             0.0             0.0
NibbleOpt                       37.48M             0.0             0.0
BitTrie                         21.78M           8.35M             0.0
BTreeMap                        20.13M           6.83M          958.2K
HashMap                         17.86M          14.97M             0.0
CTree                           14.84M           4.98M          911.4K
SortedVec                       14.66M           1.35M             0.0
PolyTrie                        10.53M             0.0             0.0
FixedLen                         6.39M             0.0             0.0

─── Iter backward (keys/sec) ───
                                   100           10000         1000000
BTreeMap                       963.75M         799.45M          30.93M
CTree                          703.43M         556.83M          31.73M
NibbleTrie                     150.51M             0.0             0.0
BitTrie                        143.61M         102.95M             0.0

─── Iter forward (keys/sec) ───
                                   100           10000         1000000
SortedVec                        2.50G           4.78G             0.0
CTree                          945.32M         576.34M          31.53M
BTreeMap                       936.94M         970.15M          40.36M
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
BTreeMap                        37.52M           9.96M           1.77M
SortedVec                       31.95M           8.98M             0.0
CTree                           31.01M           9.81M           1.81M

─── Memory (bytes/key) ───
                                   100           10000         1000000
SortedVec                         40.0            40.0             0.0
HashMap                           50.4            62.1             0.0
BTreeMap                          53.1            58.5            58.6
CTree                             58.5            59.0            58.8
BitTrie                           61.4            78.6             0.0
NibbleTrie                        67.8             0.0             0.0

─── Optimize (keys/sec) ───
                                   100           10000         1000000

