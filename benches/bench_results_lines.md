
─── Insertion (keys/sec) ───
                              10             100            1000           10000
       SortedVec          39.19M          21.13M          13.47M          12.72M
        BTreeMap          30.80M          19.63M          13.22M                
         HashMap          20.85M          14.09M           9.60M                
   DynNibbleTrie          12.01M           8.69M           8.90M                
      NibbleTrie          11.34M           8.62M           9.12M                

─── Iter backward (keys/sec) ───
                              10             100            1000           10000
        BTreeMap         418.42M         986.16M           1.03G                
   DynNibbleTrie          78.63M         124.48M         126.40M                
      NibbleTrie          68.53M         103.52M         109.18M                

─── Iter forward (keys/sec) ───
                              10             100            1000           10000
       SortedVec         508.92M           2.66G           4.50G           7.23G
        BTreeMap         415.26M         870.94M         956.46M                
   DynNibbleTrie          69.78M         111.72M         106.73M                
      NibbleTrie          67.61M          92.95M         108.78M                

─── Iter fwd index (keys/sec) ───
                              10             100            1000           10000
      NibbleTrie          71.73M         100.66M         133.82M                

─── Iter rev index (keys/sec) ───
                              10             100            1000           10000
      NibbleTrie          83.04M         125.32M         141.50M                

─── Lookup (keys/sec) ───
                              10             100            1000           10000
 NibbleUnchecked          62.26M          26.60M          23.51M                
         HashMap          60.62M          47.09M          45.42M                
   DynNibbleTrie          59.84M          25.67M          21.07M                
        BTreeMap          58.03M          42.88M          31.38M                
      NibbleTrie          56.78M          24.39M          21.63M                
       SortedVec          51.30M          27.83M          18.94M          11.80M

─── Memory (bytes/key) ───
                              10             100            1000           10000
       SortedVec            67.3            97.8            89.5            59.3
        BTreeMap            72.1           131.2           121.3                
         HashMap            89.7           108.2           125.1                
   DynNibbleTrie           140.8           153.6           152.3                
      NibbleTrie           230.4           225.3           180.2                

─── Optimize (keys/sec) ───
                              10             100            1000           10000

