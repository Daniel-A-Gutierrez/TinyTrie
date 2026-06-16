
─── Insertion (keys/sec) ───
                                    10             100            1000           10000          100000
SortedVec                       34.36M          21.13M          13.47M          12.72M                
BTreeMap                        26.28M          19.63M          13.62M          12.79M                
DynTrie                         20.48M          19.73M          17.89M          10.84M           7.04M
BitTrie                         18.82M          14.15M          14.19M          18.10M                
HashMap                         17.95M          14.09M           9.45M          15.74M                
NibbleTrie                      11.34M           8.62M           9.65M           9.46M                
PolyTrie                         9.80M           5.79M           4.61M           2.29M           1.95M

─── Iter backward (keys/sec) ───
                                    10             100            1000           10000          100000
BTreeMap                       425.16M         986.16M           1.01G           1.18G                
BitTrie                         94.46M         157.11M         171.35M         250.87M                
DynTrie                         85.51M         146.03M         141.39M         140.27M          74.37M
DynTrieOpt                      82.77M         146.22M         142.31M         139.91M          74.45M
NibbleTrie                      68.53M         103.52M         109.12M         156.10M                
PolyTrie                        65.00M          98.09M          99.91M         100.74M          56.18M
NibbleOpt                          0.0             0.0         109.92M         158.74M                

─── Iter forward (keys/sec) ───
                                    10             100            1000           10000          100000
SortedVec                      513.62M           2.66G           4.50G           7.23G                
BTreeMap                       416.24M         870.94M         920.11M           1.29G                
BitTrie                         91.66M         161.17M         171.90M         226.11M                
DynTrie                         68.04M         116.08M         107.56M         108.35M          63.55M
NibbleTrie                      67.61M          92.95M         107.81M         153.13M                
DynTrieOpt                      67.22M         115.49M         108.45M         109.25M          64.61M
PolyTrie                        66.31M          95.41M          98.83M          98.31M          57.90M
NibbleOpt                          0.0             0.0         107.95M         153.57M                

─── Iter fwd index (keys/sec) ───
                                    10             100            1000           10000          100000
NibbleTrie                      71.73M         100.66M         132.47M         188.89M                
NibbleOpt                          0.0             0.0         130.94M         190.14M                

─── Iter rev index (keys/sec) ───
                                    10             100            1000           10000          100000
NibbleTrie                      83.04M         125.32M         139.56M         201.54M                
NibbleOpt                          0.0             0.0         139.90M         203.41M                

─── Lookup (keys/sec) ───
                                    10             100            1000           10000          100000
BitTrie                        104.62M          24.20M          16.11M          12.11M           6.72M
PolyTrie                        81.77M          33.04M          27.25M           6.79M           4.25M
DynTrieOpt                      75.44M          45.03M          29.10M          15.63M           8.55M
DynTrie                         74.87M          45.31M          29.13M          15.59M           8.13M
BTreeMap                        68.84M          42.88M          32.48M          16.43M           7.93M
NibbleUnchecked                 62.26M          26.60M          23.51M                                
SortedVec                       56.81M          27.83M          18.94M          11.80M           5.93M
NibbleTrie                      56.78M          24.39M          21.52M          13.00M                
HashMap                         48.21M          47.09M          45.06M          31.50M           6.89M
NibbleOpt                          0.0             0.0          21.10M          12.46M                
NibbleOptUnchecked                 0.0             0.0          22.91M          15.55M                

─── Memory (bytes/key) ───
                                    10             100            1000           10000          100000
SortedVec                         97.5           116.7           109.8           249.7           222.5
BTreeMap                         102.3           150.1           141.6           281.3           254.1
LinkedList                       113.5           132.7           125.8           265.7           238.5
DynTrieOpt                       116.8           129.3           126.4           281.0           269.6
HashMap                          119.9           127.1           145.4           271.8           233.8
StackedTrie4                     132.8           175.4           155.1           326.9           282.7
FixedLenOpt                      138.3            79.1           135.7           138.8           117.5
PolyOpt                          144.1           154.0           139.0           298.6           260.4
BitTrie                          150.4           189.4           151.6           419.4           335.5
DynTrie                          151.2           190.7           185.0           435.5           427.3
NibbleOpt                        155.2           160.0           183.8           307.2           319.4
FixedLen                         156.5            80.2           141.2           159.1           129.4
PolyTrie                         159.3           157.7           160.8           310.4           264.9
StackedTrie2                     161.6           165.2           146.9           313.8           272.2
NibbleTrie                       189.6           221.4           218.1           462.0           422.1

─── Optimize (keys/sec) ───
                                    10             100            1000           10000          100000
DynTrieOpt                      12.76M          13.30M          12.37M           5.87M           3.93M
PolyTrie                         8.64M           5.02M           5.22M           2.46M           1.71M
NibbleOpt                          0.0             0.0           8.78M           8.32M                

