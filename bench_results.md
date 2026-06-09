─── Insertion (keys/sec) ───
                   10000      100000    10000000
    TinyTrie       1.20M       1.35M       1.72M
    BTreeMap       4.76M       3.51M       3.18M
     HashMap       7.70M       3.88M       1.75M
   SortedVec       6.56M       5.71M       3.32M

─── Lookup (keys/sec) ───
                   10000      100000    10000000
    TinyTrie      27.53M      19.71M      16.40M
    BTreeMap      16.69M      13.01M       9.33M
     HashMap      57.60M      14.17M       7.47M
   SortedVec      11.60M       9.00M       6.04M

─── Iter forward (keys/sec) ───
                   10000      100000    10000000
    TinyTrie     171.66M      88.90M      51.29M
    BTreeMap     776.74M     314.47M      46.15M
   SortedVec       4.54G       4.63G       4.62G

─── Iter backward (keys/sec) ───
                   10000      100000    10000000
    TinyTrie     247.12M     131.20M      59.88M
    BTreeMap     779.74M     291.17M      44.85M

─── Memory (bytes/key) ───
                   10000      100000    10000000
    TinyTrie        81.0        71.5        85.2
    BTreeMap        71.6        72.6        74.6
     HashMap        62.1        52.3        66.4
   SortedVec        40.0        41.0        43.0



## New Results

─── Insertion (keys/sec) ───
                   10000      100000    10000000
    TinyTrie       1.58M       1.92M       2.12M
    BTreeMap       4.92M       3.09M       2.94M
     HashMap       7.90M       3.46M       1.45M
   SortedVec       6.76M       6.10M       2.88M

─── Lookup (keys/sec) ───
                   10000      100000    10000000
    TinyTrie      20.68M      13.16M       9.73M
    BTreeMap      17.38M      13.16M       8.71M
     HashMap      53.65M      12.69M       5.96M
   SortedVec      11.37M       9.11M       6.12M

─── Iter forward (keys/sec) ───
                   10000      100000    10000000
    TinyTrie     194.53M      67.78M      38.94M
    BTreeMap     867.11M     350.26M      39.94M
   SortedVec       4.51G       4.70G       4.81G

─── Iter backward (keys/sec) ───
                   10000      100000    10000000
    TinyTrie     310.44M      84.62M      47.27M
    BTreeMap     827.34M     332.51M      39.37M

─── Memory (bytes/key) ───
                   10000      100000    10000000
    TinyTrie        91.6        82.2        95.9
    BTreeMap        71.6        72.6        74.6
     HashMap        62.1        52.3        66.4
   SortedVec        40.0        41.0        43.0