//!append-ordered storage over `Block<P>`: push archives `T` through the
//!Allocator and stores the unit (heap_len snapshot, Handle) on the stack
//!front; pop shrinks to the unit's start and `pop_heap`s to its snapshot —
//!both wipes now, so a LIFO pop ERASES the item's stack bytes and heap
//!regions completely (no freelist machinery; `T::release` stays available
//!for non-LIFO consumers, Stack never calls it). unordered storage —
//!ordering/search stays the consumer's job (`RelOrd`/`Heap` are reads, not
//!duties). v0 is ONE block, no growth: full ⇒ `CapErr` (multi-block files
//!are Stacks, task 6).
//!serialized record = [Header 16B | cap data]; `Stack::new` picks
//!cap = RECORD − 16 so the record is exactly 4096 bytes. the dead gap writes
//!as zeros and the zero-record invariant covers everything else — a record
//!is live data or zero, byte-for-byte round-tripped (no field decode). the
//!snapshot rides the record, so `from_record` needs no extra state.

use std::fmt;
use std::io::{self, Write};
use std::marker::PhantomData;
use std::mem::{align_of, size_of};
use std::path::Path;
use std::slice;

use crate::archive::{Archive, Heap};
use crate::block::{Block, CapErr};
use crate::handle::{H, Ptr, S};

///default record size — devlog §5b's 4096-byte block. cap = RECORD − 16.
pub const RECORD: usize = 4096;

///stack unit: the heap_len taken BEFORE the item's archive, plus the handle.
///the snapshot is u32 (P-independent — cap never exceeds u32). repr(C) keeps
///`h` at a fixed offset `max(4, align_of::<H>())`.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct Unit<H> {
    snap: u32,
    h: H,
}

pub struct Stack<T, P = u32> {
    block: Block<P>,
    _t: PhantomData<T>,
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
            Self::BadCap { cap } => write!(f, "bad cap {cap} (below 16 or not 16-multiple)"),
            Self::BadPtr { ptr } => write!(f, "bad ptr {ptr} (must be 16)"),
            Self::StrideMismatch { file, stride } => {
                write!(f, "file length {file} is not a multiple of record stride {stride}")
            }
            Self::NotSingleRecord { records } => write!(f, "{records} records — Stack is single-block"),
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

impl<T: Archive<P>, P: Ptr> Stack<T, P> {
    ///cap chosen so the serialized record is exactly `RECORD` bytes.
    pub fn new() -> Self {
        Self::with_cap(RECORD - 16)
    }

    ///data cap = `cap` rounded up to 16; record stride = 16 + rounded cap.
    pub fn with_cap(cap: usize) -> Self {
        Stack { block: Block::new(cap), _t: PhantomData }
    }

    ///archives `k`, then pushes the unit (pre-archive heap_len, handle) onto
    ///the stack. on Err both ends are unchanged: archive's own Err leaves the
    ///heap untouched, a stack-side Err rewinds (wipes) the archived regions.
    pub fn push(&mut self, k: T) -> Result<S<P>, CapErr> {
        self.push_borrowed(&k)
    }

    ///push that borrows k — on CapErr the value is intact, so a caller can
    ///retry the item into a fresh block without a Clone bound (Stacks' spill).
    pub fn push_borrowed(&mut self, k: &T) -> Result<S<P>, CapErr> {
        let (h, snap) = {
            let mut al = self.block.allocator();
            let snap = al.snapshot();
            (T::archive(k, &mut al)?, snap)
        };
        let s = match self.block.push(Unit { snap: snap.heap as u32, h }) {
            Ok(s) => s,
            Err(e) => {
                //stack front met the heap bottom — rewind (wipe) the archive
                let mut al = self.block.allocator();
                al.rewind(snap);
                return Err(e);
            }
        };
        //the unit's own struct padding carries uninit bytes — wipe for the
        //zero-record invariant (h's internal padding is the handle type's duty)
        let (hs, ha) = (size_of::<T::Handle>(), align_of::<T::Handle>());
        let h_off = ha.max(4);
        let u0 = s.0.as_usize();
        let bytes = self.block.stack_bytes_mut();
        bytes[u0 + 4..u0 + h_off].fill(0);
        bytes[u0 + h_off + hs..u0 + s.1.as_usize()].fill(0);
        Ok(s)
    }

    ///pops the LAST unit: shrinks (wipes) its stack bytes and `pop_heap`s to
    ///its snapshot (wiping every heap byte above it — that is the item's own
    ///regions under strict LIFO; any unit pushed after it is already gone, or
    ///its push failed and rewound). returns the handle — its bytes are wiped
    ///and its regions erased, bookkeeping only. no owned-T materialization.
    pub fn pop(&mut self) -> Option<T::Handle> {
        let u = *self.units().last()?;
        self.block.stack_shrink(self.block.stack_len() - size_of::<Unit<T::Handle>>());
        self.block.pop_heap(self.block.heap_len() - u.snap as usize);
        Some(u.h)
    }

