# Lifetimes — notes from a design discussion

## Core model
- A lifetime/region is a **set of CFG points**, not a duration in time or a stack frame. We talk about it as a duration, but it's spatial (a subset of the control-flow graph).
- `'a: 'b` ("'a outlives 'b") = region `'a ⊇ region 'b` (set containment). It's a **partial order** — regions can overlap without containment, so it's not a total "height" ordering.
- The borrow checker assigns lifetimes only to **borrows** (`&`/`&mut`) and the borrowed parts inside types — not to owned values. Owned values have a **drop region** (creation→drop), not a borrow-lifetime.

## Structs
- `struct Foo<'a, T>(&'a T)`: `'a` is a real declared generic param (no elision on struct/enum/type defs). It means "Foo holds a borrow valid for at least `'a`" — NOT "the struct lives for `'a`."
- The value has its own life `[creation, drop]`; well-formedness implies `'a ⊇ value-life` (borrow outlives the value) and `T: 'a` (referent outlives the borrow): `referent ⊇ 'a ⊇ value-life`.
- `'a` is inferred at creation+use, never named at the value level (`Foo(&x)`, not `Foo::<'a>::new`); pinned to the overlap of the fed-in refs' validity and the value's use. Covariant, so a long borrow shortens freely.
- Multiple params → independent durations (don't collapse to one shortest overlap); the lending split uses an outlives bound (`'block: 'cur`).
- No params → fully owned; nothing tracked can dangle. (Raw ptrs / `Rc` / `Arc` aren't tracked, so "no param" = no *tracked* borrows.)
- A struct can store a borrow for its whole life because it owns the arrangement.

## Traits (vs structs)
- A trait is an interface, not a value — no creation/drop, so no "the trait's lifetime." Generic over `Self` (any concrete type, any lifespan).
- Trait methods reason **per-call over the borrow lifetime of `&self`/`&mut self`** (and args), not over the value's existence. `fn f(&self) -> &T` ≡ `fn f<'b>(&'b self) -> &'b T`; the caller picks `'b` per call and guarantees `Self: 'b`.
- A trait method only borrows `self` transiently per call; it can't keep the borrow past the call unless it returns something tied to it (lending). That's the GAT case: `type W<'walker>` with `where Self: 'walker` — the trait can't assume `Self`'s full life, only that it outlives the loan window. `Self: 'walker` is the trait-level analog of the struct's `'a ⊇ value-life`, expressed as "receiver outlives this loan."
- Two separate lifetime knobs: **trait params** (`trait Foo<'a>`, the impl instantiates) vs **trait object lifetime** (`dyn Foo + 'obj`, the erased value's validity, defaults to `'static`).

## Borrow checker (NLL)
- Procedure: lower to CFG/MIR; record each borrow (place, kind, ref var); compute each borrow's live range by **backward liveness** (creation → last use, path-sensitive); at each CFG point, per place, check the access against the live borrows.
- NLL regions end at **last use**, not at lexical scope end — that's what makes regions shorter than `{}` and lets two borrows overlap without nesting.
- Conflict rules per place: `&`+`&` ok; `&mut`+`&` no; `&mut`+`&mut` no; no move/drop while borrowed.
- `x = 1` is a **mutable access** (write) — same exclusivity tier as `&mut`/move, not a `&mut` itself.

## Three borrow states (the hidden third)
- The type system has two ref types (`&T`, `&mut T`). The borrow checker/Miri tracks three *states*: **Shared** (`&`), **Unique/Active** (`&mut` mutating), **Frozen** (`&mut` reborrowed as shared — readable, not writable, no new `&mut`).
- A `&mut` reborrowed as `&` is Frozen for that window: more `&` coexist, but a fresh `&mut` is blocked. So "Frozen `&mut` + `&`" is allowed (it's really `&`+`&`); `&mut`+`&mut`, or a fresh `&mut` vs a Frozen one, is not.

## NLL vs Polonius
- **NLL**: a borrow reserves the whole contiguous span `[creation, last use]`; any conflicting access in it is illegal.
- **Polonius**: a borrow is needed only at its **use points**; between uses, accesses are allowed **as long as they don't invalidate** the loan (move/drop the place). Non-killing writes (scalar reassign) are fine; killing ones (move a `String`, which frees the buffer) still error. The shift is from *reservation* to *invalidation*.
- The relation `'a: 'b = 'a ⊇ 'b` is the same in both; only the *point-sets* computed differ (Polonius's are sparser).

## Disjoint spans / overlapping regions
- Disjoint liveness arises from **control flow** (a borrow used in two `if c` blocks, separated by unconditional code). Single-path disjoint borrows (used, gap-mutated, used again) error under NLL, become legal under Polonius (non-killing gap).
- Overlap without containment: interleaved borrows (`ra = &a` then `rb = &b`, `use(ra)` then `use(rb)`) → regions cross, neither ⊇ the other → **incomparable**. This is why outlives is a partial order and why a struct holding both needs two lifetime params.

## Lending cursor case (the soundness crux)
- `fn next(&mut self) -> &T` elides to `fn next<'s>(&'s mut self) -> &'s T`: the output is tied to the `&mut self` borrow, so the live borrow while the returned ref exists is a **mutable** borrow (kind from the call, not from the derived `&T`), living until the ref's last use.
- Holding two returned `&T`s simultaneously fails because both `&mut self` borrows are live at the joint use — and this is **soundness, not a checker limitation**: the signature permits the impl to mutate/reallocate the backing (`&mut self` → `&mut Vec` → `push`/reallocate), which would dangle the first ref. Polonius can't relax it (the first ref is used after the second call, so its borrow is live at the second call → no gap; and relaxing would be UB).
- Fixes: `&self -> &T` (true shared iterator, multiple refs coexist), or accept one-at-a-time (GAT `LendingIterator`), or split the borrow (shared backing `&Vec` + owned `pos`).

## Mental model
- "Stack height, longer-lived at the bottom, only point at ≥-lived things" — the direction rule is correct and nails ~90% (nested lexical borrows). Loose because: lifetimes are per-region (NLL: use-spans, not whole frames), it's a partial order not a total height, and `'static`/heap/`Rc` escape the stack but keep the rule. Drop to "referent must outlive referer" for the weird cases.