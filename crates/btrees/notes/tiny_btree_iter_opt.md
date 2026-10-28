note for later on - prev and next should be
Option<NonZero<PTR>> as KeyNodes ptrs are. Also though,
arent leaf nodes made in a particular order? Since a b+tree
is a fixed height thing, the bottom tier should become
leaf nodes at each increase in height right? Which implies
they could follow an ordering like in an S tree

inodes :                    [new root]                         [root3] 
                                                    [new l child] [new r child]
leaves : [leaf root] -> [l child 1] [r child 1] -> [l1] [new l]       [r1]   [new r]

so with some care in how we split a node up the tree, we can keep the leaves ordered completely i think. 

