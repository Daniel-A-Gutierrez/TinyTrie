
─── Insertion (keys/sec) ───
                                100            10000          1000000
NibbleTrie                      41.94M         16.31M         6.69M
BTreeMap                        18.19M         7.58M          3.02M
CTree                           10.79M         6.23M          2.73M

─── Iter backward (keys/sec) ───
                                100            10000          1000000
BTreeMap                        960.49M        996.82M        121.91M
CTree                           550.71M        715.14M        544.60M
NibbleTrie                      157.59M        157.34M        95.60M

─── Iter forward (keys/sec) ───
                                100            10000          1000000
CTree                           719.24M        840.40M        560.67M
BTreeMap                        943.95M        955.07M        111.84M
NibbleTrie                      167.43M        164.73M        114.17M

─── Iter fwd index (keys/sec) ───
                                100            10000          1000000
NibbleTrie                      219.03M        220.77M        130.37M

─── Iter rev index (keys/sec) ───
                                100            10000          1000000
NibbleTrie                      200.80M        203.57M        117.21M

─── Lookup (keys/sec) ───
                                100            10000          1000000
NibbleTrie                      155.35M        75.38M         32.35M
BTreeMap                        46.38M         17.26M         10.16M
CTree                           26.23M         10.89M         6.95M

─── Memory (bytes/key) ───
                                100            10000          1000000
CTree                           95.1           98.9           99.0
BTreeMap                        75.8           73.6           73.6
NibbleTrie                      66.6           82.7           78.1

─── Optimize (keys/sec) ───
                                100            10000          1000000

