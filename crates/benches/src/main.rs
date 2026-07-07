use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use tiny_trie::NibbleTrie;

// ── bench_query_methods! macro (must precede mod declarations) ─────────
//
// Generates `bench_lookup`, `bench_fwd_iter`, `bench_rev_iter`,
// `bench_fwd_idx`, `bench_rev_idx`, and `lookup_ops` methods for
// `Benchable` impls.  Only the query-side methods; the struct must
// still manually implement `build`, plus any of `bench_insert`,
// `bench_optimize`, and `bench_memory`.
//
// Usage:
//   bench_query_methods! {
//       field: trie,
//       lookup: get(lookup),           // map_get|get|get_unchecked × lookup|null|truncated|hit
//       fwd_iter: iter_kv,             // trie_callback|dyn_callback|iter_kv|iter_kv_no_current|none
//       rev_iter: iter_kv,             // trie_callback|dyn_callback|iter_kv|iter_kv_no_current|none
//       index_iter: true,              // optional, default false
//       unchecked: true,               // optional, default false — overrides lookup_ops to hit_keys
//   }

macro_rules! bench_query_methods {
    // ── Top-level entry: parse the spec and dispatch ────────────────
    //
    // `ctx: $ctx:ident` is the `BenchCtx<K>` type alias for the contestant's key
    // type — `BenchContext` (= `BenchCtx<Vec<u8>>`) for byte-string trees,
    // `BenchContextNz` (= `BenchCtx<NonZeroBytes>`) for non-zero-byte trees. It
    // threads the ctx type into `@lookup` and `@ops` so the generated method
    // signatures match `Benchable<K>`. `@fwd`/`@rev`/`@idx` take no key set, so
    // they don't need it.
    // Full form: with index_iter and unchecked
    (
        field: $field:ident,
        ctx: $ctx:ident,
        lookup: $lookup_method:ident($key_set:ident),
        fwd_iter: $fwd_style:ident,
        rev_iter: $rev_style:ident,
        index_iter: $idx:tt,
        unchecked: $unchecked:tt,
    ) => {
        bench_query_methods!(@lookup $field, $ctx, $lookup_method, $key_set);
        bench_query_methods!(@fwd $field, $fwd_style);
        bench_query_methods!(@rev $field, $rev_style);
        bench_query_methods!(@idx $field, $idx);
        bench_query_methods!(@ops $ctx, $unchecked);
    };
    // With index_iter, without unchecked
    (
        field: $field:ident,
        ctx: $ctx:ident,
        lookup: $lookup_method:ident($key_set:ident),
        fwd_iter: $fwd_style:ident,
        rev_iter: $rev_style:ident,
        index_iter: $idx:tt,
    ) => {
        bench_query_methods!(@lookup $field, $ctx, $lookup_method, $key_set);
        bench_query_methods!(@fwd $field, $fwd_style);
        bench_query_methods!(@rev $field, $rev_style);
        bench_query_methods!(@idx $field, $idx);
        bench_query_methods!(@ops $ctx);
    };
    // Without index_iter, with unchecked
    (
        field: $field:ident,
        ctx: $ctx:ident,
        lookup: $lookup_method:ident($key_set:ident),
        fwd_iter: $fwd_style:ident,
        rev_iter: $rev_style:ident,
        unchecked: $unchecked:tt,
    ) => {
        bench_query_methods!(@lookup $field, $ctx, $lookup_method, $key_set);
        bench_query_methods!(@fwd $field, $fwd_style);
        bench_query_methods!(@rev $field, $rev_style);
        bench_query_methods!(@idx $field);
        bench_query_methods!(@ops $ctx, $unchecked);
    };
    // Minimal form: no index_iter, no unchecked
    (
        field: $field:ident,
        ctx: $ctx:ident,
        lookup: $lookup_method:ident($key_set:ident),
        fwd_iter: $fwd_style:ident,
        rev_iter: $rev_style:ident,
    ) => {
        bench_query_methods!(@lookup $field, $ctx, $lookup_method, $key_set);
        bench_query_methods!(@fwd $field, $fwd_style);
        bench_query_methods!(@rev $field, $rev_style);
        bench_query_methods!(@idx $field);
        bench_query_methods!(@ops $ctx);
    };

    // ── Lookup ────────────────────────────────────────────────────────

    (@lookup $field:ident, $ctx:ident, map_get, lookup) => {
        fn bench_lookup(&self, ctx: &$ctx) -> Option<()> {
            for k in &ctx.lookup_keys { std::hint::black_box(self.$field.map_get(k)); }
            Some(())
        }
    };
    (@lookup $field:ident, $ctx:ident, map_get, null) => {
        fn bench_lookup(&self, ctx: &$ctx) -> Option<()> {
            for k in &ctx.lookup_keys_null { std::hint::black_box(self.$field.map_get(k)); }
            Some(())
        }
    };
    (@lookup $field:ident, $ctx:ident, get, lookup) => {
        fn bench_lookup(&self, ctx: &$ctx) -> Option<()> {
            for k in &ctx.lookup_keys { std::hint::black_box(self.$field.get(k)); }
            Some(())
        }
    };
    (@lookup $field:ident, $ctx:ident, get, null) => {
        fn bench_lookup(&self, ctx: &$ctx) -> Option<()> {
            for k in &ctx.lookup_keys_null { std::hint::black_box(self.$field.get(k)); }
            Some(())
        }
    };
    (@lookup $field:ident, $ctx:ident, get, truncated) => {
        fn bench_lookup(&self, ctx: &$ctx) -> Option<()> {
            for k in &ctx.fl_lookup_keys { std::hint::black_box(self.$field.get(k)); }
            Some(())
        }
    };
    (@lookup $field:ident, $ctx:ident, get_unchecked, hit) => {
        fn bench_lookup(&self, ctx: &$ctx) -> Option<()> {
            for k in &ctx.hit_keys { std::hint::black_box(unsafe { self.$field.get_unchecked(k) }); }
            Some(())
        }
    };

    // ── Forward iteration ──────────────────────────────────────────────

    (@fwd $field:ident, trie_callback) => {
        fn bench_fwd_iter(&self) -> Option<()> {
            self.$field.map_iter_fwd(|k, v| { std::hint::black_box(k); std::hint::black_box(v); });
            Some(())
        }
    };
    (@fwd $field:ident, dyn_callback) => {
        fn bench_fwd_iter(&self) -> Option<()> {
            self.$field.iter_fwd(&mut |k, v| { std::hint::black_box(k); std::hint::black_box(v); });
            Some(())
        }
    };
    (@fwd $field:ident, iter_kv) => {
        fn bench_fwd_iter(&self) -> Option<()> {
            let mut it = self.$field.iter();
            if let Some((k, v)) = it.current() { std::hint::black_box(k); std::hint::black_box(v); }
            while let Some((k, v)) = it.next() { std::hint::black_box(k); std::hint::black_box(v); }
            Some(())
        }
    };
    (@fwd $field:ident, iter_kv_no_current) => {
        fn bench_fwd_iter(&self) -> Option<()> {
            let mut it = self.$field.iter();
            while let Some((k, v)) = it.next() { std::hint::black_box(k); std::hint::black_box(v); }
            Some(())
        }
    };
    (@fwd $field:ident, none) => {
        // no bench_fwd_iter
    };

    // ── Reverse iteration ──────────────────────────────────────────────

    (@rev $field:ident, trie_callback) => {
        fn bench_rev_iter(&self) -> Option<()> {
            self.$field.map_iter_rev(|k, v| { std::hint::black_box(k); std::hint::black_box(v); });
            Some(())
        }
    };
    (@rev $field:ident, dyn_callback) => {
        fn bench_rev_iter(&self) -> Option<()> {
            self.$field.iter_rev(&mut |k, v| { std::hint::black_box(k); std::hint::black_box(v); });
            Some(())
        }
    };
    (@rev $field:ident, iter_kv) => {
        fn bench_rev_iter(&self) -> Option<()> {
            let mut it = self.$field.iter_last();
            if let Some((k, v)) = it.current() { std::hint::black_box(k); std::hint::black_box(v); }
            while let Some((k, v)) = it.prev() { std::hint::black_box(k); std::hint::black_box(v); }
            Some(())
        }
    };
    (@rev $field:ident, iter_kv_no_current) => {
        fn bench_rev_iter(&self) -> Option<()> {
            let mut it = self.$field.iter_last();
            while let Some((k, v)) = it.prev() { std::hint::black_box(k); std::hint::black_box(v); }
            Some(())
        }
    };
    (@rev $field:ident, none) => {
        // no bench_rev_iter
    };

    // ── Index iteration (optional) ──────────────────────────────────

    (@idx $field:ident, true) => {
        fn bench_fwd_idx(&self) -> Option<()> {
            let mut it = self.$field.iter();
            if let Some(i) = it.current_index() { std::hint::black_box(i); }
            while let Some(i) = it.next_index() { std::hint::black_box(i); }
            Some(())
        }
        fn bench_rev_idx(&self) -> Option<()> {
            let mut it = self.$field.iter_last();
            if let Some(i) = it.current_index() { std::hint::black_box(i); }
            while let Some(i) = it.prev_index() { std::hint::black_box(i); }
            Some(())
        }
    };
    (@idx $field:ident) => {
        // no index iteration
    };
    (@idx $field:ident, false) => {
        // no index iteration
    };

    // ── lookup_ops override (unchecked variants) ─────────────────────

    (@ops $ctx:ident, true) => {
        fn lookup_ops(&self, ctx: &$ctx) -> usize { ctx.hit_keys.len() }
    };
    (@ops $ctx:ident, false) => {
        // use trait default: ctx.lookup_keys.len()
    };
    (@ops $ctx:ident) => {
        // use trait default: ctx.lookup_keys.len()
    };
}

