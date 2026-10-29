//!append-ordered storage over `Block`: push archives `T` through the
//!Allocator and stores the unit — (heap_len snapshot u32, Handle) — on the
//!stack front; pop shrinks to the unit's start and `pop_heap`s to its
//!snapshot — both wipes, so a LIFO pop ERASES the item's stack bytes and heap
//!regions completely (no freelist machinery; `T::release` stays available
//!for non-LIFO consumers, Stack never calls it). unordered storage —
//!ordering/search stays the consumer's job (`RelOrd`/`Heap` are reads, not
//!duties). v0 is ONE block, no growth: full ⇒ `CapErr` (multi-block files
//!are Stacks).
//!unit layout = [snap u32][align gap (block-zeroed)][handle], pushed as two
//!items — `Block::push` zeroes the gap and the ZeroPad handles carry no
//!undefined bytes, so no unit-level pad wipe exists. the gap widths are
//!per-T constants, so a unit's stride is too: `layout()` derives
//!(unit0, stride) and every walk is arithmetic.
//!serialized record = [Header 16B | stack | zero gap | heap incl. freelist
//!head]; `Stack::new` picks cap = RECORD − 16 so the record is exactly 4096
//!bytes. the dead gap writes as zeros and the zero-record invariant covers
//!everything else — a record is live data or zero, byte-for-byte round-tripped
//!(no field decode). the snapshot rides the record, so `from_record` needs
//!no extra state.

use std::fmt;
use std::io::{self, Write};
use std::marker::PhantomData;
use std::mem::{align_of, size_of};
use std::path::Path;

use crate::archive::{Archive, Heap};
use crate::block::{Block, CapErr, NODE};
use crate::handle::S;

///default record size — devlog §5b's 4096-byte block. cap = RECORD − 16.
pub const RECORD: usize = 4096;

pub struct Stack<T> {
    block: Block,
    _t:    PhantomData<T>,
}

///record validation failure; `Io` comes only from file loads.
#[derive(Debug)]
pub enum LoadErr {
    Io(io::Error),
    TooShort,
    BadCap { cap: usize },
    BadPtr { ptr: u32 },
    StrideMismatch { file: usize, stride: usize },
    NotSingleRecord { records: usize },
    Overflow { stack: u32, heap: u32, cap: u32 },
}

impl fmt::Display for LoadErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::TooShort => write!(f, "record shorter than a header"),
            Self::BadCap { cap } => write!(
                f,
                "bad cap {cap} (below 16, not 16-multiple, or past the pointer width)"
            ),
            Self::BadPtr { ptr } => write!(f, "bad ptr {ptr} (must be 16)"),
            Self::StrideMismatch { file, stride } => {
                write!(f, "file length {file} is not a multiple of record stride {stride}")
            }
            Self::NotSingleRecord { records } => {
                write!(f, "{records} records — Stack is single-block")
            }
            Self::Overflow { stack, heap, cap } => {
                write!(f, "stack {stack} + heap {heap} exceed cap {cap}")
            }
        }
    }
}

impl std::error::Error for LoadErr {}

impl From<io::Error> for LoadErr {
    fn from(e: io::Error) -> Self {
        LoadErr::Io(e)
    }
}

impl<T: Archive> Stack<T> {
    ///cap chosen so the serialized record is exactly `RECORD` bytes.
    pub fn new() -> Self {
        Self::with_cap(RECORD - 16)
    }

    ///data cap = `cap` rounded up to 16; record stride = 16 + rounded cap.
    pub fn with_cap(cap: usize) -> Self {
        Stack { block: Block::new(cap), _t: PhantomData }
    }

    ///per-T unit geometry: `(unit0, stride, gap0, gap1)`. unit =
    /// [snap u32][gap1][handle]; the next unit's snap lands 4-aligned after
    /// the handle, so units ≥ 1 carry a LEADING gap0 (nonzero only for
    /// sub-4-aligned handles). every gap is a per-T CONSTANT: unit_start ≡ 0
    /// mod align_h inductively (handle ends stay aligned), so snap_end ≡
    /// 4 mod align_h always — the fact `unit_parts_back` leans on.
    fn layout() -> (usize, usize, usize, usize) {
        let (sh, ah) = (size_of::<T::Handle>(), align_of::<T::Handle>().max(1));
        let gap1 = ah.max(4) - 4;
        let gap0 = (4 - sh % 4) % 4;
        (4 + gap1 + sh, 4 + gap1 + sh + gap0, gap0, gap1)
    }

