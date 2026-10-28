//!derive behavior over the block seams: Ord agreement, handle shape,
//!archive atomicity, release order — the devlog §5c composite shapes.
//!Pair stands in for §5c's `(i32,f32)` field: tuples have no Archive impl —
//!v0 wraps them in a named-field type with manual impls. f32 → i64: block
//!carries no float impl.

use std::cell::RefCell;
use std::cmp::Ordering;

use block::{Allocator, Archive, Block, CapErr, Heap, Ptr, RelOrd, StrHandle};
use block_derive::{Archive, RelOrd};

///archive every k into one block; both cmp forms match Ord over all pairs
fn assert_cmp<K: RelOrd<u32> + Ord>(ks: &[K]) {
    let mut b = Block::<u32>::new(1 << 16);
    let hs = {
        let mut al = b.allocator();
        ks.iter().map(|k| K::archive(k, &mut al).unwrap()).collect::<Vec<_>>()
    };
    let heap = Heap::new(&b);
    for (i, a) in ks.iter().enumerate() {
        for (j, c) in ks.iter().enumerate() {
            assert_eq!(K::rel_cmp(&hs[i], c, &heap), a.cmp(c), "pair ({i},{j})");
            assert_eq!(K::rel_cmp_h(&hs[i], &hs[j], &heap), a.cmp(c));
        }
    }
}

// §5c shape A, Pair standing in for the tuple field. PairHandle carries an
// EXPLICIT pad (manual impls follow the same record law: every byte defined):
// i64's alignment over i32 leaves 4 implicit bytes otherwise
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PairHandle {
    lo: i32,
    _pad_lo: [u8; 4],
    hi: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Pair {
    lo: i32,
    hi: i64,
}

impl<P: Ptr> Archive<P> for Pair {
    //inline fields: the handle is P-free — no marker field needed here
    type Handle = PairHandle;
    const HEAP: bool = false;
    fn archive(k: &Self, _al: &mut Allocator<'_, P>) -> Result<Self::Handle, CapErr> {
        Ok(PairHandle { lo: k.lo, _pad_lo: [0; 4], hi: k.hi })
    }
    fn release(_h: &Self::Handle, _al: &mut Allocator<'_, P>) {}
}

impl<P: Ptr> RelOrd<P> for Pair {
    fn rel_cmp(h: &Self::Handle, k: &Self, _heap: &Heap<P>) -> Ordering {
        h.lo.cmp(&k.lo).then(h.hi.cmp(&k.hi))
    }
    fn rel_cmp_h(a: &Self::Handle, b: &Self::Handle, _heap: &Heap<P>) -> Ordering {
        a.lo.cmp(&b.lo).then(a.hi.cmp(&b.hi))
    }
}

#[derive(Archive, RelOrd)]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct A {
    a: i32,
    b: i64,
    c: Pair,
}

#[derive(Archive, RelOrd)]
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct B {
    a: Vec<i32>,
    b: String,
}

#[derive(Archive, RelOrd)]
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct C {
    s: Vec<String>,
    h: i32,
}

// §5c shape D: derive-on-derive recursion (AHandle rides inside DHandle)
#[derive(Archive, RelOrd)]
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct D {
    a: A,
    c: C,
}

#[derive(Archive, RelOrd)]
struct Empty {}

//the mixed-width record shape: a u32-ish field ordered BEFORE the String —
//the cross-field pad ([0; 4] after num) is P-independent and the trailing
//StrHandle lands on a multiple of 8 at EVERY width, so the canary passes for
//all P. the reverse order (String first) leaves a P-independent-4-byte tail
//only at width 4 — the narrow-width record hole, refused there by the canary
#[derive(Archive, RelOrd)]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct G {
    num: u32,
    name: String,
}

