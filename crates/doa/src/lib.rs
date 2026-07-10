use std::{ collections::VecDeque, marker::PhantomData, num::NonZeroUsize};
mod index;
use index::*;
use itertools::interleave;
pub struct BlockBuilder<PTR: BlockIndex = u16>
{
    capacity : PTR,
    prev: Option<NonZeroUsize>,
    next: Option<NonZeroUsize>,
    addr_shift: u8,
    len: PTR,
    _phantom: PhantomData<PTR>,
}

/*
initialize block
create different 'profiles' to initialize it with
idk if block builder is necessary, might be best to just have 4 functions. 
pluripotent : addr_shift is half of PTR_width-log2(cap) , v_offset is PTR::Max()/2 , none_stride is 4. 
random : addr_shift is PTR::width()-log2(cap), v_offset is PTR::MAX()/2, none_stride is 2. 
append : addr_shift is 0, v_offset is PTR::half_max(), none_stride is 16. 
prepend : addr_shift is 0, v_offset is PTR::MAX() - PTR::half_max(), none_stride is 16. 

when does a block split? 
we give the consumer options
on address exhaustion
on block capacity exhaustion
instead of or in addition to block readdress in the case of a strategy miss. 

how do we detect address exhaustion?
    before we push back, we check if the logical address of buf.len > PTR::max() as usize. 
how do we detect capacity exhaustion?
    simple. buf.len() > MAX_CAPACITY
how do we detect a strategy miss? 
    prepend -> append : we run out of address space for appending.
    prepend -> random : we cant find an open space in the middle after some budget
    append -> random : same as previous
    append -> prepend : we run out of address space for prepending
    random -> sequential : 
        - append : i think we need to track the number of appends, and trigger this if its > len/2
        - prepend : prepends are already tracked by v_offset so we dont need to store anything else. 
        i suppose append would leave a particular signature in the stored data - high len , low occupancy. 
        i could also trigger both of these by 'failing to find space in budget' when the requested space is at an end, 
            but that leaves room for coincidences.
        maybe 'failed to find space in budget' at the end when occupancy is low? Pretty strong coincidence.
            we spread on realloc by the change in addr_shift regardless, but when that change is 0 we exit early
            since it cant go below 0 prepend and append never do the work, but random and pluripotent are forced to. 
            its the intent of them
            and that leaves a signature when the only occupied slots are at the ends.
        'cant find space but occupancy is low' is a good indicator actually for any sequential work in a random optimized block, not just
        sequential work that hits the ends. 
        a middle split isnt cheap but maybe tripartite splits arent unreasonable.
            what if the dense region isnt from appends , its just tightly clustered random data?   
            itll be created pluripotent, run out of space then choose random, how about that? 
        
    actually in general, what if i just split dense regions out into pluripotent blocks? 
    what if i make halfptr::max the 'budget' , have it search in both directions, and when it cant find space, that region gets separated out as a new block? 
    when its at one end theres 2 blocks, when its in the middle theres 3. 
    if it starts at sqrt elements though it doesnt have any time to 'learn'. so i have to do half that, and on its next grow it 'decides'. 
    adult blocks then don't do any sort of complicated logic to figure out how to split/readdress, they just split out a pluripotent block when they run into a dense region
    the pluripotent block decides once when its at a specific size. 

    so when we are growing we branch on our current size. 
    If we're small, we gauge which strategy would be most appropriate based on the contents of the block, alter our parameters and readdress
    when we're large, we dont change. We just split off pluripotent blocks when we hit a continuous region on insert and our occupancy isnt high enough to warrant a realloc or realloc + spread. 

    so how do we alter the pluripotent blocks paramters? 
    Its built to be able to accept max.sqrt() sequential ptrs with its stride. 
    I think we need a function that measures a pluripotent block and recommends a strategy. 

    Also, when a block is at full size, it should split off a pluripotent on dense region, it should do a full split. 

    fn insert
        if let Ok(slot, moved) = find_spot(hint) {
            return (slot, moved) // not this simple, may need a move, this is a match not an if else. 
        }
        else let NotOK(slot) { //not ok -> ran out of budget or ran out of address space
            match NotOk {
                OutOfAddresses
                OutOfBudget {
                    if strategy=pluripotent {
                        if cap < halfptr::max () {
                            just grow and spread. 
                        }
                        else {
                            params = self.decide_strategy()
                        }
                    }

                }   
            }
            if occupancy > threshold*cap {
                if size==max { 
                    if is_minimum(slot) && strategy == prepend => {split off new empty block, no realloc, with 1/2 max capacity.} }
                    else if is_maximum(slot) && strategy==append => {mirror of above} } 
                    else if strategy = random => split in 2, 
                else {realloc, grow, spread} //we're dense, and we're trusting the pluripotent did the right thing. 
            else if(large) { 
                make pluripotent block from dense region, 
                readdress contents of new block 
            }
            else {
                alter parameters, restrategize, readdress. 
            }
        }

    Ok that got out of hand fast, we need the enum's help to split apart the decision matrix.

    trait GrowthStrategy {
        fn HandleInsertion( block : &mut Block, status : InsertionStatus ) -> HandledInsertionEnum //may contain new blocks, feedback for consumer like 'readdress this' 
    }
    struct PluripotentStragy{}
    struct AppendStrategy{}
    struct PrependStrategy{}
    struct RandomStrategy{}

    Enum BlockStrategy{
        Pluripotent(PluripotentStrategy)
        Append(AppendStrategy)
        Prepend(PrependStrategy)
        Random(RandomStrategy)
    }

    insert(&mut self, hint, value) -> Result<HandledInsertion,InsertionError> {
        match find_spot(hint) { 
            Ok(f) => { 
                match f {
                    Append => {
                        if (self.buf.len() + v_offset) & none_stride == 1 { self.buf.push_back( None ); }
                        let idx = HandledInsertion::Free(detranslate(self.buf.len()))
                        self.buf.push_back(value); 
                        return x
                    }
                    Prepend {
                        if (v_offset) & none_stride == 1 { self.buf.push_back( None ); }
                        let idx = HandleInsertion::Free(detranslate(0));
                        self.buf.push_front(value);
                        self.virtual_offset += 1;
                        return idx;
                    }
                    FreeRealestate(phys) {
                        let idx = HandledInsertion::Free(detranslate(idx))
                        self.buf[phys]=value;
                        return idx; 
                    },
                    Slot(phys) {
                        let direction = (hint >= phys) as usize;
                        let amount = phys.abs_diff(hint) - direction;
                        let min = phys.min(hint)
                        let max = phys.max(hint)
                        self.buf.copy_within(min+direction..max, min+1-direction)
                        self.buf[hint-direction]=value
                        let minus = -((direction<<1) as isize)+1;
                        return HandledInsertion::Move(amount, minus, minus<<addr_shift)
                    }
                }
            Err(e) {
                let x = match e {
                    OutOfBounds => return Err(InsertError::OutOfBounds),
                    AddressOverflow || AdressUnderflow || SlotNotFound => Strategy.HandleInsertion(&mut self, e, hint, value);
                }
                return Ok(x);
            }
        }
    }

    fn find_spot(&self virt) -> Result<FoundSlot,FindSlotErr> {
        //scan closest to furthest by stride. returns a physical address or append/prepend in the success case. 
        //let phys = block.translate(virt) //returns a usize
        //ACTUALLY i think translate needs to handle the case where we've exhausted our address space. 
        //maybe we assume elsewhere that translate is working on a valid existing address,
        //and try_translate does the check for insert , where the hint may not be an existing address? 
        let translated = block.try_translate(virt); 
        let phys = match translated {
            Ok(phys) => phys,
            OutOfBounds => {return FindSlotErr::OutOfBounds(slot)}
            AddressOverflow => {return FindSlotErr::AddressOverflow}
            AddressUnderflow => return FindSlotErr::AddressUnderflow
        };

        if phys==0 return FoundSlot::Prepend
        if phys==block.buf.len() return FoundSlot::Append
        if block.buf[phys].is_none() return FoundSlot::FreeRealEstate(phys);
        if block.buf[phys-1].is_none() return FoundSlot::FreeRealEstate(phys-1);
        aligned = align_to_none_stride(i)
        
        //ok this part is trickier than expected, virtual offset matters for the none stride alignment
        //its definitely legal to translate 0..len to virtual addresses, do i have to align them there then translate those back? 
        //detranslation is (i + offset) <<shift, right? i think i just skip the shift.
        //if i guarantee none stride is a power of 2, then its just a bitwise & check . also 0 + thing is identity so its just virtual_offset & none_stride = if_maybe_none.
        //what i want is the first aligned thing GTEQ phys 0. 
        //maybe we store none_stride as a mask, so we can just mask of the lesser N bits to get the mod quickly. 
        //also it has to be when virt&nonestride == 1, not 0, because spread maps Some(things) onto the even addresses, its the odd ones that are None. 

        let min = aligned.saturating_sub(budget*none_stride)
        let max = aligned.saturating_add(budget*none_stride);

        let left_iter = (min..aligned).into_iter().step_by(none_stride).rev()
        let right_iter = (aligned..max).into_iter().step_by(none_stride)
        let outward = left_iter.zip(right_iter).flatten()
        match outward.find(|(i)| block.buf[i].is_none()) {
            Some(i) => i,
            None => outward.tail.find(|i| block.buf[i].is_none)
        }
        match idx {
            Some(i) => return FoundSlot::Slot(i)
        }
    }

    
*/

