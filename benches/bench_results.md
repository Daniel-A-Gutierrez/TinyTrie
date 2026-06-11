
─── Insertion (keys/sec) ───
                   10000      100000    10000000
    BTreeMap      10.20M       6.46M       4.41M
     BitTrie       5.45M       4.55M       2.29M
     HashMap      16.13M       7.05M       1.79M
  NibbleTrie      17.44M      19.47M      10.91M
    PolyTrie       5.84M       5.62M       4.28M
   SortedVec       9.56M       7.67M       2.78M
    TinyTrie       5.62M       4.70M       3.27M

─── Iter backward (keys/sec) ───
                   10000      100000    10000000
    BTreeMap     901.17M     221.79M      46.52M
     BitTrie     189.21M     177.78M     162.71M
   NibbleOpt     187.17M     183.77M     184.91M
  NibbleTrie     215.17M     213.75M     200.59M
     PolyOpt     103.41M     102.19M     102.32M
    PolyTrie     149.01M     143.70M     113.27M
    TinyTrie     334.58M     196.51M      54.36M

─── Iter forward (keys/sec) ───
                   10000      100000    10000000
    BTreeMap     984.29M     324.12M      41.60M
     BitTrie     188.07M     174.33M     130.41M
   NibbleOpt     176.73M     174.91M     172.69M
  NibbleTrie     218.95M     217.75M     212.61M
     PolyOpt     122.54M     121.31M     121.07M
    PolyTrie     148.62M     147.10M     134.07M
   SortedVec       2.44G       2.44G       2.45G
    TinyTrie     311.99M     179.85M      55.16M

─── Lookup (keys/sec) ───
                   10000      100000    10000000
    BTreeMap      17.53M      11.60M       8.36M
     BitTrie      20.99M      12.32M       8.98M
     HashMap      56.56M      12.76M       6.77M
   NibbleOpt      31.61M      27.54M      23.42M
  NibbleTrie      90.43M      42.88M      26.37M
     PolyOpt      22.84M      19.81M      15.86M
    PolyTrie      34.87M      17.38M      14.03M
   SortedVec      11.40M       9.20M       6.35M
    TinyTrie      20.42M      11.00M       9.58M

─── Memory (bytes/key) ───
                   10000      100000    10000000
    BTreeMap        71.6        72.6        74.6
     BitTrie        81.1        67.7        85.8
     HashMap        62.1        52.3        66.4
   NibbleOpt        76.2        63.7        80.8
  NibbleTrie        76.0        63.4        80.6
     PolyOpt        80.1        70.6        84.4
    PolyTrie        81.1        83.4        85.8
   SortedVec        40.0        41.0        43.0
    TinyTrie        91.6        82.2        95.9

─── Optimize (keys/sec) ───
                   10000      100000    10000000
   NibbleOpt      15.33M      14.09M       7.85M
     PolyOpt       6.21M       4.30M       3.48M