// keygen and results are in the lib crate
use tiny_trie_bench::keygen;
use tiny_trie_bench::results;

mod bit_trie;
mod dyn_trie;
mod fixed_len;
mod nibble_trie;
mod poly_trie;
mod std_contestants;
mod btree;

// ── Re-exports from modules ──────────────────────────────────────────

use keygen::*;
use results::*;

use bit_trie::BitTrieBench;
use dyn_trie::{DynTrieBench, DynTrieOptBench};
use fixed_len::{FixedLenBench, FixedLenOptBench};
use nibble_trie::{NibbleOptBench, NibbleOptUncheckedBench, NibbleTrieBench, NibbleUncheckedBench};
use poly_trie::{PolyOptBench, PolyTrieBench};
use std_contestants::{
    BTreeMapBench, BTreeMapBenchU64, HashMapBench, HashMapBenchU64,
    SortedVecBench, SortedVecBenchU64,
};
use btree::{IntBTreeBench, StrBTreeBench};

// ── Type aliases ─────────────────────────────────────────────────────

type NT = NibbleTrie<Vec<u8>, usize, u32, u32>;

// ── Config ───────────────────────────────────────────────────────────


// ── Allocation tracker ──────────────────────────────────────────────

#[global_allocator]
static TRACKER: TrackingAllocator = TrackingAllocator;

struct TrackingAllocator;
static ALLOCATED: AtomicU64 = AtomicU64::new(0);

unsafe impl std::alloc::GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        ALLOCATED.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}

pub(crate) fn read_allocated() -> u64 {
    ALLOCATED.load(Ordering::Relaxed)
}