thread_local! {
    static LOG: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

struct Tag<const N: u8>;

impl<P: Ptr, const N: u8> Archive<P> for Tag<N> {
    type Handle = u8;
    const HEAP: bool = false;
    fn archive(_k: &Self, _al: &mut Allocator<'_, P>) -> Result<Self::Handle, CapErr> {
        Ok(N)
    }
    fn release(_h: &Self::Handle, _al: &mut Allocator<'_, P>) {
        LOG.with(|l| l.borrow_mut().push(N));
    }
}

impl<P: Ptr, const N: u8> RelOrd<P> for Tag<N> {
    fn rel_cmp(_h: &Self::Handle, _k: &Self, _heap: &Heap<P>) -> Ordering {
        Ordering::Equal
    }
    fn rel_cmp_h(_a: &Self::Handle, _b: &Self::Handle, _heap: &Heap<P>) -> Ordering {
        Ordering::Equal
    }
}

//release-order shapes: per-field order (reverse of declaration) and recursion
//through a derived field type
#[derive(Archive, RelOrd)]
struct Inner {
    x: Tag<1>,
    y: Tag<2>,
}

#[derive(Archive, RelOrd)]
struct Outer {
    lead: Inner,
    tail: Vec<Tag<3>>,
}

fn log() -> Vec<u8> {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

#[test]
fn heap_consts_fold_from_fields() {
    assert!(!<A as Archive<u32>>::HEAP, "all fields inline");
    assert!(<B as Archive<u32>>::HEAP);
    assert!(<C as Archive<u32>>::HEAP);
    assert!(<D as Archive<u32>>::HEAP);
    assert!(!<Empty as Archive<u32>>::HEAP);
    //A archives without touching the heap
    let mut b = Block::<u32>::new(64);
    {
        let mut al = b.allocator();
        <A as Archive<u32>>::archive(&A { a: 1, b: 2, c: Pair { lo: 3, hi: 4 } }, &mut al).unwrap();
    }
    assert_eq!(b.heap_len(), 0);
}

#[test]
fn empty_struct_edges() {
    let mut b = Block::<u32>::new(64);
    let h = { let mut al = b.allocator(); Empty::archive(&Empty {}, &mut al).unwrap() };
    assert_eq!(b.heap_len(), 0);
    let heap = Heap::new(&b);
    assert_eq!(Empty::rel_cmp(&h, &Empty {}, &heap), Ordering::Equal);
    assert_eq!(Empty::rel_cmp_h(&h, &h, &heap), Ordering::Equal);
}

///§5c shape B: the flat-vec and string layouts through the derived handle
#[test]
fn b_handle_shape_roundtrip() {
    let mut b = Block::<u32>::new(256);
    let k = B { a: vec![1, 2, 3], b: "abcdefghi".to_string() };
    let h = { let mut al = b.allocator(); B::archive(&k, &mut al).unwrap() };
    let heap = Heap::new(&b);
    //Vec<i32> flat: one region of raw u32s
    assert_eq!(h.a.len.as_usize(), 3);
    for (i, v) in [1u32, 2, 3].iter().enumerate() {
        assert_eq!(heap.handle::<u32>(h.a.h, i), v);
    }
    assert_eq!(h.a.h.1.as_usize(), 12);
    //string: 9 bytes rides the heap, pfx the first 8
    assert_eq!(heap.bytes(h.b.h), b"abcdefghi");
    assert_eq!(h.b.pfx, u64::from_be_bytes(*b"abcdefgh"));
}

///§5c shape D: the derived handle nests derived handles field-by-field;
///inline fields read back as values, string/vec regions resolve through Heap
#[test]
fn d_handle_shape_roundtrip() {
    let mut b = Block::<u32>::new(256);
    let k = D {
        a: A { a: 1, b: 2, c: Pair { lo: 3, hi: 4 } },
        c: C { s: vec!["abcdefghi".to_string(), "z".to_string()], h: 5 },
    };
    let h = { let mut al = b.allocator(); D::archive(&k, &mut al).unwrap() };
    let heap = Heap::new(&b);
    //A is inline: values ride the handle directly
    assert_eq!(h.a.a, 1);
    assert_eq!(h.a.b, 2);
    assert_eq!(h.a.c.lo, 3);
    assert_eq!(h.a.c.hi, 4);
    //C's vec handles region holds StrHandles; "z" is inline, "abcdefghi" heap
    assert_eq!(h.c.h, 5);
    assert_eq!(h.c.s.len.as_usize(), 2);
    let s0 = heap.handle::<StrHandle<u32>>(h.c.s.h, 0);
    assert_eq!(heap.bytes(s0.h), b"abcdefghi");
    let s1 = heap.handle::<StrHandle<u32>>(h.c.s.h, 1);
    assert_eq!(s1.h.0.as_usize(), 0, "inline element");
    //layout: 2 × StrHandle<u32> (32 bytes) + 9 string bytes
    assert_eq!(b.heap_len(), 41);
}

#[test]
fn cmp_matches_ord() {
    //shape A: field-wise of inline ints + the manual Pair (per-field lo, hi)
    assert_cmp(&[
        A { a: 0, b: 0, c: Pair { lo: 0, hi: 0 } },
        A { a: 0, b: 0, c: Pair { lo: 0, hi: 1 } },
        A { a: 0, b: 1, c: Pair { lo: 0, hi: 0 } },
        A { a: 1, b: 0, c: Pair { lo: 5, hi: 0 } },
        A { a: i32::MIN, b: i64::MAX, c: Pair { lo: -1, hi: i64::MIN } },
    ]);
    //shape B: flat vec lex order + the string prefix family
    assert_cmp(&[
        B { a: vec![], b: "".into() },
        B { a: vec![0], b: "a".into() },
        B { a: vec![1], b: "abcdefghi".into() },
        B { a: vec![1, u32::MAX as i32], b: "abcdefghi".into() },
        B { a: vec![1, 2], b: "abcdefgh\0".into() },
        B { a: vec![2], b: "b".into() },
    ]);
    //shape C: vec-vs-h cross ties both directions
    assert_cmp(&[
        C { s: vec![], h: 0 },
        C { s: vec!["".into()], h: 0 },
        C { s: vec!["abcdefghi".into()], h: 0 },
        C { s: vec!["abcdefghi".into(), "z".into()], h: 1 },
        C { s: vec!["abcdefghj".into()], h: 1 },
        C { s: vec!["b".into()], h: 2 },
    ]);
    //shape D: nested A × C over both tie axes
    let as_ = [
        A { a: 0, b: 0, c: Pair { lo: 0, hi: 0 } },
        A { a: 1, b: 0, c: Pair { lo: 5, hi: 0 } },
    ];
    let cs = [
        vec![],
        vec!["".to_string()],
        vec!["abcdefghi".to_string()],
        vec!["abcdefghi".to_string(), "z".to_string()],
    ];
    let mut dks = Vec::new();
    for (i, a) in as_.iter().enumerate() {
        for (j, s) in cs.iter().enumerate() {
            dks.push(D { a: a.clone(), c: C { s: s.clone(), h: (i + j) as i32 } });
        }
    }
    assert_cmp(&dks);
}

///mid-loop failure inside a derived field: the entry snapshot rewinds the
///whole composite — heap_len and free unchanged, space reusable
#[test]
fn failed_archive_atomic() {
    let mut b = Block::<u32>::new(128);
    let ks: Vec<String> = (0..6).map(|i| format!("string-{i}-padding")).collect();
    let k = C { s: ks, h: 0 };
    let (heap0, free0) = (b.heap_len(), b.free_bytes());
    let r = { let mut al = b.allocator(); C::archive(&k, &mut al) };
    assert!(r.is_err(), "6 × 16-byte handles + strings exceed 128 bytes free");
    assert_eq!(b.heap_len(), heap0);
    assert_eq!(b.free_bytes(), free0);
    {
        let mut al = b.allocator();
        C::archive(&C { s: vec!["twelve_bytes".into()], h: 0 }, &mut al).unwrap();
    }
    assert_eq!(b.heap_len(), 16 + 12, "rewound space is reusable");
}

///derived release = per-field releases in REVERSE declaration order, each
///field's impl recursing children-first: tail's two Tag<3>s, then Inner's
///y(2) then x(1)
#[test]
fn release_order_children_first() {
    let mut b = Block::<u32>::new(64);
    let k = Outer {
        lead: Inner { x: Tag::<1>, y: Tag::<2> },
        tail: vec![Tag::<3>, Tag::<3>],
    };
    let oh = { let mut al = b.allocator(); Outer::archive(&k, &mut al).unwrap() };
    {
        let mut al = b.allocator();
        Outer::release(&oh, &mut al);
    }
    assert_eq!(log(), vec![3u8, 3, 2, 1]);
    //inline-only composite: release touches nothing
    let mut b = Block::<u32>::new(64);
    let ah = {
        let mut al = b.allocator();
        A::archive(&A { a: 1, b: 2, c: Pair { lo: 3, hi: 4 } }, &mut al).unwrap()
    };
    {
        let mut al = b.allocator();
        A::release(&ah, &mut al);
    }
    assert!(log().is_empty());
    assert_eq!(b.heap_len(), 0);
}

///narrow-width record hole closed: G archived at pointer width u16, pushed
///into a stack record — every pad byte (cross-field 4..8, StrHandle's
///internal tail 20..24) is DEFINED and reads zero; rel_cmp agrees with Ord
#[test]
fn narrow_record_zero_pads() {
    let mut b = Block::<u16>::new(256);
    let k = G { num: 0x0102_0304, name: "abcdefghij".to_string() };
    let h = { let mut al = b.allocator(); G::archive(&k, &mut al).unwrap() };
    assert_eq!(std::mem::size_of::<GHandle<u16>>(), 24);
    let s = b.push::<GHandle<u16>>(h).unwrap();
    let rec = b.stack_bytes();
    assert_eq!(rec.len(), 24);
    assert_eq!(&rec[4..8], &[0; 4], "cross-field pad");
    assert_eq!(&rec[20..24], &[0; 4], "StrHandle tail pad");
    //the string's heap bytes landed; the handle's own zero-pad is untouched
    let k2 = G { num: 0x0102_0305, name: "abcdefghij".to_string() };
    let h2 = { let mut al = b.allocator(); G::archive(&k2, &mut al).unwrap() };
    let _ = s;
    let heap = Heap::new(&b);
    assert_eq!(heap.bytes(h.name.h), b"abcdefghij");
    assert_eq!(h.num, 0x0102_0304);
    assert_eq!(G::rel_cmp(&h, &k, &heap), Ordering::Equal);
    assert_eq!(G::rel_cmp(&h, &k2, &heap), Ordering::Less, "num decides");
    assert_eq!(G::rel_cmp(&h2, &k, &heap), Ordering::Greater);
    assert_eq!(G::rel_cmp_h(&h, &h2, &heap), Ordering::Less);
}