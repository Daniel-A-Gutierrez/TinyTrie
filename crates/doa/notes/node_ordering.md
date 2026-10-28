# In Order

A degree-4 B+Tree (max 4 children → max 3 keys per node). 16 keys: `2,5,8,11,14,17,20,23,26,29,32,35,38,41,44,47`.

```
                                            ┌─────────────────────────┐
                                            │  #0  (root, internal)   │
                                            │  keys: [ 38 ]           │
                                            │  ptrs:     #1 ,     #2  │
                                            └───────────┬─────────────┘
                                                        │
              ┌─────────────────────────────────────────┴────────────────────────────────────┐
              │ #1                                                                           │ #2
              ▼                                                                              ▼
  ┌────────────────────────────────────────────────────────────────┐              ┌────────────────────────┐
  │                       #1  (internal)                           │              │  #2  (internal)        │
  │                 keys: [ 11 , 20 , 29 ]                         │              │  keys: [ 47 ]          │
  │                 ptrs: ►#3 , ►#4 , ►#5 , ►#6                    │              │  ptrs: ►#7 , ►#8       │
  └───┬─────────────────┬───────────────────┬────────────────────┬─┘              └───────┬────────┬───────┘
    #3│               #4│                 #5│                  #6│                      #7│      #8│
      ▼                 ▼                   ▼                    ▼                        ▼        ▼
  ┌─────────┐     ┌─────────────┐     ┌─────────────┐     ┌─────────────┐     ┌─────────────┐     ┌────────┐
  │ #3 leaf │     │ #4 leaf     │     │ #5 leaf     │     │ #6 leaf     │     │ #7 leaf     │     │ #8 leaf│
  │[2,5,8]  │     │[11,14,17]   │     │[20,23,26]   │     │[29,32,35,36]│     │[38,41,44]   │     │[47]    │
  └─────────┘     └─────────────┘     └─────────────┘     └─────────────┘     └─────────────┘     └────────┘
                                                                                                          ∅
Search rule: at an internal node, descend the child whose bracket the key falls in — e.g. `25` at #1 sits between `20` and `29` → take the →#5 pointer → leaf `[20,23,26]` ✓. 
Leaves carry all the data; internals are just separators, and the leaf `next` chain gives you ordered iteration without revisiting internals.
```

# Node Walk Order

## In Order 
Walk to Leftmost
V = Visit,  P = Pop
Using mid = len/2
v0, v1, v3, p3, v4, p4, p1?, v5, p5, v6, p6, p0?, v2, v7, p7, p2?, p8
[3,4,1,5,6,0,7,2,8]
Using mid = Degree/2
v0, v1, v3, p3, v4, p4, p1?, v5, p5, v6, p6, v2, v7, p7, p8, p2, p0
[3,4,1,5,6,7,8,2,0]

## Pre Order
Visit = pop
[0,1,3,4,5,6,2,7,8]

## Post Order
Visit = pop
[3,4,5,6,1,7,8,2,0]

## Breadth First
[0,1,2,3,4,5,6,7,8]

# Node Insert / Split Fixup
Say 6 splits, cascading up to 1 splitting and inserting a new sibling between 1 and 2. This will be 9. 
6 is split into 6 and 10. 
1 now parents 3, 4, and 9. 
9 now parents 5, 6, and 10. 
How do we maintain the ordering of nodes, if the nodes are ordered by their walk order?

## In Order
Degree Mid
[3,4,1,5,6,9,10,0,7,2,8]
Notes : 9 and 10, the new nodes, just get inserted. neat. 9 goes after 1's former left children and 1 doesnt need to move so... makes sense.
They won't be sequential if the tree was taller though, 10 would have space between it and 9, its left subtree goes there. 
Also, 0 has to go from being after 7,2,8 to before it. 
Basically it has to 'reseek' to remain between its children, because a left child split. 

## Pre Order
Trivial , literally just insert the new child, order maintained.
9 has to go after 4's subtree. 
[0,1,3,4,9,5,6,10,2,7,8]

## Post Order
[3,4,1,5,6,10,9,7,8,2,0]
1 moves between 4 and 5 since its no longer 5's parent
9 is inserted, 10 to its left afterward. 

## Breadth First
[0,1,9,2,3,4,5,6,10,7,8]
Basically just two disparate inserts. 

