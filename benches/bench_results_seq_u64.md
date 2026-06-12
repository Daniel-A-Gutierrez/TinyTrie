
─── Insertion (keys/sec) ───
                              10             100            1000           10000
      LinkedList           8.25M           8.32M             0.0             0.0
       SortedVec           6.01M           3.89M           5.18M          814.9K
        BTreeMap           4.54M           3.42M          16.60M          14.80M
         BitTrie           2.37M           2.22M          20.33M          12.83M
      NibbleTrie           2.09M           1.38M             0.0             0.0
   DynNibbleTrie           2.06M           1.40M             0.0             0.0
         HashMap           1.75M           1.77M          16.61M          16.71M

─── Iter backward (keys/sec) ───
                              10             100            1000           10000
        BTreeMap         406.94M         957.80M           1.03G         876.58M
         BitTrie          88.25M         145.73M         152.73M         153.22M

─── Iter forward (keys/sec) ───
                              10             100            1000           10000
       SortedVec         492.07M           2.54G           4.35G           4.75G
        BTreeMap         398.08M         841.57M         929.14M         898.93M
         BitTrie          88.82M         152.05M         159.70M         157.17M

─── Iter fwd index (keys/sec) ───
                              10             100            1000           10000

─── Iter rev index (keys/sec) ───
                              10             100            1000           10000

─── Lookup (keys/sec) ───
                              10             100            1000           10000
         BitTrie         109.91M          65.83M          42.41M          27.93M
         HashMap          75.23M          79.80M          75.70M          59.50M
        BTreeMap          74.19M          49.55M          34.14M          17.81M
       SortedVec          59.49M          31.78M          18.14M          11.98M

─── Memory (bytes/key) ───
                              10             100            1000           10000
       SortedVec            40.0            40.0            40.0            40.0
        BTreeMap            44.8            73.4            71.8            71.6
      LinkedList            56.0            56.0             0.0             0.0
    DynNibbleOpt            59.3            43.9             0.0             0.0
         HashMap            62.4            50.4            75.6            62.1
   DynNibbleTrie            65.6            47.4             0.0             0.0
         BitTrie            76.8            61.4            49.2            78.6
       NibbleOpt            81.7            52.8             0.0             0.0
      NibbleTrie            88.0            56.3             0.0             0.0

─── Optimize (keys/sec) ───
                              10             100            1000           10000

