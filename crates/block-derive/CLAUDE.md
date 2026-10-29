# block-derive — Archive/RelOrd derives

Proc-macro crate generating block's `Archive` + `RelOrd` impls for named-field
structs with concrete field types (tuples/generics/enums are compile_error'd;
generic-struct-with-generic-fields is future work). The traits it targets are
FROZEN in block — `crates/block/CLAUDE.md` carries the contract map.

Generated shape per `#[derive(Archive)]` (a `#[derive(RelOrd)]` complements
it in the same crate):

- `struct XHandle { per-field `<F as Archive>::Handle` projects, ... }` —
  Copy + Eq + hand-written Debug (derive bounds would demand `A: Debug` on
  VecHandle's element type); `#[repr(C)]` (LOAD-BEARING: repr(Rust) may
  reorder the explicit pads into wrong record positions).
- Pads are CONST EXPRESSIONS over the real `size_of`/`align_of` of the
  projected handles (`_pad_k: [u8; (align − off % align) % align]`, running
  offset chained through quote), plus a computed `_tail` — no macro-side
  layout table, so ANY field shape works (derive-on-derive, manual handles).
- A `const _` canary pins `size_of::<XHandle>() == computed total` — a shape
  that would carry implicit padding is a compile error, not a dirty record.
- The same canary PROVES the emitted `unsafe impl ZeroPad/ReprC` — the
  markers `Block::push` gates on. (2026-10-28: the old closed Klass table +
  per-width TailPad machinery died with block's u16 pinning.)
- archive = per-field archive with UNWIND-based atomicity: a failure at
  field k releases fields k−1..0 in reverse (children first) then
  `al.rollback(heap0)` — no snapshot; the unwind list is generated per arm,
  so each Err arm's undo is mechanical. release = per-field release in
  reverse declaration order. Comparison order = declaration order
  (field-wise, short-circuiting).

Known quirks (don't re-derive these — see tests):
- `Ord::max`/`usize::max` are not const-callable on this toolchain — the
  align max chains through `if a > b { a } else { b }`.
- array-length const expressions CAN reference `size_of`/`align_of` of
  concrete types (the design's foundation — probed and confirmed).
- the generated hand-written Debug formats each field's projected Handle,
  so a MANUAL handle type without `Debug` fails inside `derive(Archive)`
  with an opaque bound error — give handwritten handles a Debug impl.

tests/derive.rs integration tests: cmp-vs-Ord batteries (shapes A–D incl.
derive-on-derive recursion), record-pad pins, heap-const folds, atomic
failed archive, release-order trace.