    ///(snap offset, handle offset) of unit `i`, absolute stack-region bytes —
    ///unit 0 has no leading gap0.
    fn unit_geo(i: usize) -> (usize, usize) {
        let (unit0, stride, gap0, gap1) = Self::layout();
        let (snap, handle) = (gap0, gap0 + 4 + gap1);
        if i == 0 {
            (0, 4 + gap1)
        } else {
            let u = unit0 + (i - 1) * stride;
            (u + snap, u + handle)
        }
    }

    ///archives `k`, then pushes the unit (pre-archive heap_len, handle) onto
    ///the stack. on Err both ends are unchanged: a failing `T::archive`
    ///unwinds itself (children-first release + front rollback — the archive
    ///contract), and a stack-side Err unwinds the landed archive the same
    ///way before shrinking the snap back.
    pub fn push(&mut self, k: T) -> Result<S<T::Handle>, CapErr> {
        self.push_borrowed(&k)
    }

    ///push that borrows k — on CapErr the value is intact, so a caller can
    ///retry the item into a fresh block without a Clone bound (Stacks' spill).
    pub fn push_borrowed(&mut self, k: &T) -> Result<S<T::Handle>, CapErr> {
        let heap0 = self.block.heap_len();
        let front0 = self.block.stack_len();
        let h = {
            let mut al = self.block.allocator();
            match T::archive(k, &mut al) {
                Ok(h) => h,
                //the impl unwound itself — the heap is already unchanged
                Err(e) => return Err(e),
            }
        };
        match self.block.push(heap0 as u32) {
            Ok(_) => {}
            Err(e) => {
                //unwind the landed archive, then give the space back
                let mut al = self.block.allocator();
                T::release(&h, &mut al);
                al.rollback(heap0);
                return Err(e);
            }
        }
        match self.block.push(h) {
            Ok(s) => Ok(s),
            Err(e) => {
                //pull the snap back to the PRE-PUSH front — s_snap.off() is
                //the snap's aligned start, so it would strand the snap's
                //alignment gap on the stack front (sub-4-aligned handles)
                self.block.stack_shrink(front0);
                let mut al = self.block.allocator();
                T::release(&h, &mut al);
                al.rollback(heap0);
                Err(e)
            }
        }
    }

    ///pops the LAST unit: shrinks (wipes) its stack bytes and `pop_heap`s to
    ///its snapshot (wiping every heap byte above it — that is the item's own
    ///regions under strict LIFO; any unit pushed after it is already gone, or
    ///its push failed and unwound). returns the handle — its bytes are wiped
    ///and its regions erased, bookkeeping only. no owned-T materialization.
    pub fn pop(&mut self) -> Option<T::Handle> {
        //copy out before mutating — last() borrows the stack bytes
        let (snap, h) = self.last()?;
        let h = *h;
        self.block.stack_shrink(self.unit_start_back());
        self.block.pop_heap(self.block.heap_len() - snap as usize);
        Some(h)
    }

