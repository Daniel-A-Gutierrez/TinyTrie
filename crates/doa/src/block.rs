use std::{ collections::VecDeque, marker::PhantomData, num::NonZeroUsize};
use crate::index::*;

pub struct Block<T, PTR: BlockIndex = i16, const MAX: usize = 65535>
where
    T: Sized,
{
    buf : VecDeque<Option<T>>,
    prev: Option<usize>,
    next: Option<usize>,
    addr_shift: u32,
    none_mask : u32,
    virt_offset : isize,
    _phantom : PhantomData<PTR>
}

// /*
// initialize block
// create different 'profiles' to initialize it with
// idk if block builder is necessary, might be best to just have 4 functions. 
// pluripotent : addr_shift is half of PTR_width-log2(cap) , v_offset is PTR::Max()/2 , none_stride is 4. 
// random : addr_shift is PTR::width()-log2(cap), v_offset is PTR::MAX()/2, none_stride is 2. 
// append : addr_shift is 0, v_offset is PTR::half_max(), none_stride is 16. 
// prepend : addr_shift is 0, v_offset is PTR::MAX() - PTR::half_max(), none_stride is 16. 

//     trait GrowthStrategy {
//         fn HandleInsertion( block : &mut Block, status : InsertionStatus ) -> HandledInsertionEnum //may contain new blocks, feedback for consumer like 'readdress this' 
//     }
//     struct PluripotentStragy{}
//     struct AppendStrategy{}
//     struct PrependStrategy{}
//     struct RandomStrategy{}

//     Enum BlockStrategy{
//         Pluripotent(PluripotentStrategy)
//         Append(AppendStrategy)
//         Prepend(PrependStrategy)
//         Random(RandomStrategy)
//     }

//     insert(&mut self, hint, value) -> Result<HandledInsertion,InsertionError> {
//         match find_spot(hint) { 
//             Ok(f) => { 
//                 match f {
//                     Append => {
//                         if (self.buf.len() + v_offset) & none_stride == 1 { self.buf.push_back( None ); }
//                         let idx = HandledInsertion::Free(detranslate(self.buf.len()))
//                         self.buf.push_back(value); 
//                         return x
//                     }
//                     Prepend {
//                         if (v_offset) & none_stride == 1 { self.buf.push_back( None ); }
//                         let idx = HandleInsertion::Free(detranslate(0));
//                         self.buf.push_front(value);
//                         self.virtual_offset += 1;
//                         return idx;
//                     }
//                     FreeRealestate(phys) {
//                         let idx = HandledInsertion::Free(detranslate(idx))
//                         self.buf[phys]=value;
//                         return idx; 
//                     },
//                     Slot(phys) {
//                         let direction = (hint >= phys) as usize;
//                         let amount = phys.abs_diff(hint) - direction;
//                         let min = phys.min(hint)
//                         let max = phys.max(hint)
//                         self.buf.copy_within(min+direction..max, min+1-direction)
//                         self.buf[hint-direction]=value
//                         let minus = -((direction<<1) as isize)+1;
//                         return HandledInsertion::Move(amount, minus, minus<<addr_shift)
//                     }
//                 }
//             Err(e) {
//                 let x = match e {
//                     OutOfBounds => return Err(InsertError::OutOfBounds),
//                     AddressOverflow || AdressUnderflow || SlotNotFound => Strategy.HandleInsertion(&mut self, e, hint, value);
//                 }
//                 return Ok(x);
//             }
//         }
//     }

//     see find_slot.rs for implemented version
//     fn find_spot(&self virt) -> Result<FoundSlot,FindSlotErr> {
//         //scan closest to furthest by stride. returns a physical address or append/prepend in the success case. 
//         //let phys = block.translate(virt) //returns a usize
//         //ACTUALLY i think translate needs to handle the case where we've exhausted our address space. 
//         //maybe we assume elsewhere that translate is working on a valid existing address,
//         //and try_translate does the check for insert , where the hint may not be an existing address? 
//     }

    
// */



// impl<T, PTR: BlockIndex, const MAX: usize> Block<T,PTR,MAX>
//     where T: Sized 
// {
//     /*
//     push
//     insert(hint)
//     realloc
//     spread
//     iter()
//     get(virtual) -> T at lookup(virtual)
//     lookup(virtual) -> physical ptr (still signed, -1..-cap/2 go in rev array)
//     translate(physical) -> virtual ptr
//     */
// }