# How To Build The Ordering
How does the arena know where to insert a new node? Or if a node is split, what to do? 

## Pre Order
This ones been simplest so far so...
After the previous child's subtree right? 
And if theres no previous child, just directly after the parent. 

## Breadth first
We pretty much need to be bounded, if the insert isnt the root.
After prev rightmost where prev is the prev child or the cousins last child. 
You have to go sideways until you find a node with a child then descend to its rightmost, insert after that. 
If its going in a new tier, it goes after the parents rightmost sibling. 

## Post Order
Traverse to parent, insert after rightmost descendant of prior child. 
if no children, insert before parent. 

# Fixup
1. walk the moved nodes, offsetting their parents ptrs to them by 1. 
    afterward, fixup any nodes in the walker that were in the moved run. 
2. build a map of [(old,new)] ordered by visitation order, pop it as we walk over the offset nodes, then apply it to the walker. 
3. forget maintaining a parent stack, use the nodes built in parent ptr to derive a 'frame' as we walk linearly in the block.

perhaps a fixed size array of size BUDGET can be used instead of a dynamically allocated one? 
maybe we can update the walker as we walk over the offset area to do the fixup? 
No cant , it has to happen after or we corrupt the walker. 
I think the 'build up then apply' deltas using a fixed size buffer has gotta be cleaner. 


# Conclusion 
A node cannot know the position that a child or parent of it must be inserted at , its not a local property. 
It requires the walker to be able to probe to the leftmost and rightmost child of a node, to do that repeatedly and find the boundaries of subtrees. 
For bfo, leftmost and rightmost could support a depth limit, which would let us get the positions of a 'tier' of nodes. 
FixupStack would be useful as well. 
I think we get this stuff by just replacing NodeIter with DoubleEndedIter, and also supporting a direct seek. 