fn x() {
    let y = VecDeque::new();
    y.iter().zip(y.iter())
    let x = [1,2,3];
    let a = 55usize;
    let b = 67usize;
    let d = a.abs_diff(b);
    
}

impl<T, PTR: BlockIndex> BlockBuilder<PTR>
where
    T: Sized,
{
    /// Empty builder with defaults: no arrays, no links, `len = 0`, and
    /// `addr_shift = PTR::width()` (a fresh block addresses its single slot
    /// over the full pointer range, per the design notes).
    pub fn new() -> Self {
        Self {
            array: Vec::new(),
            rev_array: Vec::new(),
            prev: None,
            next: None,
            addr_shift: PTR::width(),
            len: PTR::from_usize(0),
            _phantom: PhantomData,
        }
    }

    /// Forward storage: the dense array of present elements.
    pub fn array(mut self, array: Vec<Option<T>>) -> Self {
        self.array = array;
        self
    }

    /// Reverse storage (mirror of [`array`](Self::array) for backward scans).
    pub fn rev_array(mut self, rev_array: Vec<Option<T>>) -> Self {
        self.rev_array = rev_array;
        self
    }

    /// Link to the predecessor block (arena-local index), or `None` if head.
    pub fn prev(mut self, prev: Option<NonZeroUsize>) -> Self {
        self.prev = prev;
        self
    }

    /// Link to the successor block (arena-local index), or `None` if tail.
    pub fn next(mut self, next: Option<NonZeroUsize>) -> Self {
        self.next = next;
        self
    }

    /// Logical→physical address shift (`phys = logical >> addr_shift`).
    pub fn addr_shift(mut self, addr_shift: u8) -> Self {
        self.addr_shift = addr_shift;
        self
    }

    /// Logical length (number of live elements).
    pub fn len(mut self, len: PTR) -> Self {
        self.len = len;
        self
    }

    /// Consume the builder and materialize the [`Block`].
    ///
    /// The `none_stride` / `virt_offset` / `first` / `last` fields are not yet
    /// wired through the builder (Block is a WIP skeleton); they default to
    /// zero so the crate compiles. `addr_shift` widens to the struct's `u32`.
    pub fn build(self) -> Block<T, PTR, MAX> {
        let _ = &self.len; // builder's `len` is not yet a Block field
        Block {
            buf : VecDeque::new(),
            prev: self.prev,
            next: self.next,
            addr_shift: self.addr_shift as u32,
            none_stride: PTR::ZERO,
            virt_offset: PTR::ZERO,
            first: PTR::ZERO,
            last: PTR::ZERO,
        }
    }
}

impl<T, PTR: BlockIndex, const MAX: usize> Default for BlockBuilder<T, PTR, MAX>
where
    T: Sized,
{
    fn default() -> Self {
        Self::new()
    }
}

pub struct BlockIter<T, PTR: BlockIndex, const MAX: usize>
where
    T: Sized,
{
    _phantom: PhantomData<(T, PTR)>,
}

pub struct Block<T, PTR: BlockIndex = u16, const MAX: usize = 65535>
where
    T: Sized,
{
    buf : VecDeque<Option<T>>,
    prev: Option<usize>,
    next: Option<usize>,
    addr_shift: u32,
    none_stride : PTR,
    virt_offset : PTR,
}

impl<T, PTR: BlockIndex, const MAX: usize> Block<T,PTR,MAX>
    where T: Sized 
{
    /*
    push
    insert(hint)
    realloc
    spread
    iter()
    get(virtual) -> T at lookup(virtual)
    lookup(virtual) -> physical ptr (still signed, -1..-cap/2 go in rev array)
    translate(physical) -> virtual ptr
    */
}