//!multi-block `Stacks` (devlog §5e): an append-only set of `Stack` records in
//!one file. block_id i = the file's i-th record (sequential u32, append-only —
//!ids and records are immortal); push appends into the CURRENT block, a full
//!one appends a fresh record to the file; pop retreats across exhausted
//!blocks (their records stay, empty). zero-record invariant holds in every
//!record, so a never-flushed block reads back as a valid empty one.
//!threading model — per-block RwLocks, no owner thread (the kanal actor was
//!dissolved 2026-10-27: the ~650ns channel RTT floor dominated every op):
//!- `Cell` = `RwLock<Slot { stack, dirty }>` + an atomic lease counter, one
//!  per record. mutations take the cell's write lock; read closures and
//!  lease grants are READ-FIRST: check residency under the read guard and
//!  run there — a resident block can't stop being resident under a read
//!  lock (the evictor's try_write fails against it), so the fast path has
//!  no eviction window and readers of one block run in PARALLEL. a miss
//!  escalates to the write path, where `ensure` rechecks residency under
//!  the lock (the TOCTOU double-check: a racing loader that won first
//!  leaves the stack loaded, no double file read, no double register).
//!- `Shared` (one mutex) owns the structure: the push/pop target `top`, the
//!  cell table, the LRU deque, the file length. LOCK ORDER is shared → cell →
//!  file, plus one cell → shared edge (with_slot's residency register) — safe
//!  because the only shared-holder that touches cells, the evictor, never
//!  BLOCKS on a cell lock (try_write only).
//!- reader counting is lease-based and real: holding a Lease = a reader, via
//!  the cell's atomic counter (incremented under the cell's write lock on
//!  the miss path, under the read guard on the fast path — safe either way:
//!  an evictor that already passed its readers check fails try_write
//!  against the guard, and one that acquires after sees the count).
//!  eviction (LRU, after each completing op) skips the MRU, leased
//!  and busy blocks — over-budget
//!  residency is the documented fallback (deferral), so budget pressure
//!  never blocks or loses a reader's view.
//!- mutation policy: dirty + write-back — residency changes and the last
//!  handle's drop flush dirty records to the file. crash-consistency is out
//!  of scope (no fsync); an unflushed block reads back as the previous (or
//!  empty) record. eviction flush failures are best-effort silent; `flush()`
//!  surfaces I/O errors explicitly.
//!- there is no owner thread to outlive anyone: the last `Stacks` clone drop
//!  is `Inner`'s Drop, which flushes every dirty record synchronously — a
//!  reopen after the final drop is deterministic. a Lease holds its CELL, not
//!  the shared state: the exit flush runs at the last clone's drop (flush
//!  takes each cell's write lock; a lease pins residency, not the flush),
//!  but a release is always exactly accounted — the counter it decrements
//!  cannot outlive it (the old channel drop was best-effort).
//!- a panic in a read closure unwinds naturally on the caller's thread; the
//!  lock poisons and later access recovers via into_inner — read closures
//!  cannot mutate, so poison carries no corruption. closures must not call
//!  back into the same Stacks (self-deadlock).
//!- concurrent ops interleave per-op: a pop's retreat can skip a block a
//!  racing push refilled after its emptiness was observed, and a push that
//!  fails a stale top retries against the current one. single-threaded
//!  semantics are exactly the old owner's.

use std::collections::VecDeque;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockWriteGuard};

use crate::archive::{Archive, Heap};
use crate::block::CapErr;
use crate::handle::S;
use crate::stack::{LoadErr, RECORD, Stack};

///SAFETY: Block owns exactly one allocation and carries no thread-affine
///state; every access runs under the owning cell's RwLock (or the final
///flush), so a Block shared through Stacks never aliases.
unsafe impl Send for crate::block::Block {}

///SAFETY: every &Block method is a read of the raw bytes (mutation is &mut
///only, no interior mutability); the cell RwLock keeps every live &Block
///clear of &mut aliasing, so read-guard sharing across threads is sound.
unsafe impl Sync for crate::block::Block {}

///default resident-block budget when a constructor takes none.
pub const DEFAULT_BUDGET: usize = 8;