    ///the stored units in push order — (snapshot, handle) pairs. `snap` names
    ///the heap extent the unit's pop will erase to.
    pub fn items(&self) -> impl Iterator<Item = (u32, &T::Handle)> + '_ {
        let n = self.len();
        let bytes = self.block.stack_bytes();
        (0..n).map(move |i| {
            let (s, h) = Self::unit_geo(i);
            let snap = u32::from_le_bytes(bytes[s..s + 4].try_into().unwrap());
            // SAFETY: units push (snap, handle) with the handle at its
            // unit-geo offset, handle-aligned by Block::push
            let h = unsafe { &*bytes.as_ptr().add(h).cast::<T::Handle>() };
            (snap, h)
        })
    }

    ///the last unit's (snapshot, handle), without popping — DIVISION-FREE:
    ///the unit geometry is constant per T, so the last unit's position
    ///derives backward from stack_len (a runtime stride division here is a
    ///~20-cycle stall on every read; see layout()'s proof for why snap_end
    ///sits exactly gap1 before the handle).
    pub fn last(&self) -> Option<(u32, &T::Handle)> {
        let bytes = self.block.stack_len();
        if bytes == 0 {
            return None;
        }
        let (snap_end, handle_start) = self.unit_parts_back(bytes);
        let stack_bytes = self.block.stack_bytes();
        let snap = u32::from_le_bytes(stack_bytes[snap_end - 4..snap_end].try_into().unwrap());
        // SAFETY: the last pushed item is the handle, ending at stack_len;
        // Block::push placed it handle-aligned
        let h = unsafe { &*stack_bytes.as_ptr().add(handle_start).cast::<T::Handle>() };
        Some((snap, h))
    }

    ///backward geometry of the last unit, given `bytes = stack_len ≥ its
    ///unit`: handle = [bytes − size_h, bytes); snap ends gap1 before the
    ///handle starts (gap1 is the per-T constant — snap_end ≡ 0 mod 4 and
    ///snap_end mod align_h is the constant 4 mod align_h). the assert is the
    ///misuse canary: any stack_len that cannot be a unit's end is broken.
    fn unit_parts_back(&self, bytes: usize) -> (usize, usize) {
        let (sh, gap1) = (size_of::<T::Handle>(), Self::layout().3);
        let handle_start = bytes - sh;
        let snap_end = handle_start - gap1;
        assert!(snap_end >= 4, "stack/unit stride broken");
        (snap_end, handle_start)
    }

    ///the last unit's start byte (pop's shrink target) — backward, without
    ///the count: unit0 has no leading gap0, steady units do; one comparison
    ///replaces the runtime-stride division.
    fn unit_start_back(&self) -> usize {
        let bytes = self.block.stack_len();
        let (unit0, stride, gap0, _) = Self::layout();
        debug_assert!(
            bytes == 0 || bytes == unit0 || (bytes - unit0) % stride == 0,
            "stack/unit stride broken"
        );
        let snap_end = self.unit_parts_back(bytes).0;
        if bytes == unit0 { snap_end - 4 } else { snap_end - 4 - gap0 }
    }

    ///item count = stack bytes / unit stride. NOT the hot path — last()/pop()
    ///derive positions backward without the count (the stride is a runtime
    ///divisor; the division is real and stays out of reads).
    pub fn len(&self) -> usize {
        let bytes = self.block.stack_len();
        if bytes == 0 {
            return 0;
        }
        let (unit0, stride, _, _) = Self::layout();
        1 + (bytes - unit0) / stride
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    ///read seam over the block's heap regions.
    pub fn heap(&self) -> Heap<'_> {
        Heap::new(&self.block)
    }

    pub fn block(&self) -> &Block {
        &self.block
    }

    ///the serialized record: [Header | stack | zero gap | heap + freelist
    ///head]. two records of equal stacks are byte-identical (zero-record
    ///invariant + zeroed gap).
    pub fn to_record(&self) -> Vec<u8> {
        let (stack, heap, cap) =
            (self.block.stack_len(), self.block.heap_len(), self.block.cap());
        let mut v = Vec::with_capacity(16 + cap);
        v.extend_from_slice(&(stack as u32).to_le_bytes());
        v.extend_from_slice(&(heap as u32).to_le_bytes());
        v.extend_from_slice(&(cap as u32).to_le_bytes());
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(self.block.stack_bytes());
        v.resize(16 + cap - NODE - heap, 0);
        //whole heap span including the freelist head: the zero-record
        //invariant keeps even alignment pads defined
        v.extend_from_slice(self.block.heap_span());
        v
    }

    ///writes one record to `path` (multi-record files are Stacks).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        std::fs::File::create(path)?.write_all(&self.to_record())
    }

    ///rebuilds from a record — byte-for-byte, no field decode. the whole data
    ///region is copied as-is, so on-disk garbage in dead bytes stays garbage:
    ///records are trusted, not sandboxed.
    pub fn from_record(record: &[u8]) -> Result<Self, LoadErr> {
        Self::validate(record)?;
        Ok(Stack { block: Block::from_record(record), _t: PhantomData })
    }

    ///loads one record from `path`.
    pub fn load(path: &Path) -> Result<Self, LoadErr> {
        Self::from_record(&std::fs::read(path)?)
    }

    ///head checks: sane cap, whole-record stride, exactly one record, ptr and
    ///front bounds. byte contents are NOT validated (the load is a raw copy).
    fn validate(record: &[u8]) -> Result<(), LoadErr> {
        if record.len() < 16 {
            return Err(LoadErr::TooShort);
        }
        let f4 = |i: usize| {
            u32::from_le_bytes([record[i], record[i + 1], record[i + 2], record[i + 3]])
        };
        let (stack, heap, cap, ptr) = (f4(0), f4(4), f4(8), f4(12));
        if cap < 16 || !cap.is_multiple_of(16) || cap as usize > u16::MAX as usize {
            return Err(LoadErr::BadCap { cap: cap as usize });
        }
        let stride = 16 + cap as usize;
        if !record.len().is_multiple_of(stride) {
            return Err(LoadErr::StrideMismatch { file: record.len(), stride });
        }
        if record.len() / stride > 1 {
            return Err(LoadErr::NotSingleRecord { records: record.len() / stride });
        }
        if ptr != 16 {
            return Err(LoadErr::BadPtr { ptr });
        }
        //the freelist head occupies the last NODE bytes — not front space
        if stack as usize + heap as usize > cap as usize - NODE {
            return Err(LoadErr::Overflow { stack, heap, cap });
        }
        Ok(())
    }
}