    ///the stored units in push order — one consecutive stride. `snap` names
    ///the heap extent the unit's pop will erase to.
    fn units(&self) -> &[Unit<T::Handle>] {
        let size = size_of::<Unit<T::Handle>>();
        let bytes = self.item_bytes();
        let raw = self.block.stack_bytes();
        assert!(
            (raw.as_ptr() as usize).is_multiple_of(align_of::<Unit<T::Handle>>()),
            "unit misaligned"
        );
        // SAFETY: Stack owns its block and only ever pushes one unit type —
        // Block::push places every unit at a multiple of its own size
        // (size_of is a multiple of align_of), so the stack IS [Unit; n]
        unsafe { slice::from_raw_parts(raw.as_ptr().cast(), bytes / size) }
    }

    ///(snapshot, handle) pairs in push order, borrowing the stack.
    pub fn items(&self) -> impl Iterator<Item = (u32, &T::Handle)> {
        self.units().iter().map(|u| (u.snap, &u.h))
    }

    ///the last unit's (snapshot, handle), without popping.
    pub fn last(&self) -> Option<(u32, &T::Handle)> {
        self.units().last().map(|u| (u.snap, &u.h))
    }

    ///item count = stack bytes / unit size.
    pub fn len(&self) -> usize {
        self.item_bytes() / size_of::<Unit<T::Handle>>()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    ///read seam over the block's heap regions.
    pub fn heap(&self) -> Heap<'_, P> {
        Heap::new(&self.block)
    }

    pub fn block(&self) -> &Block<P> {
        &self.block
    }

    ///the serialized record: [Header | stack | zero gap | heap]. two records
    ///of equal stacks are byte-identical (zero-record invariant + zeroed gap).
    pub fn to_record(&self) -> Vec<u8> {
        let (stack, heap, cap) = (self.block.stack_len(), self.block.heap_len(), self.block.cap());
        let mut v = Vec::with_capacity(16 + cap);
        v.extend_from_slice(&(stack as u32).to_le_bytes());
        v.extend_from_slice(&(heap as u32).to_le_bytes());
        v.extend_from_slice(&(cap as u32).to_le_bytes());
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(self.block.stack_bytes());
        v.resize(16 + cap - heap, 0);
        //whole heap span: zero-record invariant keeps even alignment pads defined
        v.extend_from_slice(self.block.heap(H(P::from_usize(heap), P::from_usize(heap))));
        v
    }

    ///writes one record to `path` (multi-record files are Stacks, task 6).
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
        let f4 = |i: usize| u32::from_le_bytes([record[i], record[i + 1], record[i + 2], record[i + 3]]);
        let (stack, heap, cap, ptr) = (f4(0), f4(4), f4(8), f4(12));
        if cap < 16 || !cap.is_multiple_of(16) {
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
        if stack as usize + heap as usize > cap as usize {
            return Err(LoadErr::Overflow { stack, heap, cap });
        }
        Ok(())
    }

