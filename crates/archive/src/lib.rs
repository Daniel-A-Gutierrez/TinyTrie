#![allow(incomplete_features)]
#![feature(generic_const_exprs)]
#![feature(portable_simd)]
#![allow(internal_features)]
#![feature(nonzero_internals)]

//! Archived/legacy trie implementations — not published.
//!
//! This crate contains earlier versions of the data structures for reference.
//! It is not intended for production use.

mod archive;
pub use archive::prefix_trie::null_terminate;
