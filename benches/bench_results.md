
─── Insertion (keys/sec) ───
                              10             100            1000          100000        10000000
      LinkedList          67.65M          59.19M          58.63M          55.29M          42.07M
       SortedVec          32.49M          19.27M          13.83M           8.18M           3.31M
      NibbleTrie          25.43M          27.33M          25.43M          26.86M          18.77M
        BTreeMap          24.50M          15.16M          14.22M           8.02M           5.13M
   DynNibbleTrie          23.76M          24.83M          23.92M          23.23M          16.97M
         HashMap          22.99M          19.38M          16.53M          12.06M           2.36M
        TinyTrie          14.52M           9.77M           7.84M           5.10M           3.61M
        PolyTrie           8.94M           7.73M           8.58M           5.76M           4.62M
         BitTrie           8.81M           6.73M           8.51M           4.63M           2.57M

─── Iter backward (keys/sec) ───
                              10             100            1000          100000        10000000
      LinkedList         482.97M           1.07G         712.40M         698.42M         438.37M
        BTreeMap         421.24M         989.98M           1.09G         362.73M          53.74M
        TinyTrie         174.43M         333.45M         354.19M         265.16M          69.99M
    DynNibbleOpt         115.16M         180.58M         189.68M         191.73M         188.46M
   DynNibbleTrie         114.39M         180.92M         189.17M         191.74M         188.48M
       NibbleOpt         107.99M         160.60M         172.36M         171.71M         168.63M
      NibbleTrie         107.00M         160.27M         172.10M         166.92M         167.75M
         BitTrie          97.98M         175.43M         190.95M         185.89M         182.31M
         PolyOpt          93.85M         133.60M         138.47M         140.04M         139.20M
        PolyTrie          93.42M         133.17M         138.24M         137.22M         130.78M

─── Iter forward (keys/sec) ───
                              10             100            1000          100000        10000000
       SortedVec         510.99M           2.65G           4.42G           4.94G           4.98G
      LinkedList         471.46M           1.06G         716.91M         681.01M         414.80M
        BTreeMap         410.18M         882.31M         954.90M         387.03M          53.38M
        TinyTrie         174.42M         321.42M         331.96M         246.86M          69.08M
      NibbleTrie          99.59M         155.29M         163.16M         163.00M         161.94M
       NibbleOpt          98.62M         155.52M         163.29M         163.56M         162.31M
    DynNibbleOpt          97.32M         154.27M         160.51M         154.92M         154.14M
   DynNibbleTrie          97.23M         153.95M         160.13M         154.70M         154.08M
         BitTrie          96.99M         179.69M         192.32M         184.43M         173.53M
         PolyOpt          88.34M         127.68M         131.08M         131.59M         131.04M
        PolyTrie          87.46M         127.49M         131.09M         130.91M         127.71M

─── Iter fwd index (keys/sec) ───
                              10             100            1000          100000        10000000
      NibbleTrie         126.03M         223.47M         240.16M         240.16M         237.98M
       NibbleOpt         125.49M         223.34M         240.67M         240.26M         237.80M

─── Iter rev index (keys/sec) ───
                              10             100            1000          100000        10000000
       NibbleOpt         146.69M         228.89M         253.59M         254.71M         248.35M
      NibbleTrie         146.25M         228.04M         253.71M         254.28M         247.98M

─── Lookup (keys/sec) ───
                              10             100            1000          100000        10000000
NibbleOptUnchecked       215.76M         199.65M         139.47M          76.24M          52.71M
 NibbleUnchecked         212.38M         201.02M         139.29M          76.93M          51.56M
    DynNibbleOpt         193.98M         153.41M         101.61M          68.16M          43.75M
   DynNibbleTrie         193.73M         153.26M         102.08M          68.14M          43.83M
       NibbleOpt         184.21M         149.33M         108.47M          68.49M          44.42M
      NibbleTrie         183.91M         149.67M         108.65M          68.45M          44.39M
         PolyOpt         133.10M          76.23M          50.61M          29.51M          20.77M
        PolyTrie         132.67M          76.15M          50.56M          22.08M          16.47M
         BitTrie          99.52M          53.17M          32.33M          15.46M          11.06M
         HashMap          82.40M          85.31M          82.12M          20.86M           8.79M
        TinyTrie          78.42M          44.42M          31.12M          15.69M          11.79M
        BTreeMap          75.04M          49.13M          34.08M          14.35M          10.18M
       SortedVec          60.77M          33.06M          19.87M           9.88M           6.57M
      LinkedList           77.9K            8.1K                                                

─── Memory (bytes/key) ───
                              10             100            1000          100000        10000000
       SortedVec            37.0            38.0            39.0            41.0            43.0
        BTreeMap            41.8            71.4            70.8            72.6            74.6
      LinkedList            53.0            54.0            55.0            57.0            59.0
    DynNibbleOpt            56.3            41.9            37.5            52.1            66.2
   DynNibbleTrie            57.6            46.1            38.7            56.2            75.3
         HashMap            59.4            48.4            74.6            52.3            66.4
         PolyOpt            74.0            66.4            59.4            70.6            84.4
         BitTrie            76.5            63.3            53.1            67.7            85.8
       NibbleOpt            78.7            50.8            42.8            54.9            69.7
      NibbleTrie            80.0            55.0            44.0            59.0            78.9
        TinyTrie            86.0            78.0            71.0            82.2            95.9
        PolyTrie            98.8            79.0            65.4            83.4            85.8

─── Optimize (keys/sec) ───
                              10             100            1000          100000        10000000
       NibbleOpt          23.33M          25.00M          23.22M          24.92M          11.48M
    DynNibbleOpt          22.10M          22.63M          22.14M          21.59M          14.06M
         PolyOpt           8.42M           7.46M           8.11M           4.73M           3.70M

