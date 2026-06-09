─── Insertion (keys/sec) ───
                   10000      100000    10000000
    TinyTrie       1.16M       1.12M       1.63M
  NibbleTrie       3.18M       3.37M       4.24M
    BTreeMap       5.08M       3.95M       3.24M
     HashMap       6.33M       4.50M       1.84M
   SortedVec       5.73M       5.20M       3.33M

─── Lookup (keys/sec) ───
                   10000      100000    10000000
    TinyTrie      21.32M      16.27M      11.15M
  NibbleTrie      31.05M      22.74M      18.67M
    BTreeMap      17.58M      13.80M       9.97M
     HashMap      60.72M      16.27M       8.19M
   SortedVec      12.23M       9.49M       6.22M

─── Iter forward (keys/sec) ───
                   10000      100000    10000000
    TinyTrie     211.33M     136.51M      58.46M
  NibbleTrie     184.14M     182.38M     178.51M
    BTreeMap     821.57M     358.95M      48.37M
   SortedVec       4.71G       4.78G       4.75G

─── Iter backward (keys/sec) ───
                   10000      100000    10000000
    TinyTrie     204.27M     144.54M      57.50M
  NibbleTrie     184.10M     181.40M     175.88M
    BTreeMap     823.49M     348.65M      48.31M

─── Memory (bytes/key) ───
                   10000      100000    10000000
    TinyTrie        91.6        82.2        95.9
  NibbleTrie        76.2        63.7        80.8
    BTreeMap        71.6        72.6        74.6
     HashMap        62.1        52.3        66.4
   SortedVec        40.0        41.0        43.0