// ── FixedLen helpers ────────────────────────────────────────────────

pub(crate) const FIXED_LEN_MAX: usize = 16;

pub(crate) fn truncate_key(key: &[u8]) -> Vec<u8> {
    if key.len() <= FIXED_LEN_MAX { key.to_vec() } else { key[..FIXED_LEN_MAX].to_vec() }
}

pub(crate) fn max_key_len(keys: &[Vec<u8>]) -> usize {
    keys.iter().map(|k| k.len().min(FIXED_LEN_MAX)).max().unwrap_or(1)
}

// ── Sorted-vec helpers ──────────────────────────────────────────────

pub(crate) fn build_sorted_vec<K: Ord + Clone>(keys: &[K]) -> Vec<(K, usize)> {
    let mut v: Vec<_> = keys.iter().enumerate().map(|(i, k)| (k.clone(), i)).collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

pub(crate) fn sorted_vec_get<K: Ord>(sv: &[(K, usize)], key: &K) -> Option<usize> {
    sv.binary_search_by(|e| e.0.cmp(key)).ok().map(|i| sv[i].1)
}

// ── BenchCtx ──────────────────────────────────────────────────────────

/// Shared key sets for lookup benchmarks — built once per size.
///
/// Generic over the contestant's native key type `K`. The byte-string
/// contestants use `BenchCtx<Vec<u8>>` (aliased `BenchContext`); the
/// fixed-width `u64` contestants use `BenchCtx<u64>`; the non-zero-byte
/// null-terminator tries use `BenchCtx<NonZeroBytes>` (aliased
/// `BenchContextNz`).
///
/// `lookup_keys_null` holds the null-terminated lookup needles (each key + a
/// trailing `0x00`) consumed by the macro's `get(null)` arm — the null-terminator
/// tries' `get` requires null-terminated input. Because those needles *contain*
/// `0x00`, they are `Vec<Vec<u8>>` regardless of `K` (a `NonZeroBytes` can't
/// carry a terminator). For byte ctx this is the same type as before; for the
/// `u64` ctx it is unused and empty.
///
/// `fl_lookup_keys` is byte-only (`FixedLenBench`'s truncated `get(truncated)`
/// arm) — `Vec<Vec<u8>>`, empty for non-byte ctxs.
pub(crate) struct BenchCtx<K> {
    pub lookup_keys: Vec<K>,
    pub lookup_keys_null: Vec<Vec<u8>>,
    pub fl_lookup_keys: Vec<Vec<u8>>,
    pub hit_keys: Vec<K>,
}

/// Byte-string context. `lookup_keys` interleaves each key with a miss
/// (`key + b'z'`, a longer key sharing the prefix) so lookup benches do `2n`
/// probes, half hits half misses. `lookup_keys_null` mirrors that with a
/// null terminator appended (for null-terminator trie `get(null)` arms).
/// `fl_lookup_keys` is the truncated (≤16-byte) projection of `lookup_keys`.
type BenchContext = BenchCtx<Vec<u8>>;

/// Non-zero-byte context for null-terminator tries (BitTrie, PolyTrie).
/// `lookup_keys` carries the `0x00`-free keys + misses (all `NonZeroBytes`,
/// `2n`); `lookup_keys_null` carries the null-terminated needles (`Vec<u8>`,
/// `2n`) that the `get(null)` macro arm feeds to `get`. `hit_keys`/`fl_lookup_keys`
/// are unused (these contestants use `get(null)`, not `get_unchecked`/`get(truncated)`).
type BenchContextNz = BenchCtx<NonZeroBytes>;

fn build_context(keys: &[Vec<u8>]) -> BenchContext {
    let mut lookup_keys = Vec::with_capacity(keys.len() * 2);
    let mut lookup_keys_null = Vec::with_capacity(keys.len() * 2);
    for k in keys {
        lookup_keys.push(k.clone());
        let mut nt = k.clone();
        nt.push(0);
        lookup_keys_null.push(nt);
        let mut miss = k.clone();
        miss.push(b'z');
        lookup_keys.push(miss.clone());
        miss.push(0);
        lookup_keys_null.push(miss);
    }
    let fl_lookup_keys: Vec<Vec<u8>> = lookup_keys.iter().map(|k| truncate_key(k)).collect();
    BenchContext {
        hit_keys: keys.to_vec(),
        lookup_keys,
        lookup_keys_null,
        fl_lookup_keys,
    }
}

/// Non-zero-byte context: same `2n` hit+miss shape as the byte ctx so the
/// `get(null)` lookup bench does `2n` probes (matching the byte contestants'
/// op count for a fair comparison). `lookup_keys` = `[key, key+'z']` interleaved
/// as `NonZeroBytes` (`'z'` is non-zero, so the miss is still `0x00`-free);
/// `lookup_keys_null` = `[key+0x00, (key+'z')+0x00]` as `Vec<u8>` (the trailing
/// `0x00` is the null terminator `get` requires). `hit_keys`/`fl_lookup_keys`
/// unused, left empty.
fn build_context_nonzero(keys: &[NonZeroBytes]) -> BenchContextNz {
    let mut lookup_keys = Vec::with_capacity(keys.len() * 2);
    let mut lookup_keys_null = Vec::with_capacity(keys.len() * 2);
    for k in keys {
        lookup_keys.push(k.clone());
        let mut nt = k.as_bytes().to_vec();
        nt.push(0);
        lookup_keys_null.push(nt);
        let mut miss = k.as_bytes().to_vec();
        miss.push(b'z');
        lookup_keys.push(NonZeroBytes::new(miss.clone()).expect("key+'z' is non-zero"));
        miss.push(0);
        lookup_keys_null.push(miss);
    }
    BenchContextNz {
        hit_keys: Vec::new(),
        lookup_keys,
        lookup_keys_null,
        fl_lookup_keys: Vec::new(),
    }
}

/// Fixed-width `u64` context. Mirrors the byte ctx's hit+miss interleaving so
/// `IntBTreeFixedBench` performs the same `2n` lookups (half hit, half miss) as
/// the byte CTree — a fair cross-contestant comparison. The miss is the bitwise
/// complement `!v`, a guaranteed-absent `u64` for both `RandomU64` (collision
/// with another key is ~n/2^64) and `SeqU64` (complement of a small `i` is
/// ~2^64, far outside `[0, n)`). `lookup_keys_null`/`hit_keys`/`fl_lookup_keys`
/// are unused by the `u64` contestants (which only read `lookup_keys`) and stay
/// empty.
fn build_context_u64(keys: &[u64]) -> BenchCtx<u64> {
    let mut lookup_keys = Vec::with_capacity(keys.len() * 2);
    for &v in keys {
        lookup_keys.push(v);
        lookup_keys.push(!v);
    }
    BenchCtx {
        hit_keys: Vec::new(),
        lookup_keys,
        lookup_keys_null: Vec::new(),
        fl_lookup_keys: Vec::new(),
    }
}

// ── Benchable trait ─────────────────────────────────────────────────

/// A benchmark contestant, generic over its native key type `K`.
///
/// `K` is never erased: a contestant is `Benchable<K>` for a concrete `K`
/// (`Vec<u8>` for byte-string trees, `NonZeroBytes` for null-terminator tries,
/// `u64` for fixed-width), so the contestant's operations are monomorphized for
/// that `K` — e.g. `CTree<u64>::get` inlines its SIMD `find_position`. The
/// harness groups heterogeneous contestants by `K` via the `Bench` enum, which
/// only erases the *contestant type* (not `K`) through a `dyn Benchable<K>`
/// vtable.
///
/// Which `K`s a contestant supports is structural (it carries `Option<Box<dyn Benchable<K>>>`
/// for each `K`), and which *modes* it runs in is structural too (`variant_for`): a contestant
/// runs only in modes where it has a matching key variant — no `u64`-as-byte-string
/// projection, no runtime domain check.
pub(crate) trait Benchable<K: Clone + 'static> {
    /// Populate internal state from keys. Called once per size before query benches.
    fn build(&mut self, _keys: &[K], _ctx: &BenchCtx<K>) {}

    fn bench_insert(&self, _keys: &[K]) -> Option<()> { None }
    fn bench_lookup(&self, _ctx: &BenchCtx<K>) -> Option<()> { None }
    fn bench_fwd_iter(&self) -> Option<()> { None }
    fn bench_rev_iter(&self) -> Option<()> { None }
    fn bench_fwd_idx(&self) -> Option<()> { None }
    fn bench_rev_idx(&self) -> Option<()> { None }
    fn bench_optimize(&self, _keys: &[K]) -> Option<()> { None }
    fn bench_memory(&self, _keys: &[K]) -> Option<f64> { None }

    /// Number of lookup operations — overridden by unchecked variants.
    fn lookup_ops(&self, ctx: &BenchCtx<K>) -> usize { ctx.lookup_keys.len() }
}


