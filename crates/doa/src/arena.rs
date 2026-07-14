//! Arena — owns the `Vec` of blocks (linked via `prev`/`next`) and a queue of
//! recent insert hints. Intercepts block-level `InsertDelta`s to perform
//! auto-split/grow; `try_insert` variants bubble errors for manual control.
//! Skeleton: methods stubbed, fleshed out in Phase 4.

use std::collections::VecDeque;

use crate::block::Block;
use crate::index::SignedBlockIndex;

pub struct Arena<T, U, I: SignedBlockIndex> {
    blocks:   VecDeque<Block<T, I>>,
    /// Last ~16 insert hints: `(user_key, address)`.
    requests: VecDeque<(U, I)>,
}

impl<T, U, I: SignedBlockIndex> Arena<T, U, I> {
    pub fn new() -> Self {
        Self { blocks: VecDeque::new(), requests: VecDeque::new() }
    }

    pub fn insert_before(&mut self, _at: (U, I), _val: T) {
        todo!("GP4: block-level insert_before with auto-split/grow + InsertDelta remap")
    }
    pub fn insert_after(&mut self, _at: (U, I), _val: T) {
        todo!("GP4: block-level insert_after with auto-split/grow + InsertDelta remap")
    }
}

impl<T, U, I: SignedBlockIndex> Default for Arena<T, U, I> {
    fn default() -> Self {
        Self::new()
    }
}
