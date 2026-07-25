use crate::index;
use std::{cmp::Ordering::{Equal, Greater, Less},
          collections::VecDeque,
          ops::Range};
pub trait Ordering {}
///only exposes append, guaranteeing elements stay in insert-order.
pub struct Insert;
///user maintains ordering and handles ptr updating.
pub struct Manual {}
impl Ordering for Insert {}
impl Ordering for Manual {}
///max cap is bounded by P::Half::MAX. cannot exhaust address space.
pub trait Strategy {}
pub struct Block<T, O, S>
where
    T: Sized,
    O: Ordering,
    S: Strategy,
{
    ordering: O,
    strategy: S,
    store:    VecDeque<Option<T>>,
    translator : A,
    len:      u16,
}
pub struct Pluripotent {}
impl Strategy for Pluripotent {}
enum SearchResult {
    Append,
    Prepend,
    Found(usize),
    NotFound,
}
impl<T> Block<T, Insert, Pluripotent>
where
    T: Sized,
{
    fn append(&mut self, val: T) -> u32 {
        let len = self.store.len() as u32;
        self.store.push_back(Some(val));
        return len;
    }
}
enum Direction {
    Left,
    Right,
}
struct InsertDelta {
    direction: Direction,
    amount:    usize,
    increment: u32,
}
enum InsertSuccess {
    Free(usize),
    Moved(InsertDelta),
}
enum InsertFailure {
    MaxCapacity,
    AddressExhaustion,
    OutOfBudget,
}
struct BlockIter<'a, T: Sized, O: Ordering, S: Strategy> {
    block: &'a Block<T, O, S>,
    phys:  usize,
}
impl<'a, T: Sized, O: Ordering, S: Strategy> BlockIter<'a, T, O, S> {
    fn forward(&mut self) {
        todo!()
    }
    fn current(&self) {
        todo!()
    }
    fn back(&mut self) {
        todo!()
    }
}
impl<T> Block<T, Manual, Pluripotent>
where
    T: Sized,
{
    pub fn insert_between(
        &mut self,
        prev: Option<u32>,
        val: T,
        next: Option<u32>,
    ) -> Result<InsertSuccess, InsertFailure> {
        if self.store.len() == 0 {
            self.store.push_back(Some(val));
            return Ok(InsertSuccess::Free(0));
        }
        //get search result, if see how much we have to move, do the move
        todo!();
    }
    pub fn try_insert_between(
        &mut self,
        prev: Option<u32>,
        val: T,
        next: Option<u32>,
    ) -> Result<InsertSuccess, InsertFailure> {
        if self.store.len() == 0 {
            self.store.push_back(Some(val));
            return Ok(InsertSuccess::Free(0));
        }
        //get search result, if see how much we have to move, do the move
        todo!();
    }
    pub fn append(&mut self, val: T) -> u32 {
        todo!()
    }
    pub fn prepend(&mut self, val: T) {}
    pub fn new() -> Self {
        todo!()
    }
    pub fn get(&self, ptr: u32) -> &T {
        todo!()
    }
    pub fn get_mut(&self, ptr: u32) -> &mut T {
        todo!()
    }
    pub fn cursor(&self) -> BlockIter<T, Manual, Pluripotent> {
        todo!()
    }
    pub fn remove(&mut self, ptr: u32) -> T {
        todo!()
    }
    pub fn split_off(&mut self, from: usize) -> Self {
        todo!()
    }
}