// ── Key variant dispatch ──────────────────────────────────────────────

/// Which key type a contestant uses for a given key mode. A contestant may
/// carry multiple variants (e.g. `CTree` has both `Bytes` and `U64`);
/// `variant_for(mode)` selects the one appropriate for the current mode.
#[derive(Clone, Copy)]
enum KeyVariant {
    Bytes,
    NonZero,
    U64,
}

// ── Contestant ────────────────────────────────────────────────────────

struct Contestant {
    name: &'static str,
    /// Skip this contestant for sizes larger than this (None = no limit).
    max_size: Option<usize>,
    bytes: Option<Box<dyn Benchable<Vec<u8>>>>,
    nonzero: Option<Box<dyn Benchable<NonZeroBytes>>>,
    u64: Option<Box<dyn Benchable<u64>>>,
}

/// Per-size typed key sets + contexts, one per supported `K`. The harness
/// builds these up front and hands `&Keys` to each `Contestant` dispatch method,
/// which selects the slice/ctx matching its `KeyVariant`. The `u64` set is
/// populated only in fixed-width modes (empty otherwise); the `nonzero` set
/// only in `0x00`-free modes (empty otherwise). Contestants without a matching
/// variant are skipped via `variant_for`, so they never read the empty sets.
struct Keys {
    bytes: Vec<Vec<u8>>,
    nonzero: Vec<NonZeroBytes>,
    u64: Vec<u64>,
    ctx_b: BenchCtx<Vec<u8>>,
    ctx_nz: BenchCtx<NonZeroBytes>,
    ctx_u: BenchCtx<u64>,
}

impl Contestant {
    /// Select the key variant appropriate for `mode`. Returns `None` if this
    /// contestant has no variant compatible with the mode (e.g. a bytes-only
    /// contestant in a u64 mode).
    fn variant_for(&self, mode: KeyMode) -> Option<KeyVariant> {
        if mode.is_fixed_width() {
            self.u64.as_ref().map(|_| KeyVariant::U64)
        } else if mode.may_contain_null_bytes() {
            self.bytes.as_ref().map(|_| KeyVariant::Bytes)
        } else {
            // Sequential, Words, Lines — bytes preferred, nonzero fallback
            if self.bytes.is_some() { Some(KeyVariant::Bytes) }
            else if self.nonzero.is_some() { Some(KeyVariant::NonZero) }
            else { None }
        }
    }

