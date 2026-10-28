# Contents
This workspace is home to a number of datastructure experiments, with the goal of developing fast, concurrent, serializing map structures suitable for use in a database as indexes.

Notably in crates theres
- arrays : not depended on currently, but stores primitives used by other crates
- benches : a crate that benchmarks the various datastructures that live in the workspace. bench-suite doc in crates/benches/notes.
- block : the serializable fixed-storage substrate — dual-front allocator over one relptr'd allocation, Archive v2 + RelOrd handles, Stack, threaded Stacks over kanal. Concepts, invariants and sharp edges: crates/block/CLAUDE.md; design log + subtle bugs in crates/block/notes.
- block-derive : proc-macro derives generating block's Archive/RelOrd handle structs + layout canaries. See crates/block-derive/CLAUDE.md.
- btrees : A custom b+ tree with performance generally beating std::collections::BTreeMap. plans/optimization history + test sweeps in crates/btrees/notes.
- doa : Dense ordered arena, for storing data in a order-preserving sequential space. Concepts, tiers, subtle bugs: crates/doa/CLAUDE.md; design log + subtle bugs in crates/doa/notes.
- poly-trie : Like tiny-trie but with a dynamic node size. history in crates/poly-trie/notes.
- tiny-trie : A family of Definite Finite Automata based Radix Tries with fixed node sizes. history + test outputs in crates/tiny-trie/notes.
- archive : old stuff I didn't want to delete. old experiment notes (arena/prefix tries) in crates/archive/notes.

# Notes layout
- each crate keeps its own notes/ (devlog = the user's design writing — never authored by sessions; subtle_bugs where the crate has earned them; historical experiment notes per the categories above).
- workspace notes/ holds the cross-crate material: session_history.md (per-session record, newest last, format contract at the top), plus the multi-crate historicals (design.md, _summary.md, _traits.md, todo.md) and the general ones (serialization.md, lifetimes.md, workflow.md, claude_context.md).

# Behavior
- Code quality and performance are of the utmost importance.
- Keep responses as short as possible, keep comments in code to the absolute minimum length - panics, invariants, caller guarantees, important things that cant be gleaned from a glance at the function signature. No need for complete sentences. Doc comments written for library consumers are a different story.
- Brevity calibration for prose: lead with the verdict (agree/disagree/sound/defer), one short clause of why, then the caveat or condition. No preamble, no restating the question, no "here's my take". For multi-point responses: one labeled point each, 1-2 sentences, opinionated but tight. Drop hedging and filler. Long is fine when the content earns it (a real trade-off, a design fork); short when it's a single fact or yes/no. The user will ask for more if they want it.
- Don't go off thinking on your own for minutes at a time unless instructed to, if there's confusion or you suspect a problem with the prompt ask questions or give feedback quickly. 
- Its not necessary for things to compile at each iteration - use the compiler errors to guide you, its faster and can tell you where problems lie after a change faster than you can pre-emptively look for all the call sites/uses.
- When starting work in a crate within this workspace, based on the crates claude.md and the user's promps preemptively read the entirety of the relevant source files, don't bother grepping around for call sites of individual items. 
- KEEP COMMENTS AS CONCISE AS POSSIBLE. Don't summarize anything thats obvious from reading the code or the function signature. Don't expound on things you're not sure are correct.

# Style
- doc comments are the single source of truth for item outlines - keep them
  minimal: 1-2 lines, what the item is FOR plus anything non-obvious about
  its contract (assumptions, guarantees, panics, None/return semantics).
  they never describe HOW the function works - mechanism lives as
  single-line annotations at the site doing it (rationale at the branch
  that needs it, safety notes at the loop that depends on them). variant
  and field docs carry their own semantics. SAFETY blocks and `//!` module
  headers are exempt - they hold the full contract.
- single line comments may be introduced in long functions to concisely explain what a block of code does
- a comment must never be longer than the source it applies to.
- function names and variables should be concise but explanatory - avoid arbitrary letters and abbreviations.
- module item order: `use` → `mod` → structs → types → traits → impls → macro
  invocations. a macro is defined where its output class lives — one generating
  structs sits with the structs, one generating impls with the impls.
- each file's `//!` header carries its purpose + invariants. generated item
  outlines live in the owning crate's doc/ notes (where a crate has them,
  e.g. doa/doc/) and are generated, never hand-edited.

# Workflow
- sessions start from a clean tree: the previous session ends with a commit. remind the user before making code edits if the git diff is dirty.
- when starting work in a crate: read that crate's claude.md (and subtle_bugs.md if it has one) before its sources.
- session-end routine, in order: run the crate's outline regenerator where one exists (doa: `skeletonize.py` → doa/doc/), spawn a **fresh** review agent (not a fork) on the session's diff — it reads the working crate's claude.md and subtle_bugs.md first so it doesn't re-flag intentional choices — apply findings, append the session's entry to `notes/session_history.md` (workspace-level; format contract at the file's top), update the crate's claude.md + subtle_bugs.md (merge in place, prune stale entries), re-run the regenerator if src changed, and leave the tree ready for commit — the user commits.
- review priority: correctness (bugs, invariant breaks, do-not-revive violations) >
  cheap cleanups > doc-record > perf. unbenchmarked perf suggestions are reported,
  never auto-applied.
- "defer" means written into the crate's claude.md Future Work or subtle_bugs.md — never
  "remembered".
- do not update the claude.toml until the session is ending (doa's file). 