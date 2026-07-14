# Contents
This workspace is home to a number of datastructure experiments, with the goal of developing fast, concurrent, serializing map structures suitable for use in a database as indexes.

Notably in crates theres
- arrays : not depended on currently, but stores primitives used by other crates
- benches : a crate that benchmarks the various datastructures that live in the workspace
- btrees : A custom b+ tree with performance generally beating std::collections::BTreeMap
- doa : Dense ordered arena, for storing data in a order-preserving sequential space.
- poly-trie : Like tiny-trie but with a dynamic node size
- tiny-trie : A family of Definite Finite Automata based Radix Tries with fixed node sizes.
- archive : old stuff I didn't want to delete.

# Behavior
- Consider that communications with the user are the primary bottleneck for development. 
- The second is thinking and planning.
- Keep responses as short as possible, keep comments in code to the absolute minimum length - panics, invariants, caller guarantees, important things that cant be gleaned from a glance at the function signature. No need for complete sentences. Doc comments written for library consumers are a different story.
- Don't go off thinking on your own for minutes at a time unless instructed to, if there's confusion or you suspect a problem with the prompt ask questions or give feedback quickly. 
- Code quality and performance are of the utmost importance.
- Its not necessary for things to compile at each iteration - use the compiler errors to guide you, its faster and can tell you where problems lie after a change faster than you can pre-emptively look for all the call sites/uses.
- When starting work in a crate within this workspace, based on the crates claude.md and the user's promps preemptively read the entirety of the relevant source files, don't bother grepping around for call sites of individual items. 