    fn build(&mut self, k: &Keys, mode: KeyMode) {
        match self.variant_for(mode).expect("skipped contestants should not be built") {
            KeyVariant::Bytes => self.bytes.as_mut().unwrap().build(&k.bytes, &k.ctx_b),
            KeyVariant::NonZero => self.nonzero.as_mut().unwrap().build(&k.nonzero, &k.ctx_nz),
            KeyVariant::U64 => self.u64.as_mut().unwrap().build(&k.u64, &k.ctx_u),
        }
    }
    fn bench_insert(&self, k: &Keys, mode: KeyMode) -> Option<()> {
        match self.variant_for(mode)? {
            KeyVariant::Bytes => self.bytes.as_ref()?.bench_insert(&k.bytes),
            KeyVariant::NonZero => self.nonzero.as_ref()?.bench_insert(&k.nonzero),
            KeyVariant::U64 => self.u64.as_ref()?.bench_insert(&k.u64),
        }
    }
    fn bench_lookup(&self, k: &Keys, mode: KeyMode) -> Option<()> {
        match self.variant_for(mode)? {
            KeyVariant::Bytes => self.bytes.as_ref()?.bench_lookup(&k.ctx_b),
            KeyVariant::NonZero => self.nonzero.as_ref()?.bench_lookup(&k.ctx_nz),
            KeyVariant::U64 => self.u64.as_ref()?.bench_lookup(&k.ctx_u),
        }
    }
    fn lookup_ops(&self, k: &Keys, mode: KeyMode) -> usize {
        match self.variant_for(mode).expect("skipped contestants should not call lookup_ops") {
            KeyVariant::Bytes => self.bytes.as_ref().unwrap().lookup_ops(&k.ctx_b),
            KeyVariant::NonZero => self.nonzero.as_ref().unwrap().lookup_ops(&k.ctx_nz),
            KeyVariant::U64 => self.u64.as_ref().unwrap().lookup_ops(&k.ctx_u),
        }
    }
    fn bench_optimize(&self, k: &Keys, mode: KeyMode) -> Option<()> {
        match self.variant_for(mode)? {
            KeyVariant::Bytes => self.bytes.as_ref()?.bench_optimize(&k.bytes),
            KeyVariant::NonZero => self.nonzero.as_ref()?.bench_optimize(&k.nonzero),
            KeyVariant::U64 => self.u64.as_ref()?.bench_optimize(&k.u64),
        }
    }
    fn bench_memory(&self, k: &Keys, mode: KeyMode) -> Option<f64> {
        match self.variant_for(mode)? {
            KeyVariant::Bytes => self.bytes.as_ref()?.bench_memory(&k.bytes),
            KeyVariant::NonZero => self.nonzero.as_ref()?.bench_memory(&k.nonzero),
            KeyVariant::U64 => self.u64.as_ref()?.bench_memory(&k.u64),
        }
    }
    // No-key bench methods: dispatch by variant alone.
    fn bench_fwd_iter(&self, mode: KeyMode) -> Option<()> {
        match self.variant_for(mode)? {
            KeyVariant::Bytes => self.bytes.as_ref()?.bench_fwd_iter(),
            KeyVariant::NonZero => self.nonzero.as_ref()?.bench_fwd_iter(),
            KeyVariant::U64 => self.u64.as_ref()?.bench_fwd_iter(),
        }
    }
    fn bench_rev_iter(&self, mode: KeyMode) -> Option<()> {
        match self.variant_for(mode)? {
            KeyVariant::Bytes => self.bytes.as_ref()?.bench_rev_iter(),
            KeyVariant::NonZero => self.nonzero.as_ref()?.bench_rev_iter(),
            KeyVariant::U64 => self.u64.as_ref()?.bench_rev_iter(),
        }
    }
    fn bench_fwd_idx(&self, mode: KeyMode) -> Option<()> {
        match self.variant_for(mode)? {
            KeyVariant::Bytes => self.bytes.as_ref()?.bench_fwd_idx(),
            KeyVariant::NonZero => self.nonzero.as_ref()?.bench_fwd_idx(),
            KeyVariant::U64 => self.u64.as_ref()?.bench_fwd_idx(),
        }
    }
    fn bench_rev_idx(&self, mode: KeyMode) -> Option<()> {
        match self.variant_for(mode)? {
            KeyVariant::Bytes => self.bytes.as_ref()?.bench_rev_idx(),
            KeyVariant::NonZero => self.nonzero.as_ref()?.bench_rev_idx(),
            KeyVariant::U64 => self.u64.as_ref()?.bench_rev_idx(),
        }
    }
}

/// Whether a contestant should run at the given size.
fn runnable(c: &Contestant, i: usize, active: &[bool], size: usize) -> bool {
    active[i] && c.max_size.map_or(true, |m| size <= m)
}

