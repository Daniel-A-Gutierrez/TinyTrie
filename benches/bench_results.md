
─── Insertion (keys/sec) ───
                              10             100            1000           10000          100000        10000000
        TinyTrie          14.52M           9.77M           7.84M           5.10M           3.61M                
      LinkedList           8.43M           8.37M           8.21M           8.11M           7.80M           7.03M
       SortedVec           6.53M           4.18M           3.10M           2.59M           2.12M           1.23M
        BTreeMap           3.65M           1.98M           1.36M           1.05M          800.8K          585.1K
      NibbleTrie           2.64M           2.30M           2.02M           1.54M           1.26M           1.10M
   DynNibbleTrie           2.54M           2.26M           1.94M           1.48M           1.35M           1.05M
         BitTrie           2.19M           2.29M           2.08M           1.75M           1.48M           1.17M
         HashMap           1.88M           1.82M           1.44M           1.58M           1.40M          828.5K
        PolyTrie           1.16M           7.73M           8.58M           5.76M           4.62M                

─── Iter backward (keys/sec) ───
                              10             100            1000           10000          100000        10000000
        TinyTrie         174.43M         333.45M         354.19M         265.16M          69.99M                
      LinkedList         135.17M         190.32M         197.42M         185.83M         190.22M         186.03M
         PolyOpt          93.85M         133.60M         138.47M         140.04M         139.20M                
        PolyTrie          93.42M         133.17M         138.24M         137.22M         130.78M                
        BTreeMap          30.34M          32.15M          32.46M          31.23M          29.53M          26.41M
    DynNibbleOpt           9.36M          10.61M          10.62M          10.41M          10.22M          10.15M
   DynNibbleTrie           9.30M          10.69M          10.63M          10.39M          10.15M          10.20M
       NibbleOpt           8.75M          10.03M          10.30M           9.98M          10.05M          10.12M
      NibbleTrie           8.69M          10.00M          10.29M          10.06M          10.14M          10.14M
         BitTrie           8.50M           8.82M           9.00M           8.65M           8.72M           8.41M

─── Iter forward (keys/sec) ───
                              10             100            1000           10000          100000        10000000
        TinyTrie         174.42M         321.42M         331.96M         246.86M          69.08M                
      LinkedList         156.25M         224.54M         233.06M         222.23M         227.24M         214.61M
       SortedVec         136.88M         222.46M         238.67M         233.84M         232.11M         237.52M
         PolyOpt          88.34M         127.68M         131.08M         131.59M         131.04M                
        PolyTrie          87.46M         127.49M         131.09M         130.91M         127.71M                
        BTreeMap          31.28M          32.65M          32.88M          31.74M          28.58M          24.09M
   DynNibbleTrie           8.65M          10.49M          10.54M          10.46M          10.17M          10.12M
         BitTrie           8.63M           9.51M           9.67M           8.46M           9.41M           8.80M
    DynNibbleOpt           8.63M          10.48M          10.61M          10.32M          10.17M          10.14M
       NibbleOpt           8.08M           9.84M           9.93M          10.04M          10.05M          10.09M
      NibbleTrie           8.08M           9.89M          10.17M           9.99M           9.99M          10.03M

─── Iter fwd index (keys/sec) ───
                              10             100            1000           10000          100000        10000000
       NibbleOpt          10.83M          14.36M          15.15M          14.81M          15.00M          14.98M
      NibbleTrie          10.69M          14.31M          15.10M          14.68M          14.98M          14.97M

─── Iter rev index (keys/sec) ───
                              10             100            1000           10000          100000        10000000
      NibbleTrie          12.19M          14.78M          15.26M          15.01M          14.99M          14.96M
       NibbleOpt          12.16M          14.71M          15.22M          14.99M          14.91M          14.92M

─── Lookup (keys/sec) ───
                              10             100            1000           10000          100000        10000000
         PolyOpt         133.10M          76.23M          50.61M          29.51M          20.77M                
        PolyTrie         132.67M          76.15M          50.56M          22.08M          16.47M                
        TinyTrie          78.42M          44.42M          31.12M          15.69M          11.79M                
NibbleOptUnchecked          16.27M          12.00M           9.55M           8.01M           6.74M           4.54M
 NibbleUnchecked          16.26M          12.01M           9.56M           8.00M           6.74M           5.03M
   DynNibbleTrie          11.99M           9.96M           8.63M           7.42M           6.16M           4.92M
    DynNibbleOpt          11.96M           9.95M           8.62M           7.40M           6.15M           5.00M
         BitTrie          11.81M           7.78M           5.87M           4.60M           3.79M           2.75M
      NibbleTrie          11.64M           9.60M           8.17M           7.10M           6.22M           4.66M
       NibbleOpt          11.59M           9.57M           8.20M           7.05M           6.22M           4.87M
       SortedVec          10.29M           6.90M           5.17M           3.82M           3.14M           2.29M
        BTreeMap           6.53M           4.05M           2.79M           2.01M           1.65M           1.13M
         HashMap           4.27M           4.12M           4.18M           3.55M           3.09M           2.20M
      LinkedList           77.9K            8.1K                                                                

─── Memory (bytes/key) ───
                              10             100            1000           10000          100000        10000000
       SortedVec            37.0            38.0            39.0            40.0            41.0            43.0
        BTreeMap            41.8            71.4            70.8            71.6            72.6            74.6
      LinkedList            53.0            54.0            55.0            56.0            57.0            59.0
    DynNibbleOpt            56.3            41.9            37.5            56.8            52.1            66.2
   DynNibbleTrie            57.6            46.1            38.7            63.6            56.2            75.3
         HashMap            59.4            48.4            74.6            62.1            52.3            66.4
         BitTrie            70.4            61.4            49.2            78.6            64.2            85.6
         PolyOpt            74.0            66.4            59.4            70.6            84.4                
       NibbleOpt            78.7            50.8            42.8            65.3            54.9            69.7
      NibbleTrie            80.0            55.0            44.0            72.1            59.0            78.9
        TinyTrie            86.0            78.0            71.0            82.2            95.9                
        PolyTrie            98.8            79.0            65.4            83.4            85.8                

─── Optimize (keys/sec) ───
                              10             100            1000           10000          100000        10000000
         PolyOpt           8.42M           7.46M           8.11M           4.73M           3.70M                
       NibbleOpt           2.19M           2.02M           1.84M           1.39M           1.29M           1.02M
    DynNibbleOpt           2.02M           1.97M           1.77M           1.35M           1.20M           1.01M

