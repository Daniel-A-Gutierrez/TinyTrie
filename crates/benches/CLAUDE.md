# crates/benches

Benchmark harness for the workspace's trie / map structures.

## Layout

- Binary `bencher` = `src/main.rs`: the harness, the `bench_query_methods!` macro, and one `mod` per contestant (`bit_trie.rs`, `dyn_trie.rs`, `fixed_len.rs`, `nibble_trie.rs`, `poly_trie.rs`, `std_contestants.rs`, `btree.rs`).
- Lib `tiny_trie_bench` = `src/lib.rs` re-exports `benchable_map`, `keygen`, `results`. `benchable_map.rs` owns the `BenchableMap` trait + `NonZeroBytes` type + the `impl BenchableMap for {NibbleTrie, NibTrie, BitTrie, FixedLenNibbleTrie, PolyTrie}` blocks (orphan rule: the trait lives here, so impls for foreign trie types are legal here; this keeps `tiny-trie`/`poly-trie`/`btrees` free of bench-only deps).
- **Import gotcha**: contestant modules live in the *binary* and import shared items via `use tiny_trie_bench::BenchableMap;` / `use tiny_trie_bench::NonZeroBytes;` — NOT `crate::` (which resolves to the `bencher` binary, not the lib). `keygen`/`results` are also accessed via `use tiny_trie_bench::keygen;`.

## Dispatch model

A `Contestant` is a `struct` with three `Option<Box<dyn Benchable<K>>>` fields — `bytes` (`K=Vec<u8>`), `nonzero` (`K=NonZeroBytes`), `u64` (`K=u64`). `variant_for(mode)` picks the field for the current `KeyMode`, returning `None` when no variant matches; the harness computes `skip_for_keys = variant_for(mode).is_none()` and skips. **There is no `Bench` enum and no `skip_for` method** (any comment referencing `Bench::NonZero::skip_for` is stale). To add a contestant: append a `Contestant{ name, max_size, bytes, nonzero, u64 }` to `all_contestants()` in `main.rs`, filling only the variant(s) matching its native key type (e.g. a u64-native CTree gets only `u64`; a byte trie gets only `bytes`; a generic std container gets both `bytes` and `u64`).

`K` is never erased: `Box<dyn Benchable<K>>` only vtables the *contestant type*, so each contestant's tree ops monomorphize for its concrete `K` (this is why `IntBTreeBenchGen<u64>` exercises the SIMD `find_position` path natively, not via a `Vec<u8>` decode shim).

**Adding a new key type is a wide change** — avoid if an existing `K` fits. Touch points: a `KeyVariant` arm, a field on `Contestant` + `Keys`, a `BenchCtx<K>` builder (`build_context_*`), dispatch arms in every `Contestant` method, a key-set generator in `keygen.rs`, and a `KeyMode` membership predicate (`is_fixed_width` / `may_contain_null_bytes`).

## `BenchCtx<K>` fairness invariants (silent if broken)

- `lookup_keys` must be `2n` probes — each key interleaved with a miss — for *every* `K`. Byte miss = `key + b'z'`; u64 miss = bitwise complement `!v`. A new key type that does fewer lookups makes cross-contestant lookup tables unfair.
- `lookup_keys_null: Vec<Vec<u8>>` is NOT `Vec<K>` — the null-terminated needles contain `0x00` and can't be `NonZeroBytes`. Keep it `Vec<Vec<u8>>` on `BenchCtx<K>` for all `K`.
- `fl_lookup_keys: Vec<Vec<u8>>` stays a field on `BenchCtx<K>` (empty for non-byte ctx) so the macro's `get(truncated)` arm needs no edit. Do NOT move it into `FixedLenBench::build`.

## `bench_query_methods!` macro

Generates ONLY the query methods (`bench_lookup`, `bench_fwd_iter`, `bench_rev_iter`, `bench_fwd_idx`, `bench_rev_idx`, `lookup_ops`). The struct, `build`, `bench_insert`, `bench_optimize`, `bench_memory` are hand-written per contestant.

- **Placement**: must be defined in `main.rs` BEFORE the `mod <contestant>;` declarations. `#[macro_export]` does not work for binary-crate macros — submodules can't find it. Plain `macro_rules!` above the `mod` lines.
- **Required `ctx: $ctx:ident` param**: `BenchContext` (= `BenchCtx<Vec<u8>>`) for byte contestants, `BenchContextNz` (= `BenchCtx<NonZeroBytes>`) for null-terminator contestants. It threads the ctx type into the `@lookup`/`@ops` arms so generated signatures match `Benchable<K>`.
- **`lookup` spec**: `<method>(<key_set>)`. Methods: `map_get` (the `BenchableMap` trait), `get` (inherent), `get_unchecked` (gated by tiny-trie's `unchecked` feature). Key-sets: `lookup`, `null` (null-terminated, uses `lookup_keys_null`), `truncated` (uses `fl_lookup_keys`), `hit` (uses `hit_keys`).
- **`unchecked: true`** overrides `lookup_ops` to return `ctx.hit_keys.len()` (else default `ctx.lookup_keys.len()`).
- **fwd/rev iter styles**: `trie_callback` (`map_iter_fwd`/`map_iter_rev`), `dyn_callback` (`iter_fwd`/`iter_rev`), `iter_kv` (`iter()` + `current()` + `next()`/`prev()`), `iter_kv_no_current` (`next()`/`prev()` only), `none` (skip). `index_iter: true` adds `bench_fwd_idx`/`bench_rev_idx` via `current_index`/`next_index`/`prev_index`.

## Null-terminator tries (BitTrie, PolyTrie)

Native key type is `NonZeroBytes` (a bytestring guaranteed free of `0x00`). `keygen::generate_keys_nonzero` returns empty for null-byte modes (`Random`/`RandomU64`/`SeqU64`), so these contestants are skipped *by construction* (no keys to build on) — no runtime domain check. `insert` rejects `0x00`; `get` requires null-terminated input, which the macro's `get(null)` arm supplies via `ctx.lookup_keys_null`.

## CLI

Run: `cargo run --release --bin bencher -- --keys=<mode> --sizes N --time T --tests ... --structures ...`.
- **`-t` is `--tests`** (clap derives the short from the field name `tests`); `--time` has NO short flag (would collide). `--sizes` short is `-s`.
- `--sizes`: comma list or `lo..hi` range, filtered against the canonical `SIZES = [100, 10_000, 1_000_000]` (in `keygen.rs`) — values outside this set are rejected.
- `--keys` default `Sequential`; `Random`/`RandomU64`/`SeqU64` need no corpus; `Words`/`Lines` require `--corpus <file>` (sizes beyond the corpus are dropped).
- Results persist to `benches/bench_results_<suffix>.{json,md}` (`.json` gitignored). `save_results` merges by size into existing JSON; a smoke run overwrites the `.md` — restore with `git checkout -- benches/bench_results_<suffix>.md`.