///request failure. `Load` covers open/read/structural file errors, `Record`
///a per-block record that failed validation when made resident.
#[derive(Debug)]
pub enum StacksErr {
    Load(LoadErr),
    Record {
        block: u32,
        err:   LoadErr,
    },
    ///push space exhausted — the item exceeds a whole record
    Cap(CapErr),
    UnknownBlock {
        block: u32,
    },
}

impl From<io::Error> for StacksErr {
    fn from(e: io::Error) -> Self {
        StacksErr::Load(LoadErr::Io(e))
    }
}

impl fmt::Display for StacksErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Load(e) => write!(f, "{e}"),
            Self::Record { block, err } => write!(f, "block {block}: {err}"),
            Self::Cap(e) => write!(f, "{e}"),
            Self::UnknownBlock { block } => write!(f, "unknown block {block}"),
        }
    }
}

impl std::error::Error for StacksErr {}

///resident block: the live stack and its write-back state, behind the cell
///lock; `None` = evicted (on file, reloadable).
struct Slot<T: Archive> {
    stack: Option<Stack<T>>,
    dirty: bool,
}

///per-record state: the data latch and the lease pins (readers > 0 refuses
///eviction). increments ride a lock the evictor must respect — the cell's
///write lock on the miss path, or the read guard on the fast path (an
///evictor that already passed its readers check fails try_write against
///the guard); decrements are lock-free (a decrement only ever makes the
///block more evictable).
struct Cell<T: Archive> {
    lock:    RwLock<Slot<T>>,
    readers: AtomicU32,
}

///structural state under one mutex: the push/pop target, the cell table
///(append-only — ids are immortal), the LRU order (back = most recently
///touched), and the file length.
struct Shared<T: Archive> {
    top:      u32,
    cells:    Vec<Arc<Cell<T>>>,
    resident: VecDeque<u32>,
    file_len: u64,
}

struct Inner<T: Archive> {
    file:           Mutex<File>,
    ///record stride 16 + cap — fixed per file, derived from record 0 on load
    stride:         usize,
    budget:         usize,
    ///resident-lease count mirror — the evict fast gate (Relaxed: a stale
    ///read only defers eviction, which is the documented fallback anyway)
    resident_count: AtomicUsize,
    shared:         Mutex<Shared<T>>,
}

///consumer handle; clones share the file state through the Arc (Send + Sync
///by construction — every field is a lock or a File).
pub struct Stacks<T: Archive> {
    inner: Arc<Inner<T>>,
}

///snapshot lease: an owned record copy and the reader token (the cell's
///counter pin). not Clone — dropping releases the reader. the lease holds
///its CELL Arc, not the shared state (the exit flush may run while a lease
///lives — it pins residency, not the flush): the cell Arc keeps the
///counter's storage alive, so the drop's fetch_sub is always accounted.
pub struct Lease<T: Archive> {
    rec:   Arc<Vec<u8>>,
    block: u32,
    cell:  Arc<Cell<T>>,
}

impl<T: Archive> Drop for Lease<T> {
    fn drop(&mut self) {
        self.cell.readers.fetch_sub(1, Ordering::Relaxed);
    }
}

impl<T: Archive> Lease<T> {
    pub fn block(&self) -> u32 {
        self.block
    }

    ///the snapshot record bytes (stride long, validated at grant).
    pub fn bytes(&self) -> &[u8] {
        &self.rec
    }

    ///local read view — a fresh owned copy of the snapshot (consumer-side
    ///`Stack::from_record`; the view never aliases the live stack).
    pub fn view(&self) -> Result<Stack<T>, LoadErr> {
        Stack::from_record(&self.rec)
    }
}

impl<T: Archive> Clone for Stacks<T> {
    fn clone(&self) -> Self {
        Stacks { inner: self.inner.clone() }
    }
}

///the last clone's drop flushes synchronously — no owner thread exists, so
///nothing can race the exit flush.
impl<T: Archive> Drop for Inner<T> {
    fn drop(&mut self) {
        let cells: Vec<Arc<Cell<T>>> = Stacks::<T>::shared(self).cells.clone();
        for (id, cell) in cells.iter().enumerate() {
            let mut w = Stacks::<T>::wr(&cell.lock);
            //best-effort: the file's previous record is the fallback
            let _ = Stacks::<T>::flush_slot(self, &mut w, id as u32);
        }
    }
}