    ///stack bytes, all of them units, no gaps. panics on stride violation
    ///(misuse through the escaping `block()` view).
    fn item_bytes(&self) -> usize {
        let size = size_of::<Unit<T::Handle>>();
        let len = self.block.stack_len();
        assert!(len.is_multiple_of(size), "stack/unit stride broken");
        len
    }
}

impl<T: Archive<P>, P: Ptr> Default for Stack<T, P> {
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
        let expected: Vec<StrHandle<u32>> = {
            let mut b = crate::Block::<u32>::new(4080);
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
    fn pop_erases_to_snapshot() {
        let mut st = Stack::<Vec<String>>::new();
        st.push(vec!["short".to_string(), LONG.to_string(), "abcdefgh".to_string()]).unwrap();
        let heap0 = st.block().heap_len();
        assert!(heap0 >= LONG.len());
        st.push(vec![LONG.to_string(), "abcdefghi".to_string()]).unwrap();
        let heap1 = st.block().heap_len();
        assert!(heap1 > heap0);
        //last unit's pop erases down to ITS snapshot — unit0's bytes survive
        st.pop().unwrap();
        assert_eq!(st.block().heap_len(), heap0);
        assert_eq!(st.len(), 1);
        //unit0's pop erases the rest: heap 0, stack empty, record all-zero data
        let h0 = st.pop().unwrap();
        assert_eq!(h0.len.as_usize(), 3);
        assert_eq!(st.block().heap_len(), 0);
        assert!(st.is_empty());
        let rec = st.to_record();
        assert!(rec[16..].iter().all(|&b| b == 0));
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
        assert!(st.push("way beyond the whole block capacity ______________________".to_string()).is_err());
        assert_eq!(st.block().heap_len(), heap0);
        assert_eq!(st.len(), len0);
        assert_eq!(st.block().free_bytes(), free0);

        //stack-side: empty Vec handles cost no heap — the failure is
        //exclusively the stack front meeting the heap bottom
        let mut st2 = Stack::<Vec<u32>>::with_cap(64);
        while st2.block().free_bytes() >= 16 {
            st2.push(vec![]).unwrap();
        }
        assert!(st2.block().free_bytes() < 16);
        let (heap1, len1) = (st2.block().heap_len(), st2.len());
        assert!(st2.push(vec![]).is_err());
        assert_eq!(st2.block().heap_len(), heap1);
        assert_eq!(st2.len(), len1);
        //the failed push left no residue (its archive was rewound); a pop
        //reopens the slot
        st2.pop().unwrap();
        st2.push(vec![]).expect("pop freed the stack slot");
        assert_eq!(st2.len(), len1);
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
        //dead gap reads as zeros
        let (s0, h0) = (st.block().stack_len(), st.block().heap_len());
        assert!(rec[16 + s0..16 + 4080 - h0].iter().all(|&b| b == 0));
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
        assert_eq!(st.last().unwrap().1.len.as_usize(), 1);
    }

    ///u16-width record round-trip over VecH<u16>, whose explicit pad
    ///(P::TailPad3) reads zero in the record; the String twin below pins
    ///StrHandle's pad at this width.
    #[test]
    fn narrow_pointer_width_record() {
        let mut st = Stack::<Vec<u32>, u16>::with_cap(256);
        st.push(vec![1, 2, 3]).unwrap();
        st.push(vec![]).unwrap();
        let rec = st.to_record();
        assert_eq!(rec.len(), 16 + 256);
        let loaded = Stack::<Vec<u32>, u16>::from_record(&rec).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.items().collect::<Vec<_>>(), st.items().collect::<Vec<_>>());
        assert_eq!(loaded.to_record(), rec);
        //order survives the round-trip
        let its: Vec<_> = st.items().collect();
        assert_eq!(Vec::<u32>::rel_cmp_h(its[0].1, its[1].1, &loaded.heap()), Ordering::Greater);
        let mut l2 = loaded;
        assert_eq!(l2.pop().unwrap().len.as_usize(), 0);
    }

    ///T2's pad fix closed the invariant exception: StrHandle<u16> carries an
    ///explicit zero pad — pinned byte-level in the record, round-tripped at
    ///this width, ordering intact, pop erasure to snapshot holding.
    #[test]
    fn narrow_string_record_pads_zero() {
        let mut st = Stack::<String, u16>::with_cap(256);
        for s in ["abcdefghi", LONG, "abc"] {
            st.push(s.to_string()).unwrap();
        }
        assert_eq!(st.block().stack_len(), 3 * 24, "unit = snap 4 + handle 16, align 8");
        let rec = st.to_record();
        assert_eq!(rec.len(), 16 + 256);
        //each unit's StrHandle pad bytes (handle offsets [12..16] at unit
        //offset 8) read zero — no uninit struct padding reaches the record
        for i in 0..3 {
            let u = 16 + i * 24;
            assert!(rec[u + 8 + 12..u + 8 + 16].iter().all(|&b| b == 0), "unit {i} pad");
        }
        //round-trip + re-serialization identical
        let loaded = Stack::<String, u16>::from_record(&rec).unwrap();
        assert_eq!(loaded.items().collect::<Vec<_>>(), st.items().collect::<Vec<_>>());
        assert_eq!(loaded.to_record(), rec);
        //order survives and matches Ord
        let hs: Vec<_> = loaded.items().map(|(_, h)| h).collect();
        assert_eq!(String::rel_cmp_h(hs[0], hs[1], &loaded.heap()), "abcdefghi".cmp(LONG));
        //pop erases to each unit's snapshot at this width too
        while !st.is_empty() {
            let (snap, _) = st.last().unwrap();
            st.pop().unwrap();
            assert_eq!(st.block().heap_len(), snap as usize);
        }
        assert_eq!(st.block().heap_len(), 0);
        let rec2 = st.to_record();
        assert!(rec2[16..].iter().all(|&b| b == 0));
        assert!(!rec2.windows(LONG.len()).any(|w| w == LONG.as_bytes()));
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
        let hs: Vec<_> = st.items().map(|(_, h)| h).collect();
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