fn all_contestants() -> Vec<Contestant> {
    vec![
        Contestant { name: "NibbleTrie",        max_size: None, bytes: Some(Box::new(NibbleTrieBench::new())),  nonzero: None, u64: None },
        Contestant { name: "BitTrie",            max_size: None, bytes: None, nonzero: Some(Box::new(BitTrieBench::new())), u64: None },
        Contestant { name: "BTreeMap",           max_size: None, bytes: Some(Box::new(BTreeMapBench::new())),   nonzero: None, u64: Some(Box::new(BTreeMapBenchU64::new())) },
        Contestant { name: "HashMap",            max_size: None, bytes: Some(Box::new(HashMapBench::new())),    nonzero: None, u64: Some(Box::new(HashMapBenchU64::new())) },
        Contestant { name: "SortedVec",          max_size: None, bytes: Some(Box::new(SortedVecBench::new())), nonzero: None, u64: Some(Box::new(SortedVecBenchU64::new())) },
        Contestant { name: "NibbleOpt",         max_size: None, bytes: Some(Box::new(NibbleOptBench::new())), nonzero: None, u64: None },
        Contestant { name: "NibbleUnchecked",    max_size: None, bytes: Some(Box::new(NibbleUncheckedBench::new())), nonzero: None, u64: None },
        Contestant { name: "NibbleOptUnchecked", max_size: None, bytes: Some(Box::new(NibbleOptUncheckedBench::new())), nonzero: None, u64: None },
        Contestant { name: "DynTrie",            max_size: None, bytes: Some(Box::new(DynTrieBench::new())),    nonzero: None, u64: None },
        Contestant { name: "DynTrieOpt",         max_size: None, bytes: Some(Box::new(DynTrieOptBench::new())), nonzero: None, u64: None },
        Contestant { name: "PolyTrie",           max_size: None, bytes: None, nonzero: Some(Box::new(PolyTrieBench::new())), u64: None },
        Contestant { name: "PolyOpt",            max_size: None, bytes: None, nonzero: Some(Box::new(PolyOptBench::new())), u64: None },
        Contestant { name: "FixedLen",            max_size: None, bytes: Some(Box::new(FixedLenBench::new())),  nonzero: None, u64: None },
        Contestant { name: "FixedLenOpt",         max_size: None, bytes: Some(Box::new(FixedLenOptBench::new())), nonzero: None, u64: None },
        Contestant { name: "IntBTree",      max_size: None, bytes: None, nonzero: None, u64: Some(Box::new(IntBTreeBench::new()))    },
        // Contestant { name: "IntBTreeOpt",   max_size: None, bytes: None, nonzero: None, u64: Some(Box::new(IntBTreeOptBench::new())) },
        Contestant { name: "StrBTree",      max_size: None, bytes: Some(Box::new(StrBTreeBench::new())),     nonzero: None, u64: None },
    ]
}

// ── Bench harness ─────────────────────────────────────────────────────

struct BenchResult {
    iters: u64,
    elapsed: Duration,
}

impl BenchResult {
    fn rate(&self, ops_per_iter: u64) -> f64 {
        (self.iters * ops_per_iter) as f64 / self.elapsed.as_secs_f64()
    }
}

/// Run `f` repeatedly until `budget` has elapsed, counting iterations.
/// Returns `None` if the first call returns `None` (unsupported test).
fn bench(budget: Duration, label: &str, f: impl Fn() -> Option<()>) -> Option<BenchResult> {
    let mut iters = 0u64;
    let start = Instant::now();
    loop {
        f()?;
        iters += 1;
        if start.elapsed() >= budget {
            break;
        }
    }
    let elapsed = start.elapsed();
    let per = elapsed.as_secs_f64() / iters as f64;
    if per >= 1.0 {
        eprintln!("    {label}: {iters} iters in {:.2}s ({:.2}s/iter) ✓", elapsed.as_secs_f64(), per);
    } else if per >= 0.001 {
        eprintln!("    {label}: {iters} iters in {:.2}s ({:.1}ms/iter) ✓", elapsed.as_secs_f64(), per * 1000.0);
    } else {
        eprintln!("    {label}: {iters} iters in {:.2}s ({:.1}µs/iter) ✓", elapsed.as_secs_f64(), per * 1e6);
    }
    Some(BenchResult { iters, elapsed })
}

// ── CLI ───────────────────────────────────────────────────────────────

const ALL_TESTS: &[&str] = &["insert", "lookup", "fwd", "rev", "fwd_idx", "rev_idx", "optimize", "memory"];

#[derive(Parser)]
#[command(name = "bencher", bin_name = "bencher")]
#[command(about = "TinyTrie Benchmark Suite")]
#[command(version)]
struct Cli {
    #[arg(long, short, value_delimiter = ',', default_values = ALL_TESTS)]
    tests: Vec<String>,
    #[arg(long, short)]
    sizes: Option<String>,
    #[arg(long, value_delimiter = ',')]
    structures: Option<Vec<String>>,
    #[arg(long, default_value = "sequential")]
    keys: KeyMode,
    #[arg(long)]
    corpus: Option<String>,
    #[arg(long, default_value_t = 2)]
    time: u64,
}

fn resolve_tests(raw: &[String]) -> Vec<String> {
    let mut resolved = Vec::new();
    let mut unknown = Vec::new();
    for s in raw {
        let lower = s.to_ascii_lowercase();
        let normalized = match lower.as_str() {
            "insert" | "insertion" => "insert",
            "lookup" => "lookup",
            "fwd" | "forward" => "fwd",
            "rev" | "backward" => "rev",
            "fwd_idx" | "forward_idx" | "forward_index" => "fwd_idx",
            "rev_idx" | "backward_idx" | "backward_index" | "rev_index" => "rev_idx",
            "optimize" | "opt" => "optimize",
            "memory" | "mem" => "memory",
            other => { unknown.push(other.to_string()); continue; }
        };
        if !resolved.iter().any(|t| t == normalized) {
            resolved.push(normalized.to_string());
        }
    }
    if !unknown.is_empty() {
        eprintln!("Error: unknown test(s): {}. Valid tests: {}", unknown.join(", "), ALL_TESTS.join(", "));
        std::process::exit(1);
    }
    resolved
}