impl<T: Archive> Default for Stack<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    use crate::archive::{RelOrd, StrHandle};

    const LONG: &str = "a-string-well-past-the-prefix-cutoff";

    #[test]
    fn push_pop_roundtrip() {
        let strings = ["", "abc", "abcdefgh", "abcdefghi", LONG];
        let mut st = Stack::<String>::new();
        for s in strings {
            st.push(s.to_string()).unwrap();
        }
        assert_eq!(st.len(), strings.len());
        //handles answer rel_cmp per Ord — Equal for their own values
        let heap = st.heap();
        for (s, (_, h)) in strings.iter().zip(st.items()) {
            assert_eq!(String::rel_cmp(h, &s.to_string(), &heap), Ordering::Equal);
        }
        //structural: handles equal a fresh archive of the same values
        let expected: Vec<StrHandle> = {
            let mut b = crate::Block::new(4080);
            let mut al = b.allocator();
            strings.iter().map(|s| String::archive(&s.to_string(), &mut al).unwrap()).collect()
        };
        for (e, (_, h)) in expected.iter().zip(st.items()) {
            assert_eq!(*e, *h);
        }
        //LIFO pops: handles match the fresh archive, stack AND heap empty
        for h in expected.iter().rev() {
            assert_eq!(st.pop().unwrap(), *h);
        }
        assert!(st.pop().is_none());
        assert!(st.is_empty());
        assert_eq!(st.block().heap_len(), 0, "pop erased every heap region");
    }

    ///LIFO pops erase to each unit's snapshot — heap shrinks back per pop and
    ///the record carries nothing of the popped items (no freelist machinery)
    #[test]
    fn pop_erases_to_front() {
        let mut st = Stack::<Vec<String>>::new();
        st.push(vec!["short".to_string(), LONG.to_string(), "abcdefgh".to_string()]).unwrap();
        let heap0 = st.block().heap_len();
        assert!(heap0 >= LONG.len() as usize);
        st.push(vec![LONG.to_string(), "abcdefghi".to_string()]).unwrap();
        let heap1 = st.block().heap_len();
        assert!(heap1 > heap0);
        //last unit's pop erases down to ITS snapshot — unit0's bytes survive
        st.pop().unwrap();
        assert_eq!(st.block().heap_len(), heap0);
        assert_eq!(st.len(), 1);
        //unit0's pop erases the rest: heap 0, stack empty, record all-zero data
        let h0 = st.pop().unwrap();
        assert_eq!(h0.len as usize, 3);
        assert_eq!(st.block().heap_len(), 0);
        assert!(st.is_empty());
        let rec = st.to_record();
        assert!(rec[16..rec.len() - NODE].iter().all(|&b| b == 0));
    }

    ///security pin: after a pop, the record holds NO bytes of the popped item
    ///— stack unit wiped, heap regions erased to the snapshot
    #[test]
    fn pop_leaves_no_residue() {
        let mut st = Stack::<String>::new();
        st.push(LONG.to_string()).unwrap();
        let rec0 = st.to_record();
        assert!(rec0.windows(LONG.len()).any(|w| w == LONG.as_bytes()));
        st.pop().unwrap();
        let rec = st.to_record();
        assert_eq!(rec.len(), 4096);
        assert!(!rec.windows(LONG.len()).any(|w| w == LONG.as_bytes()));
        //the stack region itself reads zero where the unit sat
        let top = 16 + u32::from_le_bytes([rec0[0], rec0[1], rec0[2], rec0[3]]) as usize;
        assert!(rec[16..top].iter().all(|&b| b == 0));
    }

    #[test]
    fn push_failure_leaves_both_ends_unchanged() {
        //archive-side: heap bytes exceed the block
        let mut st = Stack::<String>::with_cap(64);
        let (heap0, len0, free0) = (st.block().heap_len(), st.len(), st.block().free_bytes());
        assert!(
            st.push("way beyond the whole block capacity ______________________".to_string())
                .is_err()
        );
        assert_eq!(st.block().heap_len(), heap0);
        assert_eq!(st.len(), len0);
        assert_eq!(st.block().free_bytes(), free0);

        //stack-side: empty Vec handles cost no heap — the failure is
        //exclusively the stack front meeting the heap bottom
        let mut st2 = Stack::<Vec<u32>>::with_cap(64);
        //unit = snap 4 + VecHandle 8 = 12 bytes (align 2 — no gaps)
        while st2.block().free_bytes() >= 12 {
            st2.push(vec![]).unwrap();
        }
        assert!(st2.block().free_bytes() < 12);
        let (heap1, len1) = (st2.block().heap_len(), st2.len());
        assert!(st2.push(vec![]).is_err());
        assert_eq!(st2.block().heap_len(), heap1);
        assert_eq!(st2.len(), len1);
        //the failed push left no residue (its archive was unwound); a pop
        //reopens the slot
        st2.pop().unwrap();
        st2.push(vec![]).expect("pop freed the stack slot");
        assert_eq!(st2.len(), len1);
    }

    ///a failed push near-full leaves the stack front EXACTLY where it was —
    ///the landed snap's alignment gap is reclaimed too (sub-4-aligned
    ///handles), and pop keeps answering (the stride canary stays honest).
    ///regression: shrinking to the snap's aligned start stranded the gap,
    ///broke the canary on the next pop
    #[test]
    fn failed_push_leaves_no_stack_residue() {
        let mut st = Stack::<u16>::with_cap(64);
        while st.push(0xABu16).is_ok() {}
        let (front, len) = (st.block().stack_len(), st.len());
        assert!(st.push(0xCD).is_err(), "the block is full");
        assert_eq!(st.block().stack_len(), front, "gap residue reclaimed");
        assert_eq!(st.len(), len);
        assert_eq!(st.pop().unwrap(), 0xAB);
        assert!(st.push(0xCD).is_ok());
    }

    #[test]
    fn record_layout_is_exact_stride() {
        let mut st = Stack::<String>::new();
        st.push("abcdefghi".to_string()).unwrap();
        st.push("tiny".to_string()).unwrap();
        let rec = st.to_record();
        assert_eq!(st.block().cap(), 4080);
        assert_eq!(rec.len(), 4096);
        //header byte-exact: stack, heap, cap, ptr — LE u32s
        assert_eq!(&rec[0..4], &(st.block().stack_len() as u32).to_le_bytes());
        assert_eq!(&rec[4..8], &(st.block().heap_len() as u32).to_le_bytes());
        assert_eq!(&rec[8..12], &4080u32.to_le_bytes());
        assert_eq!(&rec[12..16], &16u32.to_le_bytes());
        //dead gap reads as zeros (the head's last 4 bytes are the heap span)
        let (s0, h0) = (st.block().stack_len(), st.block().heap_len());
        assert!(rec[16 + s0..16 + 4080 - NODE - h0].iter().all(|&b| b == 0));
        //record seam round-trip, re-serialization identical
        let loaded = Stack::<String>::from_record(&rec).unwrap();
        assert_eq!(loaded.items().collect::<Vec<_>>(), st.items().collect::<Vec<_>>());
        assert_eq!(loaded.to_record(), rec);
    }

    #[test]
    fn load_rejects_corrupt_records() {
        let mut st = Stack::<String>::new();
        st.push(LONG.to_string()).unwrap();
        let rec = st.to_record();
        //truncated mid-record
        assert!(matches!(
            Stack::<String>::from_record(&rec[..rec.len() - 1]),
            Err(LoadErr::StrideMismatch { file: 4095, stride: 4096 })
        ));
        //below header size
        assert!(matches!(Stack::<String>::from_record(&rec[..10]), Err(LoadErr::TooShort)));
        assert!(matches!(Stack::<String>::from_record(&[]), Err(LoadErr::TooShort)));
        //zero cap
        let bad: Vec<u8> = {
            let mut v = rec.clone();
            v[8..12].copy_from_slice(&0u32.to_le_bytes());
            v
        };
        assert!(matches!(Stack::<String>::from_record(&bad), Err(LoadErr::BadCap { cap: 0 })));
        //cap past the pointer width
        let bad: Vec<u8> = {
            let mut v = rec.clone();
            v[8..12].copy_from_slice(&65536u32.to_le_bytes());
            v
        };
        assert!(matches!(
            Stack::<String>::from_record(&bad),
            Err(LoadErr::BadCap { cap: 65536 })
        ));
        //two records in one file
        let mut two = rec.clone();
        two.extend_from_slice(&rec);
        assert!(matches!(
            Stack::<String>::from_record(&two),
            Err(LoadErr::NotSingleRecord { records: 2 })
        ));
        //ptr != 16
        let bad = {
            let mut v = rec.clone();
            v[12..16].copy_from_slice(&17u32.to_le_bytes());
            v
        };
        assert!(matches!(Stack::<String>::from_record(&bad), Err(LoadErr::BadPtr { ptr: 17 })));
        //fronts past cap
        let bad = {
            let mut v = rec.clone();
            v[0..4].copy_from_slice(&4081u32.to_le_bytes());
            v
        };
        assert!(matches!(Stack::<String>::from_record(&bad), Err(LoadErr::Overflow { .. })));
    }

    ///v0 Stack is ONE block: full is a terminal Err, no auto-grow — contents
    ///stay readable and the slot reopens on pop
    #[test]
    fn capacity_is_terminal_single_block() {
        let mut st = Stack::<Vec<u32>>::new();
        let mut pushed = 0usize;
        while st.push((0..1000u32).collect()).is_ok() {
            pushed += 1;
        }
        assert!(pushed > 0);
        assert_eq!(st.len(), pushed);
        assert_eq!(st.items().count(), pushed);
        st.pop().unwrap();
        st.push(vec![7]).unwrap();
        assert_eq!(st.len(), pushed);
        assert_eq!(st.last().unwrap().1.len as usize, 1);
    }

    ///the two-push unit layout: u64 units are [snap 4][gap 4][handle 8] =
    ///16 bytes — byte-identical to the old Unit struct, and every byte of a
    ///pushed unit is defined (gap block-zeroed, handle ZeroPad)
    #[test]
    fn unit_layout_pads_zero() {
        let mut st = Stack::<u64>::new();
        st.push(0xDEAD_BEEF_0123_4567).unwrap();
        st.push(1).unwrap();
        assert_eq!(st.block().stack_len(), 32, "two 16-byte units");
        let rec = st.to_record();
        //unit0: snap@0..4, gap@4..8 (zero), handle@8..16
        assert_eq!(&rec[16 + 4..16 + 8], &[0; 4], "unit gap zero");
        assert_eq!(&rec[16 + 8..16 + 16], &0xDEAD_BEEF_0123_4567u64.to_le_bytes());
        //round-trip byte-exact
        let loaded = Stack::<u64>::from_record(&rec).unwrap();
        assert_eq!(loaded.items().collect::<Vec<_>>(), st.items().collect::<Vec<_>>());
        assert_eq!(loaded.to_record(), rec);
        //sub-4-aligned handles take the gap0 stride (u16: unit 6, stride 8)
        let mut st2 = Stack::<u16>::new();
        st2.push(0xAB).unwrap();
        st2.push(0xCD).unwrap();
        assert_eq!(st2.block().stack_len(), 6 + 8, "irregular unit0 then steady stride");
        assert_eq!(st2.pop().unwrap(), 0xCD);
        assert_eq!(st2.pop().unwrap(), 0xAB);
        assert!(st2.pop().is_none());
    }

    #[cfg(not(miri))]
    #[test]
    fn save_load_file_roundtrip() {
        let mut st = Stack::<String>::new();
        for s in ["tiny", "abcdefgh", "abcdefghi", LONG] {
            st.push(s.to_string()).unwrap();
        }
        let path = std::env::temp_dir().join("block-stack-roundtrip.test");
        st.save(&path).unwrap();
        let file = std::fs::read(&path).unwrap();
        assert_eq!(file, st.to_record());
        assert_eq!(file.len(), 4096);

        let loaded = Stack::<String>::load(&path).unwrap();
        assert_eq!(loaded.len(), st.len());
        assert_eq!(loaded.items().collect::<Vec<_>>(), st.items().collect::<Vec<_>>());
        //rel_cmp_h ordering matches across both heaps, all pairs
        let hs: Vec<_> = st.items().map(|(_, h)| *h).collect();
        for a in &hs {
            for b in &hs {
                assert_eq!(
                    String::rel_cmp_h(a, b, &loaded.heap()),
                    String::rel_cmp_h(a, b, &st.heap())
                );
            }
        }
        //the loaded stack re-serializes identically
        assert_eq!(loaded.to_record(), st.to_record());
        std::fs::remove_file(&path).unwrap();
    }
}
