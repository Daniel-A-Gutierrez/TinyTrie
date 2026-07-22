use crate::index;
use std::{cmp::Ordering::{Equal, Greater, Less}, collections::VecDeque, ops::Range};

pub trait Ordering{}
pub trait Strategy{}
pub struct Block<T,O,S> where T: Sized, O : Ordering, S : Strategy, {
    ordering : O,
    strategy : S,
    store : VecDeque<Option<T>>,
    offset : u32,
    shift : u32,
    stride : u32,
    rotate : u8,
    len : u16,
}

pub struct BFO;
pub struct InOrder;
pub struct PreOrder;
pub struct PostOrder;
pub struct Insert;

///fast for iterating over leaves, splitting is difficult. 
impl Ordering for BFO {}
///like preorder but reversed
impl Ordering for PostOrder {}
///easiest to split, iteration OK
impl Ordering for InOrder {}
///lookup only goes forward, next element is child or sibling, can be fast for chains. 
impl Ordering for PreOrder {}
///only exposes append, guaranteeing elements stay in insert-order.
impl Ordering for Insert {}

///user maintains ordering and handles ptr updating.
pub struct Manual {}
impl Ordering for Manual {}
///max cap is bounded by P::Half::MAX. cannot exhaust address space. 
pub struct Pluripotent{}
impl Strategy for Pluripotent {}

enum ScanResult { Left(usize) , Right(usize), NotFound } 
impl ScanResult {
    fn to_position(self) -> Option<usize> {
        match self {
            Self::Left(pos) => Some(pos),
            Self::Right(pos) => Some(pos),
            Self::NotFound => None
        }
    }
}

///caller guarantees ranges are in bounds
///if slices point to the same address, boundary is 0 , otherwise its left.len()
///find the index of the nearest None value in 2 slices. if slices arent the same assumes [left]||[right]
#[inline]
fn scan_outward<T : Sized>( left : &[Option<T>], right : &[Option<T>], down : Range<usize>, up : Range<usize> ) 
    -> ScanResult {
    let mut lefti = down.end; 
    let mut righti = up.start;
    let rlen = up.end - (up.start);
    let llen = down.end - (down.start);
    let mlen=rlen.min(llen);
    for i in 0..mlen {
        let l = &left[lefti];
        let r = &right[righti];
        if l.is_none() || r.is_none() {
            if l.is_none() { return ScanResult::Left(lefti) } 
            else { return ScanResult::Right(righti) }
        }
        lefti -=1 ;
        righti +=1 ; 
    }
    for _ in mlen..llen  {
        if let None = left[lefti] { return ScanResult::Left(lefti); }
        lefti-=1;
    }
    for _ in mlen..rlen {
        if let None = right[righti] { return ScanResult::Right(righti); }
        righti+=1;
    }
    return ScanResult::NotFound;
}



enum SearchResult {
    Append,
    Prepend,
    Found(usize),
    NotFound,
}

impl<T,O,S> Block<T,O,S> where T : Sized , S : Strategy, O : Ordering {
    ///panics when pos is out of bounds
    pub fn nearest_empty(&self, pos : usize, budget : usize) -> SearchResult{
        let (front,back) = self.store.as_slices();
        let max = (u32::MAX as usize).min(self.store.len()).min(pos+budget);
        let min = pos.saturating_sub(budget);
        //keypoints - min , boundary, pos,pos+1, max . boundary can lie at any relative position. 
        
        let found = match pos.cmp(&front.len()) {
            Less => {
                let fmax = max.min(front.len());
                scan_outward(front,front,min..pos,pos..fmax)
                    .to_position()
                    .or_else(|| {
                        back[0..max-front.len()]
                        .into_iter()
                        .position(|i| i.is_none())
                        .map(|x|x+front.len())
                    })
                    .map(|f| SearchResult::Found(f))
            },
            Equal => {
                match scan_outward(front,back,min..front.len(),0..max-front.len()) {
                    ScanResult::Left(p) =>  return SearchResult::Found(p),
                    ScanResult::Right(p) => return SearchResult::Found(p+front.len()),
                    ScanResult::NotFound => None
                }
            
            },
            Greater => {
                let fmin = min.saturating_sub(front.len());
                let fmax = max - front.len();
                let fpos = pos - front.len();
                scan_outward(back,back,fmin..fpos,fpos..fmax)
                    .to_position().map(|x| SearchResult::Found(x-front.len()))
                    .or_else(|| {
                        front[min.min(front.len())..front.len()]
                            .into_iter()
                            .rev()
                            .position( |o| o.is_none())
                            .map(|p| SearchResult::Found(front.len()-p-1))
                })
            }
        };
        if found.is_some() { return found.unwrap() }
        if max - pos >= pos - min {
            if min==0 {return SearchResult::Prepend}
            if max==self.store.len() {return SearchResult::Append}
        }
        else {
            if max==self.store.len() {return SearchResult::Append}
            if min==0 {return SearchResult::Prepend}
        }
        return SearchResult::NotFound;
    }
}

impl<T> Block<T, Insert, Pluripotent> where T: Sized {  
    fn append(&mut self, val : T) -> u32 {
        let len = self.store.len() as u32;
        self.store.push_back(Some(val));
        return len;
    }
}

enum Direction { Left, Right } 
struct InsertDelta {
    direction : Direction,
    amount : usize,
    increment : u32 
}

enum InsertSuccess { 
    Free(usize),
    Moved(InsertDelta)
}
enum InsertFailure {
    MaxCapacity,
    AddressExhaustion,
    OutOfBudget
}

struct BlockIter<'a, T: Sized, O : Ordering, S : Strategy,> {
    block : &'a Block<T,O,S>,
    phys : usize,
}

impl<'a, T:Sized,O:Ordering,S:Strategy> BlockIter<'a, T,O,S> {
    fn forward(&mut self) {todo!()}
    fn current(&self) {todo!()}
    fn back(&mut self) {todo!()}
}

impl<T> Block<T, Manual, Pluripotent> where T: Sized{
    pub fn insert_between(&mut self, prev : Option<u32> , val : T, next : Option<u32>) -> Result<InsertSuccess,InsertFailure> {
        if self.store.len() == 0 { self.store.push_back(Some(val)); return Ok(InsertSuccess::Free(0)); }
        //get search result, if see how much we have to move, do the move
        todo!();
    }
    pub fn try_insert_between(&mut self, prev : Option<u32> , val : T, next : Option<u32>) -> Result<InsertSuccess,InsertFailure> {
        if self.store.len() == 0 { self.store.push_back(Some(val)); return Ok(InsertSuccess::Free(0));}
        //get search result, if see how much we have to move, do the move
        todo!();
    }
    pub fn append(&mut self, val : T) -> u32 {todo!()}
    pub fn prepend(&mut self, val : T) {}
    pub fn new() -> Self{todo!()}
    pub fn get(&self, ptr : u32) -> &T {todo!()}
    pub fn get_mut(&self, ptr: u32) -> &mut T {todo!()}
    pub fn cursor(&self) -> BlockIter<T,Manual,Pluripotent> {todo!()}
    pub fn remove(&mut self, ptr : u32) -> T {todo!()}
    pub fn split_off(&mut self, from : usize) -> Self {todo!()}
}
