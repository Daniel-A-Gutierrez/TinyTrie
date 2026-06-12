
─── Insertion (keys/sec) ───
                              10             100            1000           10000          100000         1000000        10000000
       SortedVec          31.89M          19.85M          12.96M           8.32M           3.86M           2.97M                
        BTreeMap          23.65M          13.89M          14.02M           8.76M           4.12M          563.2K                
      NibbleTrie          22.26M          20.43M          20.99M          12.00M           4.57M           6.24M                
         HashMap          21.43M          12.11M          12.41M          12.12M           4.93M           2.17M                
   DynNibbleTrie          21.16M          18.36M          18.60M          11.31M           4.74M           6.57M                
         BitTrie          17.46M          19.41M          16.41M           9.29M           4.71M           5.59M                
        PolyTrie          10.10M           7.26M           3.68M           2.60M           1.95M           1.83M          16.35M

─── Iter backward (keys/sec) ───
                              10             100            1000           10000          100000         1000000        10000000
        BTreeMap         420.85M         990.69M         995.74M         799.14M         222.73M          50.75M                
    DynNibbleOpt          98.67M         158.81M         131.46M         141.15M          72.06M          64.66M                
   DynNibbleTrie          98.26M         132.20M         131.81M         133.16M          67.99M          65.66M                
      NibbleTrie          89.38M         114.36M         114.82M         112.32M          59.37M          59.81M                
       NibbleOpt          88.13M         141.27M         114.56M         121.25M          67.35M          58.42M                
         BitTrie          85.98M         140.66M         145.44M         145.70M          71.92M          71.16M                
        PolyTrie          72.65M         110.42M         110.86M         100.69M          55.77M          53.15M         440.39M

─── Iter forward (keys/sec) ───
                              10             100            1000           10000          100000         1000000        10000000
       SortedVec         511.35M           2.60G           4.48G           4.98G           2.54G           5.03G                
        BTreeMap         413.32M         947.74M         924.66M         875.02M         261.80M          53.18M                
         BitTrie          85.29M         145.21M         154.31M         154.06M          69.82M          69.84M                
   DynNibbleTrie          84.67M         112.54M         112.00M         110.73M          61.95M          63.72M                
    DynNibbleOpt          82.24M         125.28M         112.14M         117.06M          61.96M          62.83M                
       NibbleOpt          79.13M         117.96M         109.83M         115.04M          64.10M          64.78M                
      NibbleTrie          77.00M         111.18M         109.97M         108.05M          63.96M          65.49M                
        PolyTrie          73.09M         105.23M         107.15M          98.68M          57.62M          56.77M         480.02M

─── Iter fwd index (keys/sec) ───
                              10             100            1000           10000          100000         1000000        10000000
      NibbleTrie         101.16M         143.41M         141.80M         140.56M          73.12M          75.91M                
       NibbleOpt          96.49M         165.17M         141.82M         147.55M          72.98M          75.10M                

─── Iter rev index (keys/sec) ───
                              10             100            1000           10000          100000         1000000        10000000
      NibbleTrie         110.78M         150.65M         151.48M         151.85M          77.45M          71.32M                
       NibbleOpt         107.09M         197.94M         152.12M         160.94M          77.35M          70.23M                

─── Lookup (keys/sec) ───
                              10             100            1000           10000          100000         1000000        10000000
NibbleOptUnchecked         141.04M         108.91M          60.39M          32.21M          20.39M          20.95M                
    DynNibbleOpt         138.64M         101.35M          49.03M          23.60M          18.60M          19.70M                
 NibbleUnchecked         135.22M          60.29M          43.66M          26.82M          20.70M          20.97M                
       NibbleOpt         135.22M          96.85M          52.66M          24.63M          17.88M          19.02M                
   DynNibbleTrie         123.84M          50.17M          50.11M          25.86M          18.32M          19.70M                
      NibbleTrie         119.01M          53.06M          53.23M          26.25M          17.90M          19.05M                
         BitTrie         116.73M          48.21M          26.15M          16.68M          11.16M           9.78M                
        PolyTrie          95.52M          51.18M          24.33M          15.19M           8.14M           9.62M           7.71M
        BTreeMap          82.23M          35.32M          35.11M          16.64M          11.99M           9.95M                
         HashMap          78.90M          77.29M          76.65M          50.90M          15.82M          11.07M                
       SortedVec          57.87M          31.82M          18.73M          12.09M           7.65M           6.43M                

─── Memory (bytes/key) ───
                              10             100            1000           10000          100000         1000000        10000000
       SortedVec            41.3            44.1            42.4            43.6            41.4            40.6                
        BTreeMap            46.1            74.1            74.1            75.2            73.0            72.2                
         HashMap            63.7            78.0            78.0            65.7            52.6            77.8                
    DynNibbleOpt            67.7            44.2            66.5            79.8           108.6            88.0                
         BitTrie            76.8            71.7            57.3            78.6            62.9            58.7                
   DynNibbleTrie            76.8            72.4            72.4           102.8           109.7            96.2                
        PolyTrie            97.4            76.2            95.1           102.0            83.8            93.5            12.9
       NibbleOpt           112.5            62.1           125.1           120.6           156.2           126.1                
      NibbleTrie           121.6           131.1           131.1           124.5           157.3           134.2                

─── Optimize (keys/sec) ───
                              10             100            1000           10000          100000         1000000        10000000
       NibbleOpt          20.35M          23.65M          17.49M           7.86M           4.28M           5.98M                
    DynNibbleOpt          19.45M          21.28M          15.30M           7.93M           4.30M           5.53M                
        PolyTrie           8.49M           7.32M           3.75M           2.78M           2.18M           2.35M          18.10M

