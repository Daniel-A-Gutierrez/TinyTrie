
─── Insertion (keys/sec) ───
                              10             100            1000           10000
       SortedVec          31.89M          19.85M          12.96M           8.32M
        BTreeMap          23.65M          13.89M           8.42M                
      NibbleTrie          22.26M          20.43M          11.80M                
         HashMap          21.43M          12.11M          11.66M                
   DynNibbleTrie          21.16M          18.36M          11.07M                

─── Iter backward (keys/sec) ───
                              10             100            1000           10000
        BTreeMap         420.85M         990.69M         806.56M                
   DynNibbleTrie          98.26M         132.20M         133.52M                
      NibbleTrie          89.38M         114.36M         113.22M                

─── Iter forward (keys/sec) ───
                              10             100            1000           10000
       SortedVec         511.35M           2.60G           4.48G           4.98G
        BTreeMap         413.32M         947.74M         882.40M                
   DynNibbleTrie          84.67M         112.54M         110.64M                
      NibbleTrie          77.00M         111.18M         109.82M                

─── Iter fwd index (keys/sec) ───
                              10             100            1000           10000
      NibbleTrie         101.16M         143.41M         141.25M                

─── Iter rev index (keys/sec) ───
                              10             100            1000           10000
      NibbleTrie         110.78M         150.65M         149.80M                

─── Lookup (keys/sec) ───
                              10             100            1000           10000
 NibbleUnchecked         135.22M          60.29M          43.66M                
   DynNibbleTrie         123.84M          50.17M          25.74M                
      NibbleTrie         119.01M          53.06M          26.45M                
        BTreeMap          82.23M          35.32M          16.67M                
         HashMap          78.90M          77.29M          49.41M                
       SortedVec          57.87M          31.82M          18.73M          12.09M

─── Memory (bytes/key) ───
                              10             100            1000           10000
       SortedVec            41.3            44.1            42.4            43.6
        BTreeMap            46.1            74.1            75.2                
         HashMap            63.7            78.0            65.7                
   DynNibbleTrie            76.8            72.4           102.8                
      NibbleTrie           121.6           131.1           124.5                

─── Optimize (keys/sec) ───
                              10             100            1000           10000

