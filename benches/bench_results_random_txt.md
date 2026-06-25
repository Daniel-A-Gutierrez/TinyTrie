
─── Insertion (keys/sec) ───
                                   100           10000         1000000
NibbleOpt                       38.58M          28.51M           5.01M
NibbleTrie                      37.85M          19.47M           5.15M
DynTrie                         34.57M          23.94M           5.14M
SortedVec                       18.52M           9.19M           2.61M
BTreeMap                        18.19M           9.58M           2.69M
HashMap                         15.46M          12.24M           1.66M
FixedLen                        10.90M           6.18M           1.43M
CTree                            8.44M           5.84M           2.37M
CTreeOpt                         8.30M           5.81M           2.35M

─── Iter backward (keys/sec) ───
                                   100           10000         1000000
BTreeMap                       914.96M         960.28M          89.61M
CTreeOpt                       399.51M         430.43M         240.21M
CTree                          397.77M         460.93M         237.37M
NibbleTrie                     152.39M         157.66M          84.98M
DynTrie                        152.31M         157.05M          84.50M
DynTrieOpt                     152.18M         156.74M          86.25M
StackedTrie2                   149.40M         154.31M          83.61M
StackedTrie4                   148.96M         153.69M          80.09M
NibbleOpt                      146.02M         157.25M          82.42M
FixedLen                       132.14M         130.84M          76.20M
FixedLenOpt                    130.54M         130.84M          75.94M

─── Iter forward (keys/sec) ───
                                   100           10000         1000000
SortedVec                        2.46G           4.67G           4.70G
BTreeMap                       906.55M         953.25M          82.62M
CTree                          887.48M         992.34M         405.57M
CTreeOpt                       884.57M         987.09M         412.30M
NibbleOpt                      166.11M         163.55M          93.58M
NibbleTrie                     164.80M         163.28M          93.17M
StackedTrie4                   162.06M         167.36M          89.81M
StackedTrie2                   161.45M         166.43M          91.12M
DynTrie                        120.41M         121.76M          78.41M
DynTrieOpt                     120.39M         122.13M          79.99M
FixedLen                       101.99M          96.58M          75.53M
FixedLenOpt                    101.71M          96.50M          75.14M

─── Iter fwd index (keys/sec) ───
                                   100           10000         1000000
NibbleOpt                      223.86M         223.04M         119.71M
NibbleTrie                     223.86M         222.87M         119.45M
StackedTrie2                   216.86M         222.62M         118.54M
StackedTrie4                   211.72M         216.41M         113.96M
FixedLen                       121.63M         112.04M          85.88M
FixedLenOpt                    121.02M         112.05M          86.16M

─── Iter rev index (keys/sec) ───
                                   100           10000         1000000
NibbleOpt                      203.83M         205.05M         103.26M
NibbleTrie                     202.75M         206.89M         104.72M
StackedTrie2                   198.00M         197.73M         102.88M
StackedTrie4                   194.42M         195.88M          97.00M
FixedLen                       170.62M         168.59M          90.96M
FixedLenOpt                    168.90M         168.52M          93.09M

─── Lookup (keys/sec) ───
                                   100           10000         1000000
NibbleOptUnchecked             299.02M         133.90M          48.73M
NibbleUnchecked                298.09M         133.48M          46.97M
FixedLen                       205.66M         103.41M          51.66M
FixedLenOpt                    203.01M         103.45M          51.11M
NibbleTrie                     148.85M          72.19M          22.90M
NibbleOpt                      148.58M          72.67M          24.11M
DynTrieOpt                     147.66M          67.79M          22.09M
DynTrie                        147.37M          66.17M          21.22M
StackedTrie2                   135.81M          66.98M          22.96M
StackedTrie4                   132.11M          65.69M          22.76M
HashMap                         79.09M          48.97M           8.24M
BTreeMap                        44.71M          16.59M           7.58M
CTree                           30.55M          13.48M           8.16M
CTreeOpt                        30.54M          13.30M           8.38M
SortedVec                       28.88M          10.54M           5.40M

─── Memory (bytes/key) ───
                                   100           10000         1000000
SortedVec                         42.2            41.9            42.0
DynTrieOpt                        44.6            53.6            78.0
HashMap                           52.6            64.0            79.2
DynTrie                           55.0            78.7            68.8
NibbleOpt                         60.0            66.7            75.9
StackedTrie2                      62.6            70.0            80.1
StackedTrie4                      67.7            76.5            88.5
NibbleTrie                        70.4            82.3            80.2
BTreeMap                          75.7            73.5            73.6
CTree                             81.4            81.7            81.8
CTreeOpt                          81.4            81.7            81.8
FixedLenOpt                       92.7           108.0           128.3
FixedLen                         100.0           124.6           129.5

─── Optimize (keys/sec) ───
                                   100           10000         1000000
NibbleOpt                       24.45M          13.10M           3.82M
DynTrieOpt                      22.71M          11.81M           3.76M
StackedTrie2                    21.42M           8.01M           3.41M
StackedTrie4                    20.74M           7.86M           3.26M
FixedLenOpt                      9.62M           6.44M           1.35M
CTreeOpt                         7.44M           4.61M           2.03M

