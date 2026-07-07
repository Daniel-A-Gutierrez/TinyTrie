Home of a handful of experimental datastructures under active development. 

Conventional radix trees save memory and enhance lookup performance by deduplicating shared prefixes between keys, and storing only the varying suffixes at each node. 
Iteration suffers, because the key has to be reconstructed between siblings (pop the old suffix, append the current one). 
Also, another inefficiency arises from this approach - suffixes are not a fixed size, so they need to be stored out of line of the node and fetched. 

The core concept of the trees in this crate is instead to store a single discriminating bit/nib/nibble and the position it occurs at in the key string, within the tree. 
Instead of using comparison to search, we directly address in a child table by the content of the bit/nib/nibble (word). 
Because of the birthday problem, node occupancy hovers around n^.5 in real data, with nodes closer to the root being closer to full occupancy. 
As a result, smaller 'words' than bytes are necessitated - a 255 byte table would likely only have 16 occupied slots, but a 4 entry table may have 2.
Hence why theres a nibble (4 bits), nib (2 bit) and bit trie, but no byte tree.

The memory usage of nibble_trie is currently quite poor, shrinking it is one of my current priorities. 

For example, using hexadecimal strings and a query '0x123f' , we might come across a node like this : 
```
Node : {
    prefix_len : 2
    leaf : (leaf_addr)
    terminal : false
    children : [None, (leaf_addr), None, (node_addr), ...]
}
```

```
//simplified walk to leaf node. 
node = root;
while let Some(current_node) = node {
    if prefix_len == query.len() { if  current_node.terminal {return current_node.leaf} else {return None;} }
    if let Some(Leaf(l)) = current_node.children[query as usize] { return l ; } 
    else { node = node.children[query as usize] ;} 
}
return node;
```

During insertion, the full keys must be looked up so the nodes store a 'shortcut' to the leftmost leaf descendant. 

Bit trie shines with longer keys, nibble trie with shorter ones , nib trie is in between.

All the trees are generic over their internal pointer type and will return an error if the arena is full. 
The trees can be 'promoted' or 'demoted' to increase/decrease the PTR width, allowing small trees to be very lean. 
Dyn tree takes advantage of this but requires a vtable lookup on each call, negligible for insert/lookup but painful for iteration. 