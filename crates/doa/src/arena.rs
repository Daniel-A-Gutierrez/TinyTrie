use std::collections::VecDeque;
use crate::{block::Block, index::*};
struct Arena<T,U,I> {
    blocks : VecDeque<Block<T,I,I::Unsigned::Max()>>,
    requests : VecDeque<(U,I)>
}

impl <U,I,T> Arena<U,I,T> 
    where : //blah blah 
{
    fn insert_before( (U,I), T ) {}
    fn insert_after() // rest of stuff from notes
}