Building : Lazy Insertion
┌───────┬────────┬──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│   #   │ insert │                                                                                 side effects                                                                                 │
├───────┼────────┼──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 1–3   │ 2,5,8  │ [#3] grows to [2,5,8] (full)                                                                                                                                                 │
├───────┼────────┼──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 4     │ 11     │ split leaf #3 → #3 [2,5,8] | #4 [11], promote 11; root leaf → internal root [11] → #3,#4                                                                                     │
├───────┼────────┼──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 5–6   │ 14,17  │ [#4] grows to [11,14,17] (full)                                                                                                                                              │
├───────┼────────┼──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 7     │ 20     │ split leaf #4 → #4 [11,14,17] | #5 [20], promote 20; root [11,20] → #3,#4,#5                                                                                                 │
├───────┼────────┼──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 8–9   │ 23,26  │ [#5] grows to [20,23,26] (full)                                                                                                                                              │
├───────┼────────┼──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 10    │ 29     │ split leaf #5 → #5 [20,23,26] | #6 [29], promote 29; root [11,20,29] → #3,#4,#5,#6 (full: 4 children)                                                                        │
├───────┼────────┼──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 11–12 │ 32,35  │ [#6] grows to [29,32,35] (full)                                                                                                                                              │
├───────┼────────┼──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 13    │ 38     │ split leaf #6 → #6 [29,32,35] | #7 [38], promote 38 → root overflows [11,20,29,38] → split root: #1 [11,20,29] (→#3,#4,#5,#6), #2 [] (→#7), promote 38 → new root #0 [38] →  │
│       │        │ #1,#2; height +1                                                                                                                                                             │
├───────┼────────┼──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 14–15 │ 41,44  │ [#7] grows to [38,41,44] (full)                                                                                                                                              │
├───────┼────────┼──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 16    │ 36     │ [#6] grows to [29,32,35,36] (full)                                                                                                                                              │
├───────┼────────┼──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ 17    │ 47     │ split leaf #7 → #7 [38,41,44] | #8 [47], promote 47; #2 [47] → #7,#8                                                                                                         │
└───────┴────────┴──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
# Walker Pseudocode

Vec-arena + raw indexes. Nodes are unions (no stored tag); leaf-ness is derived from `height` carried in each walker's state (leaves at `height == 0`, root at `Arena::height`). Descending is `height - 1`. Union access is `unsafe` — the variant is positional, not stored.

```rust
union Node {
    leaf:     Leaf,
    internal: Internal,
}
struct Leaf     { keys: Vec<Key> }
struct Internal { keys: Vec<Key>, children: Vec<usize> }
struct Arena { nodes: Vec<Node>, height: usize }   // height of the root
const DEGREE: usize = 4;
const MIDPOINT: usize = DEGREE / 2;

// ── Pre Order ──  [0,1,3,4,5,6,2,7,8]
struct PreWalker<'a> { arena: &'a Arena, stack: Vec<(usize, usize)> }   // (node, height)
impl<'a> PreWalker<'a> {
    fn new(arena: &'a Arena, root: usize) -> Self {
        Self { arena, stack: vec![(root, arena.height)] }
    }
    fn next(&mut self) -> Option<usize> {
        let (node, height) = self.stack.pop()?;
        if height > 0 {
            let children = unsafe { &self.arena.nodes[node].internal.children };
            for child in children.iter().rev() { self.stack.push((*child, height - 1)); }
        }
        Some(node)
    }
}

// ── Post Order ──  [3,4,5,6,1,7,8,2,0]
#[derive(Clone, Copy)]
struct PostFrame { node: usize, height: usize, child_idx: usize }
struct PostWalker<'a> { arena: &'a Arena, stack: Vec<PostFrame> }
impl<'a> PostWalker<'a> {
    fn new(arena: &'a Arena, root: usize) -> Self {
        Self { arena, stack: vec![PostFrame{node:root, height:arena.height, child_idx:0}] }
    }
    fn next(&mut self) -> Option<usize> {
        loop {
            let frame = *self.stack.last()?;
            if frame.height == 0 { self.stack.pop(); return Some(frame.node); }     // leaf
            let children = unsafe { &self.arena.nodes[frame.node].internal.children };
            if frame.child_idx >= children.len() { self.stack.pop(); return Some(frame.node); }
            let child = children[frame.child_idx];
            self.stack.last_mut().unwrap().child_idx += 1;
            self.stack.push(PostFrame{node:child, height:frame.height - 1, child_idx:0});
        }
    }
}

// ── In Order (degree split) ──  [3,4,1,5,6,7,8,2,0]
#[derive(Clone, Copy)]
struct InFrame { node: usize, child_idx: usize, emitted: bool, height: usize }
struct InWalker<'a> { arena: &'a Arena, stack: Vec<InFrame> }
impl<'a> InWalker<'a> {
    fn new(arena: &'a Arena, root: usize) -> Self {
        Self { arena, stack: vec![InFrame{node:root, child_idx:0, emitted:false, height:arena.height}] }
    }
    fn next(&mut self) -> Option<usize> {
        loop {
            let frame = *self.stack.last()?;
            if frame.height == 0 { self.stack.pop(); return Some(frame.node); }    // leaf
            let children = unsafe { &self.arena.nodes[frame.node].internal.children };
            if frame.child_idx < children.len() {
                if frame.child_idx == MIDPOINT && !frame.emitted {                 // self at split point
                    self.stack.last_mut().unwrap().emitted = true;
                    return Some(frame.node);
                }
                let child = children[frame.child_idx];
                self.stack.last_mut().unwrap().child_idx += 1;
                self.stack.push(InFrame{node:child, child_idx:0, emitted:false, height:frame.height - 1});
            } else if !frame.emitted {                                            // MIDPOINT ≥ len ⇒ self last
                self.stack.last_mut().unwrap().emitted = true;
                return Some(frame.node);
            } else { self.stack.pop(); }
        }
    }
}

// ── Breadth First ──  [0,1,2,3,4,5,6,7,8]
struct BfsWalker<'a> { arena: &'a Arena, queue: VecDeque<(usize, usize)> }        // (node, height)
impl<'a> BfsWalker<'a> {
    fn new(arena: &'a Arena, root: usize) -> Self {
        Self { arena, queue: [(root, arena.height)].into_iter().collect() }
    }
    fn next(&mut self) -> Option<usize> {
        let (node, height) = self.queue.pop_front()?;
        if height > 0 {
            let children = unsafe { &self.arena.nodes[node].internal.children };
            for child in children { self.queue.push_back((*child, height - 1)); }
        }
        Some(node)
    }
}
```
