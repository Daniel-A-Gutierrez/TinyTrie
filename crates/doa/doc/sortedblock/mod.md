```rust
//!sorted block: a dense-ordered-arena SET — height-1 consumer of the shared
//!substrate (`index`/`pos`/`translator`) + its own store copy. target surface
//!(devlog):
//!
//! ``` ignore
//! insert(key, &mut pairs) -> addr
//! remove(key) -> Option<key>
//! iter() -> impl Iterator<Item=&key>
//! get(addr) -> &key
//! ```
//!
//!invariants: pos order == key order (pos 0 = min); addrs are stable names
//!across grows (translator absorbs) — slides/gathers rename them, reported as
//!pairs ordered so a holder can apply `swap(from, to)` in sequence (the
//!hole-adjacent pair first, then the cascade). `store` records `(Pos, Pos)`
//!pairs at the slot level; the block translates them to addr pairs.
//!translator logic (devlog lines 99–245: full-width shift-down, edge-offset,
//!rotation-turn minting, reversed-flag bias) is the open design — the block
//!itself is unwritten.
///L0021
pub mod store;
///L0022
pub mod sortedblock;
```
