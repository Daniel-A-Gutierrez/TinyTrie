
─── Insertion (keys/sec) ───
                                    10             100            1000           10000          100000         1000000        10000000
SortedVec                       31.89M          19.85M          12.96M           8.32M           3.86M           2.97M                
BTreeMap                        23.65M          13.89M          14.02M           8.76M           4.12M          563.2K                
NibbleTrie                      23.06M          28.58M          18.17M           8.42M           3.30M           5.50M                
HashMap                         21.43M          12.11M          12.41M          12.12M           4.93M           2.17M                
DynTrie                         21.16M          18.36M          18.60M          11.31M           4.74M           6.57M                
BitTrie                         17.46M          19.41M          16.41M           9.29M           4.71M           5.59M                
FixedLen                        10.28M           3.52M           2.76M           1.99M           1.61M           1.18M             0.0
PolyTrie                         9.36M           3.91M           2.17M           1.59M          962.8K           1.85M                

─── Iter backward (keys/sec) ───
                                    10             100            1000           10000          100000         1000000        10000000
BTreeMap                       420.85M         990.69M         995.74M         799.14M         222.73M          50.75M                
DynTrieOpt                      98.67M         158.81M         131.46M         141.15M          72.06M          64.66M                
DynTrie                         98.26M         132.20M         131.81M         133.16M          67.99M          65.66M                
NibbleOpt                       88.13M         141.27M         114.56M         121.25M          67.35M          58.42M                
NibbleTrie                      86.30M         137.27M         138.80M         118.06M          61.84M          53.03M                
BitTrie                         85.98M         140.66M         145.44M         145.70M          71.92M          71.16M                
PolyTrie                        70.59M         106.31M         107.96M          98.25M          53.38M          49.96M                
FixedLenOpt                     65.01M          86.25M         105.61M          51.95M          37.76M          36.87M             0.0
FixedLen                        64.87M          86.42M         105.49M          52.04M          38.38M          36.94M             0.0

─── Iter forward (keys/sec) ───
                                    10             100            1000           10000          100000         1000000        10000000
SortedVec                      511.35M           2.60G           4.48G           4.98G           2.54G           5.03G                
BTreeMap                       413.32M         947.74M         924.66M         875.02M         261.80M          53.18M                
BitTrie                         85.29M         145.21M         154.31M         154.06M          69.82M          69.84M                
DynTrie                         84.67M         112.54M         119.39M         109.03M          61.95M          63.72M                
DynTrieOpt                      82.24M         125.28M         117.88M         109.33M          61.96M          62.83M                
NibbleOpt                       79.13M         117.96M         116.01M         103.27M          64.10M          64.78M                
NibbleTrie                      76.97M         119.51M         126.40M         112.24M          61.41M          62.21M                
PolyTrie                        70.41M         101.19M         104.23M          95.90M          54.31M          47.32M                
FixedLen                        48.61M          72.52M          94.02M          74.86M          34.54M          36.14M             0.0
FixedLenOpt                     48.58M          71.94M          93.92M          75.03M          34.43M          36.01M             0.0

─── Iter fwd index (keys/sec) ───
                                    10             100            1000           10000          100000         1000000        10000000
NibbleOpt                       96.49M         165.17M         141.82M         147.55M          72.98M          75.10M                
NibbleTrie                      95.24M         163.16M         175.76M         145.39M          69.95M          72.16M                
FixedLenOpt                     66.58M         118.48M         115.43M          86.30M          54.80M          57.09M             0.0
FixedLen                        66.20M         117.90M         115.06M          85.34M          54.95M          57.04M             0.0

─── Iter rev index (keys/sec) ───
                                    10             100            1000           10000          100000         1000000        10000000
NibbleOpt                      107.09M         197.94M         152.12M         160.94M          77.35M          70.23M                
NibbleTrie                     107.05M         197.93M         192.57M         155.21M          73.25M          63.21M                
FixedLenOpt                     87.32M         172.01M         174.31M         136.25M          69.49M          65.00M             0.0
FixedLen                        86.38M         171.68M         173.05M         134.69M          70.44M          65.25M             0.0

─── Lookup (keys/sec) ───
                                    10             100            1000           10000          100000         1000000        10000000
NibbleOptUnchecked             141.04M         108.91M          33.81M          21.70M          20.39M          20.95M                
DynTrieOpt                     138.64M         101.35M          28.22M          13.18M          18.60M          19.70M                
NibbleUnchecked                135.22M          60.29M          43.66M          26.82M          20.70M          20.97M                
NibbleOpt                      135.22M          96.85M          29.08M          13.76M          17.88M          19.02M                
NibbleTrie                     131.33M          86.57M          29.55M          12.23M           7.28M          11.73M                
DynTrie                        123.84M          50.17M          28.72M          12.94M          18.32M          19.70M                
BitTrie                        116.73M          48.21M          26.15M          16.68M          11.16M           9.78M                
FixedLen                        95.93M          63.90M          32.57M          18.96M          12.29M          17.89M             0.0
FixedLenOpt                     95.70M          59.17M          32.48M          18.35M          12.32M          17.73M             0.0
PolyTrie                        92.78M          45.49M          21.64M           9.39M           4.28M           4.94M                
BTreeMap                        82.23M          35.32M          35.11M          16.64M          11.99M           9.95M                
HashMap                         78.90M          77.29M          76.65M          50.90M          15.82M          11.07M                
SortedVec                       57.87M          31.82M          18.73M          12.09M           7.65M           6.43M                

─── Memory (bytes/key) ───
                                    10             100            1000           10000          100000         1000000        10000000
SortedVec                         41.3            44.1            42.4            43.6            41.4            40.6                
BTreeMap                          46.1            74.1            74.1            75.2            73.0            72.2                
HashMap                           63.7            78.0            78.0            65.7            52.6            77.8                
DynTrieOpt                        67.7            44.2            56.3            79.8           108.6            88.0                
BitTrie                           76.8            71.7            57.3            78.6            62.9            58.7                
DynTrie                           76.8            72.4            60.5            83.8           109.7            96.2                
PolyTrie                          97.4            76.2            95.1           102.0            83.8            93.5                
NibbleOpt                        112.5            62.1            81.8           120.6           156.2           126.1                
NibbleTrie                       115.2            64.0            86.0           124.5           157.3           134.2                
FixedLenOpt                      127.0            80.7           128.0           138.1           115.7           164.4             0.0
FixedLen                         139.6            86.6           128.7           154.8           123.2           165.7             0.0

─── Optimize (keys/sec) ───
                                    10             100            1000           10000          100000         1000000        10000000
NibbleOpt                       20.35M          23.65M          17.49M           7.86M           4.28M           5.98M                
DynTrieOpt                      19.45M          21.28M          15.30M           7.93M           4.30M           5.53M                
FixedLenOpt                      9.55M           3.36M           2.69M           2.74M          998.7K          781.5K             0.0
PolyTrie                         8.45M           3.81M           1.05M           1.17M          902.9K          961.2K                

