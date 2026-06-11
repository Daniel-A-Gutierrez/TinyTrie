
─── Insertion (keys/sec) ───
                   10000      100000
    BTreeMap      10.07M       5.76M       3.98M
     BitTrie       5.39M       4.48M       2.22M
     HashMap      16.45M       6.69M       1.35M
  LinkedList      57.99M      55.89M
  NibbleTrie      23.40M      21.06M      19.65M
    PolyTrie       6.02M       4.95M       4.07M
   SortedVec      10.16M       7.90M       2.10M
    TinyTrie       5.50M       4.56M       2.64M

─── Iter backward (keys/sec) ───
                   10000      100000
    BTreeMap     919.08M     313.99M      34.81M
     BitTrie     185.54M     168.02M     144.53M
  LinkedList     674.03M     726.36M
   NibbleOpt     208.39M     206.71M     199.78M
  NibbleTrie     208.96M     207.42M     201.64M
     PolyOpt     146.51M     142.80M     137.12M
    PolyTrie     140.29M     134.65M     101.16M
    TinyTrie     328.66M     170.96M      42.25M

─── Iter forward (keys/sec) ───
                   10000      100000
    BTreeMap     976.02M     283.19M      35.58M
     BitTrie     187.45M     170.28M     120.33M
  LinkedList     685.82M     732.55M
   NibbleOpt     168.53M     167.77M     166.48M
  NibbleTrie     167.15M     168.63M     165.40M
     PolyOpt     149.36M     146.09M     142.76M
    PolyTrie     146.66M     141.21M     126.32M
   SortedVec       2.46G       2.47G       2.44G
    TinyTrie     303.59M     138.72M      41.59M

─── Iter fwd index (keys/sec) ───
                   10000      100000
   NibbleOpt     236.35M     234.51M     234.10M
  NibbleTrie     236.92M     235.75M     234.32M

─── Iter rev index (keys/sec) ───
                   10000      100000
   NibbleOpt     274.74M     272.59M     256.52M
  NibbleTrie     263.97M     261.28M     253.29M

─── Lookup (keys/sec) ───
                   10000      100000
    BTreeMap      16.99M       7.49M       6.34M
     BitTrie      17.86M       9.10M       6.91M
     HashMap      45.69M       8.62M       4.31M
  LinkedList       79.4K        8.1K
   NibbleOpt      81.59M      43.06M      38.62M
  NibbleTrie      81.80M      68.29M      44.32M
     PolyOpt      33.53M      17.63M      13.76M
    PolyTrie      27.60M      10.24M       9.27M
   SortedVec      10.81M       9.24M       6.11M
    TinyTrie      18.81M       7.63M       6.98M

─── Memory (bytes/key) ───
                   10000      100000
    BTreeMap        71.6        72.6        74.6
     BitTrie        81.1        67.7        85.8
     HashMap        62.1        52.3        66.4
  LinkedList        56.0        57.0
   NibbleOpt        43.1        38.9        46.7
  NibbleTrie        57.3        47.2        63.8
     PolyOpt        80.1        70.6        84.4
    PolyTrie        81.1        83.4        85.8
   SortedVec        40.0        41.0        43.0
    TinyTrie        91.6        82.2        95.9

─── Optimize (keys/sec) ───
                   10000      100000
   NibbleOpt      20.31M      17.24M      16.52M
     PolyOpt       6.04M       4.16M       3.29M

