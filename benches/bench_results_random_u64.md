
─── Insertion (keys/sec) ───
                              10             100            1000           10000
      LinkedList           8.36M           8.32M             0.0             0.0
       SortedVec           6.15M           4.01M           6.67M           1.35M
        BTreeMap           3.87M           2.45M          14.67M           6.30M
         BitTrie           2.57M           2.00M          14.82M           8.35M
      NibbleTrie           2.17M          666.3K             0.0             0.0
   DynNibbleTrie           2.10M          690.4K             0.0             0.0
         HashMap           1.79M           1.80M          15.89M          14.97M

─── Iter backward (keys/sec) ───
                              10             100            1000           10000
        BTreeMap         408.46M         980.69M           1.12G         624.99M
         BitTrie          92.19M         143.61M         154.45M         102.95M

─── Iter forward (keys/sec) ───
                              10             100            1000           10000
       SortedVec         495.55M           2.50G           4.31G           4.78G
        BTreeMap         400.14M         854.70M         928.07M         689.17M
         BitTrie          90.60M         146.00M         157.21M         105.38M

─── Iter fwd index (keys/sec) ───
                              10             100            1000           10000

─── Iter rev index (keys/sec) ───
                              10             100            1000           10000

─── Lookup (keys/sec) ───
                              10             100            1000           10000
         BitTrie         130.32M          77.40M          41.46M          25.07M
         HashMap          75.41M          79.09M          76.24M          56.96M
        BTreeMap          74.14M          40.27M          28.46M           9.70M
       SortedVec          59.19M          31.95M          17.53M           8.98M

─── Memory (bytes/key) ───
                              10             100            1000           10000
       SortedVec            40.0            40.0            40.0            40.0
        BTreeMap            44.8            53.1            59.2            58.3
         HashMap            62.4            50.4            75.6            62.1
         BitTrie            76.8            61.4            49.2            78.6

─── Optimize (keys/sec) ───
                              10             100            1000           10000

