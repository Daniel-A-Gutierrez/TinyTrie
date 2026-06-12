
─── Insertion (keys/sec) ───
                              10             100            1000           10000
       SortedVec          31.89M          19.85M          12.96M           8.32M
        BTreeMap          23.65M          13.89M          14.02M           8.76M
      NibbleTrie          22.26M          20.43M          20.99M          12.00M
         HashMap          21.43M          12.11M          12.41M          12.12M
   DynNibbleTrie          21.16M          18.36M          18.60M          11.31M
         BitTrie          21.07M          21.64M          18.70M          10.84M

─── Iter backward (keys/sec) ───
                              10             100            1000           10000
        BTreeMap         420.85M         990.69M         995.74M         799.14M
   DynNibbleTrie          98.26M         132.20M         131.81M         133.16M
         BitTrie          94.88M         158.75M         165.73M         163.41M
      NibbleTrie          89.38M         114.36M         114.82M         112.32M
    DynNibbleOpt             0.0             0.0         131.46M         133.34M
       NibbleOpt             0.0             0.0         114.56M         112.96M

─── Iter forward (keys/sec) ───
                              10             100            1000           10000
       SortedVec         511.35M           2.60G           4.48G           4.98G
        BTreeMap         413.32M         947.74M         924.66M         875.02M
         BitTrie          93.17M         163.23M         175.56M         173.09M
   DynNibbleTrie          84.67M         112.54M         112.00M         110.73M
      NibbleTrie          77.00M         111.18M         109.97M         108.05M
    DynNibbleOpt             0.0             0.0         112.14M         110.55M
       NibbleOpt             0.0             0.0         109.83M         109.46M

─── Iter fwd index (keys/sec) ───
                              10             100            1000           10000
      NibbleTrie         101.16M         143.41M         141.80M         140.56M
       NibbleOpt             0.0             0.0         141.82M         141.39M

─── Iter rev index (keys/sec) ───
                              10             100            1000           10000
      NibbleTrie         110.78M         150.65M         151.48M         151.85M
       NibbleOpt             0.0             0.0         152.12M         151.92M

─── Lookup (keys/sec) ───
                              10             100            1000           10000
 NibbleUnchecked         135.22M          60.29M          43.66M                
         BitTrie         128.70M          50.41M          27.72M          17.44M
   DynNibbleTrie         123.84M          50.17M          50.11M          25.86M
      NibbleTrie         119.01M          53.06M          53.23M          26.25M
        BTreeMap          82.23M          35.32M          35.11M          16.64M
         HashMap          78.90M          77.29M          76.65M          50.90M
       SortedVec          57.87M          31.82M          18.73M          12.09M
    DynNibbleOpt             0.0             0.0          49.03M          25.52M
       NibbleOpt             0.0             0.0          52.66M          26.29M
NibbleOptUnchecked             0.0             0.0          60.39M          43.32M

─── Memory (bytes/key) ───
                              10             100            1000           10000
    DynNibbleOpt             0.0             0.0            66.5           101.3
       NibbleOpt             0.0             0.0           125.1           123.0
       SortedVec            41.3            44.1            42.4            43.6
        BTreeMap            46.1            74.1            74.1            75.2
         HashMap            63.7            78.0            78.0            65.7
         BitTrie            76.8            71.7            57.3            78.6
   DynNibbleTrie            76.8            72.4            72.4           102.8
      NibbleTrie           121.6           131.1           131.1           124.5

─── Optimize (keys/sec) ───
                              10             100            1000           10000
    DynNibbleOpt             0.0             0.0          15.30M           8.73M
       NibbleOpt             0.0             0.0          17.49M           9.34M

