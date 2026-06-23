
─── Insertion (keys/sec) ───
                                100            10000          1000000
SortedVec                       19.85M         8.32M          2.97M
DynTrie                         29.19M         12.49M         7.16M
BTreeMap                        13.89M         8.76M          563.2K
NibbleTrie                      28.58M         8.42M          5.50M
HashMap                         12.11M         12.12M         2.17M
BitTrie                         19.41M         9.29M          5.59M
FixedLen                        3.52M          1.99M          1.18M
PolyTrie                        3.91M          1.59M          1.85M
CTree                           4.48M          2.31M          2.60M

─── Iter backward (keys/sec) ───
                                100            10000          1000000
BTreeMap                        990.69M        799.14M        50.75M
DynTrieOpt                      172.23M        151.61M        66.57M
DynTrie                         171.21M        151.39M        66.07M
StackedTrie2                    161.87M        131.49M        57.24M
NibbleOpt                       141.27M        121.25M        58.42M
StackedTrie4                    164.97M        132.37M        57.30M
NibbleTrie                      137.27M        118.06M        53.03M
BitTrie                         140.66M        145.70M        71.16M
PolyTrie                        106.31M        98.25M         49.96M
FixedLenOpt                     86.25M         51.95M         36.87M
FixedLen                        86.42M         52.04M         36.94M
CTree                           545.58M        700.40M        571.32M

─── Iter forward (keys/sec) ───
                                100            10000          1000000
SortedVec                       2.60G          4.98G          5.03G
BTreeMap                        947.74M        875.02M        53.18M
StackedTrie4                    159.49M        143.66M        72.61M
StackedTrie2                    157.33M        142.73M        72.69M
BitTrie                         145.21M        154.06M        69.84M
NibbleOpt                       117.96M        103.27M        64.78M
DynTrieOpt                      121.13M        110.38M        64.16M
DynTrie                         120.80M        110.16M        62.69M
NibbleTrie                      119.51M        112.24M        62.21M
PolyTrie                        101.19M        95.90M         47.32M
FixedLen                        72.52M         74.86M         36.14M
FixedLenOpt                     71.94M         75.03M         36.01M
CTree                           750.80M        815.68M        644.81M

─── Iter fwd index (keys/sec) ───
                                100            10000          1000000
StackedTrie2                    215.70M        184.52M        82.13M
StackedTrie4                    205.38M        179.43M        80.80M
NibbleOpt                       165.17M        147.55M        75.10M
NibbleTrie                      163.16M        145.39M        72.16M
FixedLenOpt                     118.48M        86.30M         57.09M
FixedLen                        117.90M        85.34M         57.04M

─── Iter rev index (keys/sec) ───
                                100            10000          1000000
StackedTrie4                    213.42M        157.11M        65.71M
StackedTrie2                    206.83M        156.92M        65.42M
NibbleOpt                       197.94M        160.94M        70.23M
NibbleTrie                      197.93M        155.21M        63.21M
FixedLenOpt                     172.01M        136.25M        65.00M
FixedLen                        171.68M        134.69M        65.25M

─── Lookup (keys/sec) ───
                                100            10000          1000000
NibbleOptUnchecked              108.91M        21.70M         20.95M
NibbleUnchecked                 60.29M         26.82M         20.97M
NibbleOpt                       96.85M         13.76M         19.02M
NibbleTrie                      86.57M         12.23M         11.73M
DynTrieOpt                      81.47M         15.75M         14.93M
DynTrie                         81.30M         15.65M         14.87M
BitTrie                         48.21M         16.68M         9.78M
StackedTrie2                    68.22M         12.78M         10.75M
StackedTrie4                    68.11M         11.79M         10.78M
FixedLen                        63.90M         18.96M         17.89M
FixedLenOpt                     59.17M         18.35M         17.73M
PolyTrie                        45.49M         9.39M          4.94M
BTreeMap                        35.32M         17.55M         9.95M
HashMap                         77.29M         50.90M         11.07M
SortedVec                       31.82M         12.09M         6.43M
CTree                           31.01M         6.61M          5.19M

─── Memory (bytes/key) ───
                                100            10000          1000000
CTree                           84.6           97.7           96.9
SortedVec                       44.1           43.6           40.6
BTreeMap                        74.1           75.2           72.2
DynTrieOpt                      37.6           72.5           76.7
HashMap                         78.0           65.7           77.8
DynTrie                         46.1           83.8           130.0
BitTrie                         71.7           78.6           58.7
PolyTrie                        76.2           102.0          93.5
StackedTrie2                    55.5           105.3          78.8
NibbleOpt                       62.1           120.6          126.1
StackedTrie4                    60.7           118.4          87.2
NibbleTrie                      64.0           124.5          134.2
FixedLenOpt                     80.7           138.1          164.4
FixedLen                        86.6           154.8          165.7

─── Optimize (keys/sec) ───
                                100            10000          1000000
NibbleOpt                       23.65M         7.86M          5.98M
DynTrieOpt                      20.01M         7.20M          4.43M
StackedTrie4                    17.61M         4.54M          3.62M
StackedTrie2                    18.90M         5.07M          3.15M
FixedLenOpt                     3.36M          2.74M          781.5K
PolyTrie                        3.81M          1.17M          961.2K

