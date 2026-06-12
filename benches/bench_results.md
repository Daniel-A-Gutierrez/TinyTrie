
─── Insertion (keys/sec) ───
                              10             100            1000           10000          100000        10000000
        TinyTrie          14.52M           9.77M           7.84M           5.10M           3.61M                
        PolyTrie           8.94M           7.73M           8.58M           5.76M           4.62M                
      LinkedList           8.08M           8.11M           7.79M           7.56M           6.12M           6.65M
       SortedVec           6.27M           4.03M           3.03M           2.47M           2.05M           1.00M
        BTreeMap           3.45M           1.93M           1.28M          953.2K          724.3K          541.3K
      NibbleTrie           2.57M           2.15M           1.93M           1.48M           1.30M           1.08M
   DynNibbleTrie           2.56M           2.23M           1.90M           1.45M           1.31M           1.07M
         HashMap           1.79M           1.73M           1.36M           1.48M           1.06M          647.7K
         BitTrie           1.26M           1.03M          838.1K          888.7K          734.3K          547.2K

─── Iter backward (keys/sec) ───
                              10             100            1000           10000          100000        10000000
        TinyTrie         174.43M         333.45M         354.19M         265.16M          69.99M                
      LinkedList         119.70M         183.65M         190.98M         165.08M          95.97M          53.61M
         PolyOpt          93.85M         133.60M         138.47M         140.04M         139.20M                
        PolyTrie          93.42M         133.17M         138.24M         137.22M         130.78M                
        BTreeMap          28.63M          31.68M          31.63M          31.12M          27.73M          23.58M
   DynNibbleTrie           8.98M          10.19M          10.34M          10.44M           9.86M           9.81M
    DynNibbleOpt           8.98M          10.25M          10.36M          10.43M           9.86M           9.92M
       NibbleOpt           8.60M           9.88M          10.04M           9.99M           9.90M          10.02M
      NibbleTrie           8.59M           9.91M          10.06M           9.99M           9.80M          10.26M
         BitTrie           8.22M           8.68M           8.68M           8.60M           7.52M           7.41M

─── Iter forward (keys/sec) ───
                              10             100            1000           10000          100000        10000000
        TinyTrie         174.42M         321.42M         331.96M         246.86M          69.08M                
      LinkedList         148.37M         219.42M         226.77M         198.49M         110.45M         223.07M
       SortedVec         132.14M         216.27M         229.39M         232.83M         233.04M         240.50M
         PolyOpt          88.34M         127.68M         131.08M         131.59M         131.04M                
        PolyTrie          87.46M         127.49M         131.09M         130.91M         127.71M                
        BTreeMap          29.67M          31.86M          31.84M          30.75M          22.66M          24.54M
    DynNibbleOpt           8.35M          10.31M          10.33M          10.24M           9.80M           9.53M
   DynNibbleTrie           8.34M          10.35M          10.35M          10.29M           9.85M          10.21M
         BitTrie           8.29M           9.35M           9.36M           9.36M           7.88M           8.96M
       NibbleOpt           7.96M           9.72M           9.90M           9.88M           9.89M          10.26M
      NibbleTrie           7.90M           9.78M           9.80M           9.95M           9.77M          10.29M

─── Iter fwd index (keys/sec) ───
                              10             100            1000           10000          100000        10000000
      NibbleTrie          10.86M          14.24M          14.77M          13.68M          14.79M          15.06M
       NibbleOpt          10.85M          14.03M          14.77M          14.38M          14.76M          14.78M

─── Iter rev index (keys/sec) ───
                              10             100            1000           10000          100000        10000000
      NibbleTrie          12.10M          14.21M          15.00M          14.90M          14.77M          14.70M
       NibbleOpt          12.07M          14.42M          15.02M          14.92M          14.81M          14.91M

─── Lookup (keys/sec) ───
                              10             100            1000           10000          100000        10000000
         PolyOpt         133.10M          76.23M          50.61M          29.51M          20.77M                
        PolyTrie         132.67M          76.15M          50.56M          22.08M          16.47M                
        TinyTrie          78.42M          44.42M          31.12M          15.69M          11.79M                
 NibbleUnchecked          15.66M          11.79M           9.20M           7.78M           6.56M           5.10M
NibbleOptUnchecked          15.65M          11.77M           9.21M           7.90M           6.57M           5.08M
   DynNibbleTrie          11.55M           9.73M           8.23M           7.22M           5.84M           5.02M
    DynNibbleOpt          11.52M           9.68M           8.21M           7.19M           5.75M           4.99M
      NibbleTrie          11.20M           9.35M           7.77M           6.84M           5.67M           4.73M
       NibbleOpt          11.19M           9.37M           7.86M           6.84M           5.70M           4.72M
         BitTrie          11.17M           7.53M           5.56M           4.40M           3.52M           2.73M
       SortedVec           9.98M           6.65M           4.91M           3.64M           3.06M           2.28M
        BTreeMap           6.28M           3.97M           2.65M           1.93M           1.59M           1.13M
         HashMap           4.19M           3.94M           3.96M           3.47M           2.39M           1.72M
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
       NibbleOpt           2.13M           1.94M           1.77M           1.36M           1.26M           1.03M
    DynNibbleOpt           2.12M           1.95M           1.69M           1.35M           1.18M           1.04M