impl<T: Archive> Stacks<T> {
    ///creates (truncates) a fresh file with the default budget.
    pub fn create(path: impl AsRef<Path>) -> Result<Self, StacksErr> {
        Self::with_budget(path, DEFAULT_BUDGET)
    }

    ///creates (truncates) a fresh file; `budget` resident blocks max.
    pub fn with_budget(path: impl AsRef<Path>, budget: usize) -> Result<Self, StacksErr> {
        let cap = RECORD - 16;
        if cap > u16::MAX as usize {
            return Err(StacksErr::Load(LoadErr::BadCap { cap }));
        }
        let file =
            OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path)?;
        Ok(Stacks {
            inner: Arc::new(Inner {
                file: Mutex::new(file),
                stride: RECORD,
                budget,
                resident_count: AtomicUsize::new(0),
                shared: Mutex::new(Shared {
                    top:      0,
                    cells:    Vec::new(),
                    resident: VecDeque::new(),
                    file_len: 0,
                }),
            }),
        })
    }

    ///opens an existing multi-record file with the default budget.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, StacksErr> {
        Self::load_with_budget(path, DEFAULT_BUDGET)
    }

    ///opens an existing multi-record file. stride comes from record 0's cap;
    ///the file length must be a whole number of records (truncation →
    ///`Load(StrideMismatch)`), per-record contents validate at residency.
    pub fn load_with_budget(path: impl AsRef<Path>, budget: usize) -> Result<Self, StacksErr> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let len = file.metadata()?.len();
        let stride = if len == 0 {
            RECORD
        } else {
            let mut hdr = [0u8; 16];
            file.read_exact(&mut hdr)?;
            let cap = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as usize;
            if cap < 16 || !cap.is_multiple_of(16) || cap > u16::MAX as usize {
                return Err(StacksErr::Load(LoadErr::BadCap { cap }));
            }
            let stride = 16 + cap;
            if !len.is_multiple_of(stride as u64) {
                return Err(StacksErr::Load(LoadErr::StrideMismatch {
                    file: len as usize,
                    stride,
                }));
            }
            stride
        };
        let n = (len as usize / stride) as u32;
        let cells: Vec<_> = (0..n)
            .map(|_| {
                Arc::new(Cell {
                    lock:    RwLock::new(Slot { stack: None, dirty: false }),
                    readers: AtomicU32::new(0),
                })
            })
            .collect();
        Ok(Stacks {
            inner: Arc::new(Inner {
                file: Mutex::new(file),
                stride,
                budget,
                resident_count: AtomicUsize::new(0),
                shared: Mutex::new(Shared {
                    top: n.saturating_sub(1),
                    cells,
                    resident: VecDeque::new(),
                    file_len: len,
                }),
            }),
        })
    }

    // locks (poison carries no corruption — see the module header)

    fn wr(lock: &RwLock<Slot<T>>) -> RwLockWriteGuard<'_, Slot<T>> {
        lock.write().unwrap_or_else(PoisonError::into_inner)
    }

    fn shared(inner: &Inner<T>) -> MutexGuard<'_, Shared<T>> {
        inner.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn file(inner: &Inner<T>) -> MutexGuard<'_, File> {
        inner.file.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn sh(&self) -> MutexGuard<'_, Shared<T>> {
        Self::shared(&self.inner)
    }

    fn cell(sh: &Shared<T>, block: u32) -> Result<&Arc<Cell<T>>, StacksErr> {
        sh.cells.get(block as usize).ok_or(StacksErr::UnknownBlock { block })
    }

    ///the block's file record, validated (per-record cap must match the
    ///file's stride — from_record asserts that, so pre-check to stay
    ///panic-free).
    fn record_at(&self, block: u32) -> Result<Stack<T>, StacksErr> {
        let mut rec = vec![0u8; self.inner.stride];
        let mut file = Self::file(&self.inner);
        file.seek(SeekFrom::Start(block as u64 * self.inner.stride as u64))?;
        file.read_exact(&mut rec)?;
        drop(file);
        let cap = u32::from_le_bytes([rec[8], rec[9], rec[10], rec[11]]) as usize;
        if 16 + cap != self.inner.stride {
            return Err(StacksErr::Load(LoadErr::StrideMismatch {
                file:   self.inner.stride,
                stride: 16 + cap,
            }));
        }
        Stack::from_record(&rec).map_err(|e| StacksErr::Record { block, err: e })
    }

    ///entry prologue: LRU-touch `block` under shared, hand out its cell.
    fn cell_of(&self, block: u32) -> Result<Arc<Cell<T>>, StacksErr> {
        let mut sh = self.sh();
        Self::touch(&mut sh, block);
        Self::cell(&sh, block).map(Arc::clone)
    }

    ///cell write access with residency ensured: loads the record from file
    ///and registers it in the LRU order on first touch.
    fn with_slot<R>(
        &self,
        block: u32,
        f: impl FnOnce(&mut Slot<T>) -> R,
    ) -> Result<R, StacksErr> {
        let cell = self.cell_of(block)?;
        let mut w = Self::wr(&cell.lock);
        self.ensure(&mut w, block)?;
        Ok(f(&mut w))
    }

    ///residency for `block` under a HELD cell write lock: loads the record
    ///and registers it. the cell → shared edge here is the only one; safe
    ///because the evictor never blocks on a cell lock.
    fn ensure(
        &self,
        w: &mut RwLockWriteGuard<'_, Slot<T>>,
        block: u32,
    ) -> Result<(), StacksErr> {
        if w.stack.is_none() {
            let stack = self.record_at(block)?;
            w.stack = Some(stack);
            let mut sh = self.sh();
            Self::register(&self.inner, &mut sh, block);
        }
        Ok(())
    }

    ///LRU-order touch: moves `block` to the resident back (the MRU).
    fn touch(sh: &mut Shared<T>, block: u32) {
        if let Some(p) = sh.resident.iter().position(|&c| c == block)
            && p + 1 != sh.resident.len()
        {
            let v = sh.resident.remove(p).unwrap();
            sh.resident.push_back(v);
        }
    }

    ///residency registration: LRU back + the count mirror.
    fn register(inner: &Inner<T>, sh: &mut Shared<T>, block: u32) {
        debug_assert!(!sh.resident.contains(&block), "resident without a stack");
        sh.resident.push_back(block);
        inner.resident_count.fetch_add(1, Ordering::Relaxed);
    }

    ///post-op budget enforcement, gated by the count mirror — usually no
    ///lock at all on the hot path.
    fn enforce_budget(&self) {
        if self.inner.resident_count.load(Ordering::Relaxed) > self.inner.budget {
            let mut sh = self.sh();
            self.evict(&mut sh);
        }
    }

    ///LRU eviction to budget: skips the MRU, leased (readers > 0) and busy
    ///(try_write fails) blocks — deferral, never blocking.
    fn evict(&self, sh: &mut MutexGuard<'_, Shared<T>>) {
        while sh.resident.len() > self.inner.budget {
            let len = sh.resident.len();
            let mut victim = None;
            for i in 0..len.saturating_sub(1) {
                let id = sh.resident[i];
                let cell = sh.cells[id as usize].clone();
                if cell.readers.load(Ordering::Relaxed) != 0 {
                    continue;
                }
                let Ok(mut w) = cell.lock.try_write() else { continue };
                //recheck under the lock: a lease may have been granted between
                //the readers load above and this acquisition — a miss-path
                //grant's fetch_add rode this same lock, and a fast-path
                //grant released its read guard before we acquired (the
                //RwLock edge orders it) — either way it is visible here
                if cell.readers.load(Ordering::Relaxed) != 0 {
                    continue;
                }
                //best-effort: a failed write-back leaves the file's previous
                //record; explicit flush() is the honest surface
                let _ = Self::flush_slot(&self.inner, &mut w, id);
                //dirty dies with the stack (a failed write-back left it in
                //the file's previous record — flagging an evicted slot would
                //panic flush_slot's expect)
                w.stack = None;
                w.dirty = false;
                victim = Some(i);
                break;
            }
            let Some(i) = victim else { break };
            sh.resident.remove(i);
            self.inner.resident_count.fetch_sub(1, Ordering::Relaxed);
        }
    }

    ///dirty write-back of one record; caller holds the cell's write lock.
    fn flush_slot(inner: &Inner<T>, slot: &mut Slot<T>, id: u32) -> Result<(), StacksErr> {
        if !slot.dirty {
            return Ok(());
        }
        let rec = slot.stack.as_ref().expect("dirty evicted slot").to_record();
        let mut file = Self::file(inner);
        file.seek(SeekFrom::Start(id as u64 * inner.stride as u64))?;
        file.write_all(&rec)?;
        slot.dirty = false;
        Ok(())
    }

    ///a fresh empty record at the append frontier: the file grows and the
    ///birth record writes immediately, so even an unflushed block reads back
    ///as a valid empty record (a zeroed cap would read corrupt — subtle_bugs
    ///§6). caller holds the shared lock.
    fn append_block(&self, sh: &mut MutexGuard<'_, Shared<T>>) -> Result<u32, StacksErr> {
        let id = sh.cells.len() as u32;
        let end = (id as u64 + 1) * self.inner.stride as u64;
        let stack = Stack::with_cap(self.inner.stride - 16);
        {
            let mut file = Self::file(&self.inner);
            if end > sh.file_len {
                file.set_len(end)?;
                sh.file_len = end;
            }
            file.seek(SeekFrom::Start(id as u64 * self.inner.stride as u64))?;
            file.write_all(&stack.to_record())?;
        }
        sh.cells.push(Arc::new(Cell {
            lock:    RwLock::new(Slot { stack: Some(stack), dirty: true }),
            readers: AtomicU32::new(0),
        }));
        Self::register(&self.inner, sh, id);
        sh.top = id;
        Ok(id)
    }

    ///spill after a failed push into `seen`: only appends when `top` is still
    ///the failed block — a racing producer that already spilled owns the
    ///append, and this push retries into the fresh record instead.
    fn spill(&self, seen: u32) -> Result<(), StacksErr> {
        let mut sh = self.sh();
        if sh.top == seen {
            self.append_block(&mut sh)?;
        }
        Ok(())
    }

    ///archives k into the current block; full → appends a fresh record and
    ///continues there. returns (block id, stack address). `Cap` = the item
    ///exceeds a whole record (state unchanged; the last fresh record stays).
    pub fn push(&self, k: T) -> Result<(u32, S<T::Handle>), StacksErr> {
        loop {
            let top = {
                let mut sh = self.sh();
                if sh.cells.is_empty() {
                    self.append_block(&mut sh)?;
                }
                sh.top
            };
            //Err flag = the failed block was EMPTY: only an item exceeding a
            //whole record fails there — Cap. any other Err is a full block.
            let r = self.with_slot(top, |slot| {
                let stack = slot.stack.as_mut().unwrap();
                match stack.push_borrowed(&k) {
                    Ok(s) => {
                        slot.dirty = true;
                        Ok(s)
                    }
                    Err(e) => Err((e, stack.is_empty())),
                }
            })?;
            match r {
                Ok(s) => {
                    self.enforce_budget();
                    return Ok((top, s));
                }
                //stack contract: state unchanged on Err — k never landed
                Err((e, true)) => return Err(StacksErr::Cap(e)),
                Err((_, false)) => {
                    self.spill(top)?;
                }
            }
        }
    }

    ///pops the stack top — the current block, retreating across exhausted
    ///blocks. `None` when every block is empty.
    pub fn pop(&self) -> Result<Option<T::Handle>, StacksErr> {
        loop {
            let top = {
                let sh = self.sh();
                if sh.cells.is_empty() {
                    return Ok(None);
                }
                sh.top
            };
            //Option<Handle>: None = the block is empty (retreat), else the pop
            let popped = self.with_slot(top, |slot| {
                let stack = slot.stack.as_mut().unwrap();
                if !stack.is_empty() {
                    slot.dirty = true;
                }
                stack.pop()
            })?;
            match popped {
                Some(h) => {
                    self.enforce_budget();
                    return Ok(Some(h));
                }
                None => {
                    //retreat only from the block we saw empty; a racing op
                    //that moved `top` owns the decision
                    let mut sh = self.sh();
                    if sh.top == top {
                        if sh.top == 0 {
                            return Ok(None);
                        }
                        sh.top -= 1;
                    }
                }
            }
        }
    }

    ///global item count — visits every block (residency churn).
    pub fn len(&self) -> Result<usize, StacksErr> {
        let n = self.blocks()?;
        let mut total = 0;
        for b in 0..n {
            total += self.len_block(b)?;
        }
        Ok(total)
    }

    pub fn is_empty(&self) -> Result<bool, StacksErr> {
        Ok(self.len()? == 0)
    }

    pub fn len_block(&self, block: u32) -> Result<usize, StacksErr> {
        let n = self.with_slot(block, |slot| slot.stack.as_ref().unwrap().len())?;
        self.enforce_budget();
        Ok(n)
    }

    ///record count (append frontier; emptied mid-blocks stay counted).
    pub fn blocks(&self) -> Result<u32, StacksErr> {
        let n = self.sh().cells.len() as u32;
        self.enforce_budget();
        Ok(n)
    }

    ///current push/pop block id (0 for an empty stacks — no such block;
    /// accessing it errs `UnknownBlock`).
    pub fn top(&self) -> Result<u32, StacksErr> {
        let t = self.sh().top;
        self.enforce_budget();
        Ok(t)
    }

    ///runs `f` locally under the block's read lock; readers of one block run
    ///in parallel. resident blocks never touch the write lock (read-guard,
    ///check, run — no eviction window); only a miss escalates to the write
    ///path (load + run there). a panic in `f` unwinds on this thread (the
    ///lock poisons; later access recovers). `f` must not re-enter this
    ///Stacks.
    pub fn read<R, F>(&self, block: u32, f: F) -> Result<R, StacksErr>
    where F: FnOnce(&Stack<T>, &Heap<'_>) -> R {
        let cell = self.cell_of(block)?;
        //fast path: resident blocks never touch the write lock — check AND
        //run under one read guard (a resident block can't stop being
        //resident under a read lock: the evictor's try_write fails against
        //it), so this has no eviction window at all
        let g = cell.lock.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(stack) = &g.stack {
            let heap = Heap::new(stack.block());
            return Ok(f(stack, &heap));
        }
        drop(g);
        //miss: load under the write lock and run there; if a racing loader
        //won first the stack is already resident — with_slot's ensure
        //recheck leaves it a plain re-read under the held write lock
        let r = self.with_slot(block, |slot| {
            let stack = slot.stack.as_ref().unwrap();
            let heap = Heap::new(stack.block());
            f(stack, &heap)
        })?;
        self.enforce_budget();
        Ok(r)
    }

    ///ships an owned record snapshot; holding the lease pins the block
    ///against eviction (the returned Stack in `view()` is a local copy).
    ///resident blocks grant under the read guard; a miss escalates to the
    ///write path (ensure's recheck is the TOCTOU double).
    pub fn lease(&self, block: u32) -> Result<Lease<T>, StacksErr> {
        let cell = self.cell_of(block)?;
        //fast path: grant under the read guard — to_record is read-only and
        //the reader pin's fetch_add is safe there (an evictor that already
        //passed its readers check fails try_write against this guard)
        let g = cell.lock.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(stack) = &g.stack {
            let rec = Arc::new(stack.to_record());
            cell.readers.fetch_add(1, Ordering::Relaxed);
            drop(g);
            return Ok(Lease { rec, block, cell });
        }
        drop(g);
        //miss: write path — ensure loads (the TOCTOU double-check), grant
        //under write (an evictor either fails its try_write or holds the
        //lock and rechecks — a grant can never be missed)
        let rec = self.with_slot(block, |slot| {
            let stack = slot.stack.as_ref().unwrap();
            let rec = Arc::new(stack.to_record());
            cell.readers.fetch_add(1, Ordering::Relaxed);
            rec
        })?;
        self.enforce_budget();
        Ok(Lease { rec, block, cell })
    }

    ///flushes every dirty resident record to the file, surfacing I/O errors.
    pub fn flush(&self) -> Result<(), StacksErr> {
        let cells: Vec<Arc<Cell<T>>> = self.sh().cells.clone();
        let mut first = None;
        for (id, cell) in cells.iter().enumerate() {
            let mut w = Self::wr(&cell.lock);
            if let Err(e) = Self::flush_slot(&self.inner, &mut w, id as u32) {
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(()), Err)
    }

    ///diagnostic: resident block ids in LRU order (front = oldest).
    pub fn resident_ids(&self) -> Result<Vec<u32>, StacksErr> {
        let ids = self.sh().resident.iter().copied().collect();
        self.enforce_budget();
        Ok(ids)
    }
}