// ── Main ──────────────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();

    if matches!(cli.keys, KeyMode::Words | KeyMode::Lines) && cli.corpus.is_none() {
        eprintln!("Error: --keys={} requires --corpus <file>",
            match cli.keys { KeyMode::Words => "words", KeyMode::Lines => "lines", _ => unreachable!() });
        std::process::exit(1);
    }
    if matches!(cli.keys, KeyMode::RandomU64 | KeyMode::SeqU64) && cli.corpus.is_some() {
        eprintln!("Warning: --keys={}? ignores --corpus",
            match cli.keys { KeyMode::RandomU64 => "random-u64", KeyMode::SeqU64 => "seq-u64", _ => unreachable!() });
    }

    let tests = resolve_tests(&cli.tests);
    let mut sizes = resolve_sizes(cli.sizes.as_deref());
    let struct_filters: Vec<String> = match &cli.structures {
        Some(s) if !s.is_empty() => s.iter().map(|f| f.to_ascii_lowercase()).collect(),
        _ => Vec::new(),
    };
    let key_mode = cli.keys;
    let corpus_path = cli.corpus;
    let bench_secs = cli.time;

    let corpus_keys: Option<Vec<Vec<u8>>> = match key_mode {
        KeyMode::Words => {
            let path = corpus_path.as_deref().expect("--keys=words requires --corpus <file>");
            let all = load_corpus_words(path);
            eprintln!("Corpus: {} unique words from {}", all.len(), path);
            Some(all)
        }
        KeyMode::Lines => {
            let path = corpus_path.as_deref().expect("--keys=lines requires --corpus <file>");
            let all = load_corpus_lines(path);
            eprintln!("Corpus: {} unique lines from {}", all.len(), path);
            Some(all)
        }
        _ => None,
    };
    if let Some(ref all) = corpus_keys {
        let max_n = all.len();
        let before = sizes.len();
        sizes.retain(|&s| s <= max_n);
        if sizes.len() < before {
            let skipped: Vec<usize> = resolve_sizes(cli.sizes.as_deref())
                .into_iter().filter(|&s| s > max_n).collect();
            eprintln!("Skipping sizes {:?}: corpus has only {} entries", skipped, max_n);
            if sizes.is_empty() {
                eprintln!("Error: no sizes to benchmark (corpus has only {} entries, all requested sizes exceed it)", max_n);
                std::process::exit(1);
            }
        }
    }

    let mut contestants = all_contestants();
    let active: Vec<bool> = contestants.iter().map(|c| {
        if struct_filters.is_empty() { true } else { struct_filters.iter().any(|f| c.name.to_ascii_lowercase().contains(f)) }
    }).collect();
    if active.iter().all(|a| !a) {
        let names: Vec<&str> = contestants.iter().map(|c| c.name).collect();
        eprintln!("No structures match filters {:?}. Available: {}", struct_filters, names.join(", "));
        std::process::exit(1);
    }

    let budget = Duration::from_secs(bench_secs);
    let names: Vec<&str> = contestants.iter().map(|c| c.name).collect();

    let run_insert   = tests.iter().any(|t| t == "insert");
    let run_lookup    = tests.iter().any(|t| t == "lookup");
    let run_fwd       = tests.iter().any(|t| t == "fwd");
    let run_rev       = tests.iter().any(|t| t == "rev");
    let run_fwd_idx   = tests.iter().any(|t| t == "fwd_idx");
    let run_rev_idx   = tests.iter().any(|t| t == "rev_idx");
    let run_optimize  = tests.iter().any(|t| t == "optimize");
    let run_memory    = tests.iter().any(|t| t == "memory");

    println!();
    println!("=== TinyTrie Benchmark Suite ===");
    {
        let active_names: Vec<&str> = names.iter().zip(active.iter()).filter(|(_, a)| **a).map(|(n, _)| *n).collect();
        eprintln!("Tests:    {}", tests.join(", "));
        eprintln!("Sizes:    {}", sizes.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(", "));
        eprintln!("Structs:  {}", if struct_filters.is_empty() { "all".to_string() } else { active_names.join(", ") });
        eprintln!("Keys:     {:?}", key_mode);
    }
    println!("{bench_secs}s per bench · sequential per size");
    println!();

    let mut ins  = ResultMap::new();
    let mut look = ResultMap::new();
    let mut fwd  = ResultMap::new();
    let mut rev  = ResultMap::new();
    let mut fwd_idx = ResultMap::new();
    let mut rev_idx = ResultMap::new();
    let mut mem  = ResultMap::new();
    let mut opt  = ResultMap::new();

    let (json_path, md_path) = results_paths(&key_mode);
    let mut results = load_results(&json_path);
    for &sz in &sizes {
        if !results.sizes.contains(&sz) {
            results.sizes.push(sz);
        }
    }
    results.sizes.sort();

    let needs_structures = run_lookup || run_fwd || run_rev || run_fwd_idx || run_rev_idx;

    // Pre-compute which contestants are incompatible with this key mode. A
    // contestant runs only if it has a variant whose key type matches the mode
    // (`variant_for`): bytes-only contestants sit out fixed-width u64 modes;
    // u64-only contestants sit out variable-length modes; nonzero-only contestants
    // sit out modes that may emit `0x00`.
    let skip_for_keys: Vec<bool> = contestants.iter()
        .map(|c| c.variant_for(key_mode).is_none())
        .collect();

    for &size in &sizes {
        eprintln!("[n = {size}]");

        eprint!("  generating keys ({:?})... ", key_mode);
        // Byte-string keys are produced only for non-`u64` modes — the byte/trie
        // contestants skip fixed-width modes, so no `u64`-as-byte-string projection
        // is generated. The `u64` set (below) is the only key set in `u64` modes.
        let keys_bytes = if key_mode.is_fixed_width() {
            Vec::new()
        } else {
            generate_keys(&key_mode, size, corpus_keys.as_deref())
        };
        eprintln!("✓ ({} byte keys, {} u64 keys)",
            keys_bytes.len(),
            if key_mode.is_fixed_width() { size } else { 0 });

        // Typed key sets + contexts, one per supported `K`. The `u64` set is
        // populated only in fixed-width modes (empty otherwise); the `nonzero`
        // set only in `0x00`-free modes (empty otherwise). `U64`/`NonZero`
        // contestants are skipped outside their modes, so the empty sets are
        // never read. All ctxs are built up front so `Contestant::build` and the
        // query benches can borrow the matching one via `&Keys`.
        let keys_u64 = if key_mode.is_fixed_width() {
            generate_keys_u64(&key_mode, size)
        } else {
            Vec::new()
        };
        let keys_nz = generate_keys_nonzero(&key_mode, size, corpus_keys.as_deref());
        let keys = Keys {
            ctx_b: build_context(&keys_bytes),
            ctx_nz: build_context_nonzero(&keys_nz),
            ctx_u: build_context_u64(&keys_u64),
            bytes: keys_bytes,
            nonzero: keys_nz,
            u64: keys_u64,
        };

        // Announce skipped contestants for incompatible key modes
        for (i, c) in contestants.iter().enumerate() {
            if skip_for_keys[i] && runnable(c, i, &active, size) {
                eprintln!("  {}: skipped (incompatible key mode {:?})", c.name, key_mode);
            }
        }

        // ── Insertion ──────────────────────────────────────────────────
        if run_insert {
            eprintln!("  insertion:");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                if let Some(r) = bench(budget, c.name, || c.bench_insert(&keys, key_mode)) {
                    ins.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Build structures for lookup / iteration ─────────────────────
        if needs_structures {
            eprint!("  building structures... ");
            let t0 = Instant::now();
            for (i, c) in contestants.iter_mut().enumerate() {
                if skip_for_keys[i] || !runnable(c, i, &active, size) { continue; }
                c.build(&keys, key_mode);
            }
            eprintln!("{:.2}s ✓", t0.elapsed().as_secs_f64());
        }

        // ── Lookup ───────────────────────────────────────────────────
        if run_lookup {
            eprintln!("  lookup:");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                let ops = c.lookup_ops(&keys, key_mode);
                if let Some(r) = bench(budget, c.name, || c.bench_lookup(&keys, key_mode)) {
                    look.entry(c.name.into()).or_default().push(r.rate(ops as u64));
                }
            }
        }

        // ── Forward iteration ─────────────────────────────────────────
        if run_fwd {
            eprintln!("  iteration (forward):");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                if let Some(r) = bench(budget, c.name, || c.bench_fwd_iter(key_mode)) {
                    fwd.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Backward iteration ───────────────────────────────────────
        if run_rev {
            eprintln!("  iteration (backward):");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                if let Some(r) = bench(budget, c.name, || c.bench_rev_iter(key_mode)) {
                    rev.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Forward index iteration ──────────────────────────────────
        if run_fwd_idx {
            eprintln!("  iteration (forward index):");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                if let Some(r) = bench(budget, c.name, || c.bench_fwd_idx(key_mode)) {
                    fwd_idx.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Backward index iteration ──────────────────────────────────
        if run_rev_idx {
            eprintln!("  iteration (backward index):");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                if let Some(r) = bench(budget, c.name, || c.bench_rev_idx(key_mode)) {
                    rev_idx.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Optimize time ─────────────────────────────────────────────
        if run_optimize {
            eprintln!("  optimize:");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                if let Some(r) = bench(budget, c.name, || c.bench_optimize(&keys, key_mode)) {
                    opt.entry(c.name.into()).or_default().push(r.rate(size as u64));
                }
            }
        }

        // ── Memory ────────────────────────────────────────────────────
        if run_memory {
            eprintln!("  memory:");
            for (i, c) in contestants.iter().enumerate() {
                if !runnable(c, i, &active, size) || skip_for_keys[i] { continue; }
                if let Some(bytes_per_key) = c.bench_memory(&keys, key_mode) {
                    eprintln!("    {}: {:.1}/key", c.name, bytes_per_key);
                    mem.entry(c.name.into()).or_default().push(bytes_per_key);
                }
            }
        }

        eprintln!();
    }

    // ── Print summary tables ────────────────────────────────────────
    if run_insert   { print_table("Insertion", "keys/sec", &ins, &sizes, &names); }
    if run_lookup    { print_table("Lookup", "keys/sec", &look, &sizes, &names); }
    if run_fwd       { print_table("Iter forward", "keys/sec", &fwd, &sizes, &names); }
    if run_rev       { print_table("Iter backward", "keys/sec", &rev, &sizes, &names); }
    if run_fwd_idx   { print_table("Iter fwd index", "keys/sec", &fwd_idx, &sizes, &names); }
    if run_rev_idx   { print_table("Iter rev index", "keys/sec", &rev_idx, &sizes, &names); }
    if run_optimize  { print_table("Optimize", "keys/sec", &opt, &sizes, &names); }
    if run_memory    { print_mem_table(&mem, &sizes, &names); }

    // ── Merge and save results ───────────────────────────────────────
    if run_insert   { merge_results(&mut results, "Insertion (keys/sec)",  &ins, &sizes); }
    if run_lookup   { merge_results(&mut results, "Lookup (keys/sec)",     &look, &sizes); }
    if run_fwd      { merge_results(&mut results, "Iter forward (keys/sec)",  &fwd, &sizes); }
    if run_rev      { merge_results(&mut results, "Iter backward (keys/sec)", &rev, &sizes); }
    if run_fwd_idx  { merge_results(&mut results, "Iter fwd index (keys/sec)", &fwd_idx, &sizes); }
    if run_rev_idx  { merge_results(&mut results, "Iter rev index (keys/sec)", &rev_idx, &sizes); }
    if run_optimize { merge_results(&mut results, "Optimize (keys/sec)",  &opt, &sizes); }
    if run_memory   { merge_results(&mut results, "Memory (bytes/key)",   &mem, &sizes); }
    save_results(&results, &json_path, &md_path);

    println!();
}