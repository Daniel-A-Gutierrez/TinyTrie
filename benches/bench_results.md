
─── Insertion (keys/sec) ───
                   10000      100000    10000000
    BTreeMap       9.94M       6.75M       4.40M
     BitTrie       5.25M       4.40M       2.66M
     HashMap      16.13M       7.05M       1.79M
  NibbleTrie      13.27M      14.66M      12.32M
    PolyTrie       6.21M       5.57M       4.43M
   SortedVec       9.56M       7.67M       2.78M
    TinyTrie       5.55M       4.91M       3.40M

─── Iter backward (keys/sec) ───
                   10000      100000    10000000
    BTreeMap     691.18M     149.26M      43.36M
     BitTrie     155.36M     149.19M     146.03M
   NibbleOpt     187.17M     183.77M     184.91M
  NibbleTrie     183.72M     179.40M     173.65M
     PolyOpt     103.41M     102.19M     102.32M
    PolyTrie     101.68M      95.63M      91.00M
    TinyTrie     206.20M     128.96M      47.57M

─── Iter forward (keys/sec) ───
                   10000      100000    10000000
    BTreeMap     764.78M     135.03M      42.90M
     BitTrie     161.55M     154.39M     145.51M
   NibbleOpt     176.73M     174.91M     172.69M
  NibbleTrie     177.55M     174.73M     173.81M
     PolyOpt     122.54M     121.31M     121.07M
    PolyTrie     122.90M     118.20M     115.04M
   SortedVec       2.44G       2.44G       2.45G
    TinyTrie     207.48M     114.47M      48.48M

─── Lookup (keys/sec) ───
                   10000      100000    10000000
    BTreeMap      16.00M      10.51M       7.48M
     BitTrie      13.32M       9.29M       7.42M
     HashMap      56.56M      12.76M       6.77M
   NibbleOpt      31.61M      27.54M      23.42M
  NibbleTrie      30.42M      17.93M      15.78M
     PolyOpt      22.84M      19.81M      15.86M
    PolyTrie      21.12M      14.23M      11.82M
   SortedVec      11.40M       9.20M       6.35M
    TinyTrie      19.73M      12.57M       9.46M

─── Memory (bytes/key) ───
                   10000      100000    10000000
    BTreeMap        71.6        72.6        74.6
     BitTrie        81.1        67.7        85.8
     HashMap        62.1        52.3        66.4
   NibbleOpt        76.2        63.7        80.8
  NibbleTrie        76.2        63.7        80.8
     PolyOpt        80.1        70.6        84.4
    PolyTrie        81.1        83.4        85.8
   SortedVec        40.0        41.0        43.0
    TinyTrie        91.6        82.2        95.9

─── Optimize (keys/sec) ───
                   10000      100000    10000000
   NibbleOpt      15.33M      14.09M       7.85M
     PolyOpt       6.21M       4.30M       3.48M

