
─── Insertion (keys/sec) ───
                                100            10000          1000000
CTree                           17.57M         6.18M          1.80M
LinkedList                      56.14M         0.0
SortedVec                       14.66M         1.35M
BTreeMap                        19.47M         6.30M
BitTrie                         21.78M         8.35M
NibbleTrie                      41.47M         0.0
DynTrie                         37.56M         0.0
HashMap                         17.86M         14.97M
FixedLen                        6.39M          0.0
NibbleOpt                       37.48M         0.0
PolyTrie                        10.53M         0.0

─── Iter backward (keys/sec) ───
                                100            10000          1000000
BTreeMap                        783.98M        624.99M
CTree                           790.66M        525.73M        29.85M
BitTrie                         143.61M        102.95M
NibbleTrie                      150.51M        0.0

─── Iter forward (keys/sec) ───
                                100            10000          1000000
SortedVec                       2.50G          4.78G
BTreeMap                        961.88M        689.17M
CTree                           817.88M        534.78M        31.53M
BitTrie                         146.00M        105.38M
NibbleTrie                      165.75M        0.0

─── Iter fwd index (keys/sec) ───
                                100            10000          1000000
NibbleTrie                      210.49M        0.0

─── Iter rev index (keys/sec) ───
                                100            10000          1000000
NibbleTrie                      195.27M        0.0

─── Lookup (keys/sec) ───
                                100            10000          1000000
BitTrie                         77.40M         25.07M
HashMap                         79.09M         56.96M
BTreeMap                        42.60M         9.70M
SortedVec                       31.95M         8.98M
CTree                           123.76M        30.24M         5.70M
NibbleTrie                      165.29M        0.0

─── Memory (bytes/key) ───
                                100            10000          1000000
NibbleTrie                      67.8           0.0
SortedVec                       40.0           40.0
BTreeMap                        56.8           58.3
HashMap                         50.4           62.1
CTree                           81.9           59.0           67.1
BitTrie                         61.4           78.6

─── Optimize (keys/sec) ───
                                100            10000          1000000

