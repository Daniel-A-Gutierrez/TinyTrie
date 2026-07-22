# ArenaNode traits

original intent : surface pointers that need updating when a node is moved or is vaddr changes. 
approach 1 : reverse pointers
Approach 2 : walker

I think a better direction would be to have the node *accept* a function that modifies the ptr internally, instead of yielding the ptr for us to operate on.

actually better yet lets separate it. They take a value, we precompute it and hand it out to multiple places.

The trait can then be 

/// update fields on the node, if it has them
ArenaNode {
    ///generally used by trees, set parent.children[idx] by passing it to f. 
    set_parent_child(&mut self, idx : usize, PTR)
    ///used by trees that store reverse pointers to their parents. 
    set_child_parent(&mut self, PTR)
    ///used by structures that store prev and next ptrs
    set_next(&mut self, PTR)
    set_prev(&mut self, PTR)
    ///used by graphs. assuming you store double edges, when this node has moved follow them in arena and update
    ///the pointers that pointed at OLDPTR using f
    set_inbound(&self, &mut arena, OLDPTR, PTR)
}


then the implementer can just leave the irrelevant ones for their design as nops.
we run all of them and let monomorphization take care of the optimization.

To be able to walk the tree, we need a second trait, ArenaIter

trait ArenaIter {
    current(&self, &arena) -> PTR
    advance(&mut self, &arena)
    reverse(&mut self, &arena)
    peek_next(&self) -> PTR
    peek_prev(&self) -> PTR
    parent(&self, &arena)->Option< PTR >
    from_lineage(Vec< PTR >) -> Self
    position(&self) -> usize
}

and we need to be able to iterate through the PTRs in a Node (so we can update parent pointers in this nodes 
children)

trait NodeIter {
    current(&self) -> PTR
    current_mut(&mut self) -> &mut PTR
    forward(&mut self, &arena)
    backward(&mut self, &arena)
    new(Node) -> Self
    position() -> usize //more generally an IDX type such that Node.get(i : Idx)-> &ptr
}

The walker needs to be able to go next() using a pointer it saves when it first visits the node. 

# Usage 

Insert has 2 signatures, one where we take a walker. 
Lets call them insert_walkable and insert_revptr. 
Lets go through 

```rust 
insert_walkable< Ordering , etc. >(&mut self : Arena, walker : Walker< ordering, etc.>, value : T) -> PTR {
    let hint = walker.current();
    //try_insert(hint, value) blah blah get the plan if its not free
    let (phys_range, remap_fn, insert_at) = plan.remapping;
    
    walker.seek_to_node(arena[phys_range.begin]);
    for phys in phys_range {
        let block = arena[phys.block_id]
        if block[phys.block_idx].is_none() {continue;}
        let virt = (phys.block_id, block.phys_to_virt(phys.block_idx));
        let new_addr = remap_fn(virt);
        //update children that point to parent
        let current = arena[walker.current];
        let child_iter = NodeIter::new(current);
        for child in child_iter {
            child.set_child_parent(new_addr)
        }
        //update parent's child ptr
        let parent = walker.parent()
        parent.set_parent_child(walker.position, new_addr)
        //update prev/next
        let prev = arena[walker.peek_prev()]
        let next = arena[walker.peek_next()]
        prev.set_next(new_addr)
        next.set_prev(new_addr)
        // update inbound
        current.set_inbound(&mut arena, virt,new_addr)
        walker.advance(&arena);
    }
    //apply remediation

    arena[insert_at]=value
    return 
}
```
Say we inserted a node in the arena and we have to shift the 8 things to its right right 1. 

We use that walker, get current() for the insert position's Node. 
we find position, identify the remediation that needs to happen, then update ptrs, then execute the remediation.

//updating ptrs : 
let next = arena_iter.next();
//update parent_child
let node = arena[arena_iter.current()]
let pos = node_iter.position()
if let Some(parent) = walker.parent {
    arena[parent].set_parent_child(pos,f)
}
//update child_parent
let node = arena[arena_iter.current()]
node.set_child_parent(f)
//update inbound
let node = arena[arena_iter.current()]

//update next_prev
//update prev_next

