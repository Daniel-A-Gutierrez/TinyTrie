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
//!invariants: pos order == key order — normal (pos 0 = min) or reversed
//!(pos 0 = max, Prepend bias); addrs are stable names across grows
//!(translator absorbs) — slides/gathers rename them, reported as pairs
//!ordered so a holder can apply `swap(from, to)` in sequence (the
//!hole-adjacent pair first, then the cascade). `store` records `(Pos, Pos)`
//!pairs at the slot level; the block translates them to addr pairs.

pub mod sortedblock;
pub mod store;
