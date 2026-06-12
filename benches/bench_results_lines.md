
─── Insertion (keys/sec) ───
                              10             100            1000           10000          100000
       SortedVec          34.36M          21.13M          13.47M          12.72M                
        BTreeMap          26.28M          19.63M          13.62M          12.79M                
         BitTrie          18.82M          14.15M          14.19M          18.10M                
         HashMap          17.95M          14.09M           9.45M          15.74M                
   DynNibbleTrie          12.01M           8.69M           9.35M          14.39M                
      NibbleTrie          11.34M           8.62M           9.65M           9.46M                
        PolyTrie           9.80M           5.79M           4.61M           2.29M           1.95M

─── Iter backward (keys/sec) ───
                              10             100            1000           10000          100000
        BTreeMap         425.16M         986.16M           1.01G           1.18G                
         BitTrie          94.46M         157.11M         171.35M         250.87M                
   DynNibbleTrie          78.63M         124.48M         123.43M         180.47M                
      NibbleTrie          68.53M         103.52M         109.12M         156.10M                
        PolyTrie          65.00M          98.09M          99.91M         100.74M          56.18M
    DynNibbleOpt             0.0             0.0         125.47M         181.86M                
       NibbleOpt             0.0             0.0         109.92M         158.74M                

─── Iter forward (keys/sec) ───
                              10             100            1000           10000          100000
       SortedVec         513.62M           2.66G           4.50G           7.23G                
        BTreeMap         416.24M         870.94M         920.11M           1.29G                
         BitTrie          91.66M         161.17M         171.90M         226.11M                
   DynNibbleTrie          69.78M         111.72M         109.44M         159.64M                
      NibbleTrie          67.61M          92.95M         107.81M         153.13M                
        PolyTrie          66.31M          95.41M          98.83M          98.31M          57.90M
    DynNibbleOpt             0.0             0.0         110.85M         159.84M                
       NibbleOpt             0.0             0.0         107.95M         153.57M                

─── Iter fwd index (keys/sec) ───
                              10             100            1000           10000          100000
      NibbleTrie          71.73M         100.66M         132.47M         188.89M                
       NibbleOpt             0.0             0.0         130.94M         190.14M                

─── Iter rev index (keys/sec) ───
                              10             100            1000           10000          100000
      NibbleTrie          83.04M         125.32M         139.56M         201.54M                
       NibbleOpt             0.0             0.0         139.90M         203.41M                

─── Lookup (keys/sec) ───
                              10             100            1000           10000          100000
         BitTrie         104.62M          24.20M          16.11M          12.11M           6.72M
        PolyTrie          81.77M          33.04M          27.25M           6.79M           4.25M
        BTreeMap          68.84M          42.88M          32.48M          16.43M           7.93M
 NibbleUnchecked          62.26M          26.60M          23.51M                                
   DynNibbleTrie          59.84M          25.67M          19.53M          12.47M                
       SortedVec          56.81M          27.83M          18.94M          11.80M           5.93M
      NibbleTrie          56.78M          24.39M          21.52M          13.00M                
         HashMap          48.21M          47.09M          45.06M          31.50M           6.89M
    DynNibbleOpt             0.0             0.0          20.14M          12.50M                
       NibbleOpt             0.0             0.0          21.10M          12.46M                
NibbleOptUnchecked             0.0             0.0          22.91M          15.55M                

─── Memory (bytes/key) ───
                              10             100            1000           10000          100000
    DynNibbleOpt             0.0             0.0           144.3            81.8                
       NibbleOpt             0.0             0.0           172.2           129.0                
       SortedVec            97.5            97.8            89.5            59.3                
        BTreeMap           102.3           131.2           121.3            81.1                
         HashMap           119.9           108.2           125.1            64.3                
   DynNibbleTrie           140.8           153.6           152.3            97.0                
         BitTrie           150.4           133.1           106.5            85.2                
        PolyTrie           159.3           157.7           160.8           310.4           264.9
      NibbleTrie           230.4           225.3           180.2           144.2                

─── Optimize (keys/sec) ───
                              10             100            1000           10000          100000
        PolyTrie           8.64M           5.02M           5.22M           2.46M           1.71M
    DynNibbleOpt             0.0             0.0           8.48M           8.28M                
       NibbleOpt             0.0             0.0           8.78M           8.32M                

