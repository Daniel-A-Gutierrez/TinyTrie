This workspace is home to a number of datastructure experiments, with the goal of developing fast, concurrent, serializing map structures suitable for use in a database as indexes.

Notably, the trees in tiny-trie reliably beat rusts BTreeMap collection, as well as my own highly optimized implementation. 

notes/_summary.md contains the results of various optimizations/ historical benchmark runs, benches contains the most recent run of various benchmarks by key type.

words/lines use the plaintext content of wikipedia as a corpus. 