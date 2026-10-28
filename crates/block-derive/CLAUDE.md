# block-derive — Archive/RelOrd derives

Proc-macro crate generating block's `Archive` + `RelOrd` impls for named-field
structs with concrete field types (tuples/generics/enums are compile_error'd;
generic-struct-with-generic-fields is future work). The traits it targets are
FROZEN in block — `crates/block/CLAUDE.md` carries the contract map.

Generated shape per `#[derive(Archive)]` (a `#[derive(RelOrd)]` complements
it in the same crate):

- `struct XHandle<P> { per-field `<F as Archive<P>>::Handle` projects, ... }` —
  Copy + Eq + Debug; PLUS: `PhantomData<[P; 0]>` marker (zero-size at every
  width; the old width-sized marker re-poisoned composite tails), `#[repr(C)]`
  (LOAD-BEARING: repr(Rust) reordered explicit pads into wrong record
  positions), explicit `_pad_k` fields from the layout planner, and a
  ZERO-COST inline-const layout canary inside archive() — it evaluates
  per-monomorphization, so a width whose layout would carry implicit padding
  is a compile error, not a dirty record. Structs with unclassifiable fields
  (manual/nested impls) get NEITHER pads NOR the canary — known hole.
- archive = snapshot → per-field archive → rewind on any Err (atomic: heap
  unchanged on failure). release = per-field release, reverse declaration
  order. Comparison order = declaration order (field-wise, short-circuiting).

Known quirks (don't re-derive these — see block-derive docs + tests):
- derive(Debug) breaks on projected-handle bounds (rustc quirk) → Debug is
  hand-written; a field type whose Handle lacks Debug breaks the generated
  Debug impl.
- inline consts CAN use generic projections (the canary's proof); plain const
  items and array lengths cannot (probed and rejected on this nightly).

tests/derive.rs integration tests: cmp-vs-Ord batteries (shapes A–D incl.
derive-on-derive recursion), narrow-width record-pad pins, heap-const folds,
atomic failed archive, release-order trace.