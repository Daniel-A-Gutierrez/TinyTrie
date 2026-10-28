//!multi-block `Stacks` (devlog §5e): an append-only set of `Stack` records in
//!one file, managed by a single OWNED thread. block_id i = the file's i-th
//!record (sequential u32, append-only — ids and records are immortal); push
//!appends into the CURRENT block, a full one appends a fresh record to the
//!file; pop retreats across exhausted blocks (their records stay, empty).
//!zero-record invariant holds in every record, so a never-flushed block reads
//!back as a valid empty one.
//!threading model:
//!- the owner thread holds the file, the resident map, and the LRU budget;
//!  it is the single mutator — every request (push/pop/closure) runs on it,
//!  serially. consumers hold clones of `Stacks` (a channel sender); clones
//!  move freely between threads (Stacks: Send + Sync via the channel pair).
//!- reads cross the boundary two ways: `read(block, closure)` runs the
//!  closure ON the owner with exclusive access; `lease(block)` ships an
//!  owned record snapshot (`Arc`) and the consumer builds a LOCAL view via
//!  `Lease::view` (records are byte-exact validated round-trips). closures
//!  must not call Stacks methods that wait for responses (self-deadlock).
//!- reader counting is LEASE-based and real: holding a Lease = a reader.
//!  eviction (LRU, after each request) refuses read-locked blocks and the
//!  MRU — over-budget residency is the documented fallback (deferral), so
//!  budget pressure never blocks or loses a reader's view. closures need no
//!  reader tracking: the owner serves serially, nothing evicts mid-request.
//!- mutation policy: dirty + write-back — residency changes and owner exit
//!  flush dirty records to the file. crash-consistency is out of scope (no
//!  fsync); an unflushed block reads back as the previous (or empty) record.
//!  eviction flush failures are best-effort silent; `flush()` surfaces I/O
//!  errors explicitly.
//!- the thread lives until the last `Stacks` clone drops; that drop wakes
//!  the owner with a Shutdown and BLOCKS until it has flushed and exited, so
//!  a reopen after the last drop is deterministic. a Lease does NOT keep the
//!  owner alive — a release after the exit is best-effort.
//!- panic isolation: a read closure that panics is caught at the request
//!  boundary; the payload rides back and `read` resumes the unwind on the
//!  consumer thread. the owner keeps serving (read closures cannot mutate).
//!  an owner panic kills the thread — later requests report `Closed`.

use std::any::Any;
use std::collections::VecDeque;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use kanal;
use std::sync::Arc;

use crate::archive::{Archive, Heap};
use crate::block::CapErr;
use crate::handle::{Ptr, S};
use crate::stack::{LoadErr, RECORD, Stack};

///SAFETY: Block owns exactly one allocation and carries no thread-affine
///state; Stacks' channel protocol keeps access owner-only, so moving a Block
///to the owner thread never aliases. Sync stays unimplemented.
unsafe impl<P: Ptr> Send for crate::block::Block<P> {}

///default resident-block budget when a constructor takes none.
pub const DEFAULT_BUDGET: usize = 8;

///request failure. `Load` covers open/read/structural file errors, `Record`
///a per-block record that failed validation when made resident.
#[derive(Debug)]
pub enum StacksErr {
    ///owner thread gone (all handles dropped, or the owner panicked)
    Closed,
    Load(LoadErr),
    Record { block: u32, err: LoadErr },
    ///push space exhausted — the item exceeds a whole record
    Cap(CapErr),
    ///a read closure panicked; payload returned so the caller can resume
    Panicked(Box<dyn Any + Send>),
    UnknownBlock { block: u32 },
}

impl From<io::Error> for StacksErr {
    fn from(e: io::Error) -> Self {
        StacksErr::Load(LoadErr::Io(e))
    }
}

impl fmt::Display for StacksErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => write!(f, "stacks owner thread is gone"),
            Self::Load(e) => write!(f, "{e}"),
            Self::Record { block, err } => write!(f, "block {block}: {err}"),
            Self::Cap(e) => write!(f, "{e}"),
            Self::Panicked(_) => write!(f, "read closure panicked"),
            Self::UnknownBlock { block } => write!(f, "unknown block {block}"),
        }
    }
}

impl std::error::Error for StacksErr {}

///resident block: the live stack, its write-back state, and its lease count.
struct Slot<T: Archive<P>, P: Ptr + Send> {
    stack: Stack<T, P>,
    dirty: bool,
    readers: u32,
}

///owner-side state. lives entirely on the owner thread (Send via Block's
///Send impl); `slots.len()` is the record count (append frontier).
struct Owner<T: Archive<P> + Send + 'static, P: Ptr + Send> {
    file: File,
    ///record stride 16 + cap — fixed per file, derived from record 0 on load
    stride: usize,
    slots: Vec<Option<Slot<T, P>>>,
    ///resident ids in use order, back = most recently touched
    resident: VecDeque<u32>,
    ///push/pop target (retreats across emptied blocks)
    top: u32,
    budget: usize,
    file_len: u64,
}

///consumer op or lease release, over the one channel.
type ReadFn<T, P> = Box<dyn FnOnce(&Stack<T, P>, &Heap<P>) -> Box<dyn Any + Send> + Send>;

enum Request<T: Archive<P>, P: Ptr + Send> {
    Push { k: T, resp: kanal::Sender<Result<(u32, S<P>), StacksErr>> },
    Pop { resp: kanal::Sender<Result<Option<T::Handle>, StacksErr>> },
    LenBlock { block: u32, resp: kanal::Sender<Result<usize, StacksErr>> },
    Len { resp: kanal::Sender<Result<usize, StacksErr>> },
    Blocks { resp: kanal::Sender<Result<u32, StacksErr>> },
    Top { resp: kanal::Sender<Result<u32, StacksErr>> },
    Read {
        block: u32,
        f: ReadFn<T, P>,
        resp: kanal::Sender<Result<Box<dyn Any + Send>, StacksErr>>,
    },
    Lease { block: u32, resp: kanal::Sender<Result<Arc<Vec<u8>>, StacksErr>> },
    Flush { resp: kanal::Sender<Result<(), StacksErr>> },
    ResidentIds { resp: kanal::Sender<Vec<u32>> },
}

enum Msg<T: Archive<P>, P: Ptr + Send> {
    Req(Request<T, P>),
    LeaseDrop { block: u32 },
    ///last-handle wake: exit flush, ack, thread end
    Shutdown { ack: kanal::Sender<()> },
}

///shared channel state; clones register on `refs` so the last drop can
///wake the owner for its exit flush and WAIT for it (drop of the last
///handle blocks until the file is written and the thread is gone).
struct Inner<T: Archive<P>, P: Ptr + Send> {
    tx: kanal::Sender<Msg<T, P>>,
    refs: std::sync::Mutex<u32>,
}

///consumer handle; clones share the owner thread by channel.
pub struct Stacks<T: Archive<P> + Send + 'static, P: Ptr + Send = u32> where T::Handle: Send {
    inner: Arc<Inner<T, P>>,
}

///snapshot lease: an owned record copy and the reader token (block stays
///resident while the lease lives). not Clone — dropping releases the reader.
///a lease does NOT keep the owner alive: after the last handle's drop the
///release message is best-effort (the flush has already happened).
pub struct Lease<T: Archive<P> + Send + 'static, P: Ptr + Send> where T::Handle: Send {
    rec: Arc<Vec<u8>>,
    block: u32,
    tx: kanal::Sender<Msg<T, P>>,
}

impl<T: Archive<P> + Send + 'static, P: Ptr + Send> Drop for Lease<T, P> where T::Handle: Send {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::LeaseDrop { block: self.block });
    }
}

impl<T: Archive<P> + Send + 'static, P: Ptr + Send> Lease<T, P> where T::Handle: Send {
    pub fn block(&self) -> u32 {
        self.block
    }

    ///the snapshot record bytes (stride long, validated at grant).
    pub fn bytes(&self) -> &[u8] {
        &self.rec
    }

    ///local read view — a fresh owned copy of the snapshot (consumer-side
    ///`Stack::from_record`; the view never aliases the owner's stack).
    pub fn view(&self) -> Result<Stack<T, P>, LoadErr> {
        Stack::from_record(&self.rec)
    }
}

impl<T: Archive<P> + Send + 'static, P: Ptr + Send> Clone for Stacks<T, P> where T::Handle: Send {
    fn clone(&self) -> Self {
        *self.inner.refs.lock().unwrap() += 1;
        Stacks { inner: self.inner.clone() }
    }
}

impl<T: Archive<P> + Send + 'static, P: Ptr + Send> Drop for Stacks<T, P> where T::Handle: Send {
    fn drop(&mut self) {
        let last = {
            let mut r = self.inner.refs.lock().unwrap();
            *r -= 1;
            *r == 0
        };
        if last {
            //wake the owner for the exit flush and block until it acks — a
            //reopen racing the flush would read unflushed records
            let (ack, rx) = kanal::bounded(1);
            if self.inner.tx.send(Msg::Shutdown { ack }).is_ok() {
                let _ = rx.recv();
            }
        }
    }
}

impl<T: Archive<P> + Send + 'static, P: Ptr + Send> Stacks<T, P> where T::Handle: Send {
    ///creates (truncates) a fresh file with the default budget.
    pub fn create(path: impl AsRef<Path>) -> Result<Self, StacksErr> {
        Self::with_budget(path, DEFAULT_BUDGET)
    }

    ///creates (truncates) a fresh file; `budget` resident blocks max.
    pub fn with_budget(path: impl AsRef<Path>, budget: usize) -> Result<Self, StacksErr> {
        let cap = RECORD - 16;
        if cap > P::MAX.as_usize() {
            return Err(StacksErr::Load(LoadErr::BadCap { cap }));
        }
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(path)?;
        Self::spawn(Owner::new(file, RECORD, budget))
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
            if cap < 16 || !cap.is_multiple_of(16) || cap > P::MAX.as_usize() {
                return Err(StacksErr::Load(LoadErr::BadCap { cap }));
            }
            let stride = 16 + cap;
            if !len.is_multiple_of(stride as u64) {
                return Err(StacksErr::Load(LoadErr::StrideMismatch { file: len as usize, stride }));
            }
            stride
        };
        let n = (len as usize / stride) as u32;
        let slots: Vec<_> = (0..n).map(|_| None).collect();
        Self::spawn(Owner { file, stride, slots, resident: VecDeque::new(), top: n.saturating_sub(1), budget, file_len: len })
    }

    fn spawn(owner: Owner<T, P>) -> Result<Self, StacksErr> {
        let (tx, rx) = kanal::unbounded();
        std::thread::Builder::new()
            .name("block-stacks".into())
            .spawn(move || owner.run(rx))
            .map_err(|e| StacksErr::Load(LoadErr::Io(io::Error::other(e.to_string()))))?;
        let inner = Inner { tx, refs: std::sync::Mutex::new(1) };
        Ok(Stacks { inner: Arc::new(inner) })
    }

    ///one round-trip: build the request with a fresh oneshot, send, await.
    fn send_req<R>(&self, build: impl FnOnce(kanal::Sender<R>) -> Msg<T, P>) -> Result<R, StacksErr> {
        let (tx, rx) = kanal::bounded(1);
        self.inner.tx.send(build(tx)).map_err(|_| StacksErr::Closed)?;
        rx.recv().map_err(|_| StacksErr::Closed)
    }

    ///archives k into the current block; full → appends a fresh record and
    ///continues there. returns (block id, stack address). `Cap` = the item
    ///exceeds a whole record (state unchanged; the fresh empty record stays).
    pub fn push(&self, k: T) -> Result<(u32, S<P>), StacksErr> {
        self.send_req(|tx| Msg::Req(Request::Push { k, resp: tx }))?
    }

    ///pops the stack top — the current block, retreating across exhausted
    ///blocks. `None` when every block is empty.
    pub fn pop(&self) -> Result<Option<T::Handle>, StacksErr> {
        self.send_req(|tx| Msg::Req(Request::Pop { resp: tx }))?
    }

    ///global item count — visits every block (residency churn).
    pub fn len(&self) -> Result<usize, StacksErr> {
        self.send_req(|tx| Msg::Req(Request::Len { resp: tx }))?
    }

    pub fn is_empty(&self) -> Result<bool, StacksErr> {
        Ok(self.len()? == 0)
    }

    pub fn len_block(&self, block: u32) -> Result<usize, StacksErr> {
        self.send_req(|tx| Msg::Req(Request::LenBlock { block, resp: tx }))?
    }

    ///record count (append frontier; emptied mid-blocks stay counted).
    pub fn blocks(&self) -> Result<u32, StacksErr> {
        self.send_req(|tx| Msg::Req(Request::Blocks { resp: tx }))?
    }

    ///current push/pop block id.
    pub fn top(&self) -> Result<u32, StacksErr> {
        self.send_req(|tx| Msg::Req(Request::Top { resp: tx }))?
    }

    ///runs `f` on the owner thread with exclusive access to the block;
    ///`R` ships home by value (Send + 'static — no references may escape:
    ///the block is !Sync, so borrowing it can never make R Send). a panic
    ///in `f` resumes on this thread.
    pub fn read<R, F>(&self, block: u32, f: F) -> Result<R, StacksErr>
    where
        F: FnOnce(&Stack<T, P>, &Heap<P>) -> R + Send + 'static,
        R: Send + 'static,
    {
        let f: ReadFn<T, P> = Box::new(move |s, h| Box::new(f(s, h)) as Box<dyn Any + Send>);
        match self.send_req(|tx| Msg::Req(Request::Read { block, f, resp: tx }))? {
            Ok(v) => Ok(*v.downcast::<R>().expect("response type")),
            Err(StacksErr::Panicked(p)) => panic::resume_unwind(p),
            Err(e) => Err(e),
        }
    }

    ///ships an owned record snapshot; holding the lease pins the block
    ///against eviction (the returned Stack in `view()` is a local copy).
    pub fn lease(&self, block: u32) -> Result<Lease<T, P>, StacksErr> {
        let rec = self.send_req(|tx| Msg::Req(Request::Lease { block, resp: tx }))??;
        Ok(Lease { rec, block, tx: self.inner.tx.clone() })
    }

    ///flushes every dirty resident record to the file, surfacing I/O errors.
    pub fn flush(&self) -> Result<(), StacksErr> {
        self.send_req(|tx| Msg::Req(Request::Flush { resp: tx }))?
    }

    ///diagnostic: resident block ids in LRU order (front = oldest).
    pub fn resident_ids(&self) -> Result<Vec<u32>, StacksErr> {
        self.send_req(|tx| Msg::Req(Request::ResidentIds { resp: tx }))
    }
}

impl<T: Archive<P> + Send + 'static, P: Ptr + Send> Owner<T, P> {
    fn new(file: File, stride: usize, budget: usize) -> Self {
        Owner {
            file,
            stride,
            slots: Vec::new(),
            resident: VecDeque::new(),
            top: 0,
            budget,
            file_len: 0,
        }
    }

    fn run(mut self, rx: kanal::Receiver<Msg<T, P>>) {
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Shutdown { ack } => {
                    let _ = self.flush_all();
                    let _ = ack.send(());
                    break;
                }
                Msg::LeaseDrop { block } => {
                    if let Some(s) = self.slots.get_mut(block as usize).and_then(|s| s.as_mut()) {
                        debug_assert!(s.readers > 0, "lease release without a grant");
                        s.readers = s.readers.saturating_sub(1);
                    }
                    self.evict_budget();
                }
                Msg::Req(r) => {
                    self.handle(r);
                    self.evict_budget();
                }
            }
        }
        let _ = self.flush_all();
    }

    fn handle(&mut self, r: Request<T, P>) {
        match r {
            Request::Push { k, resp } => {
                let _ = resp.send(self.push(k));
            }
            Request::Pop { resp } => {
                let _ = resp.send(self.pop());
            }
            Request::LenBlock { block, resp } => {
                let _ = resp.send(self.len_block(block));
            }
            Request::Len { resp } => {
                let _ = resp.send(self.len_all());
            }
            Request::Blocks { resp } => {
                let _ = resp.send(Ok(self.slots.len() as u32));
            }
            Request::Top { resp } => {
                let _ = resp.send(Ok(self.top));
            }
            Request::Read { block, f, resp } => {
                self.run_read(block, f, resp);
            }
            Request::Lease { block, resp } => {
                let _ = resp.send(self.grant_lease(block));
            }
            Request::Flush { resp } => {
                let _ = resp.send(self.flush_all());
            }
            Request::ResidentIds { resp } => {
                let _ = resp.send(self.resident.iter().copied().collect());
            }
        }
    }

    fn push(&mut self, k: T) -> Result<(u32, S<P>), StacksErr> {
        if self.slots.is_empty() {
            self.top = self.append_block()?;
        }
        let top = self.top;
        let slot = self.ensure(top)?;
        //borrowed attempt: `k` survives so the spill retry can consume it
        match slot.stack.push_borrowed(&k) {
            Ok(s) => {
                slot.dirty = true;
                Ok((top, s))
            }
            //stack contract: state unchanged on Err — spill to a fresh record
            Err(_) => {
                self.top = self.append_block()?;
                let id = self.top;
                let slot = self.ensure(id)?;
                match slot.stack.push(k) {
                    Ok(s) => {
                        slot.dirty = true;
                        Ok((id, s))
                    }
                    Err(e) => Err(StacksErr::Cap(e)),
                }
            }
        }
    }

    fn pop(&mut self) -> Result<Option<T::Handle>, StacksErr> {
        loop {
            if self.slots.is_empty() {
                return Ok(None);
            }
            let slot = self.ensure(self.top)?;
            if !slot.stack.is_empty() {
                let h = slot.stack.pop();
                slot.dirty = true;
                return Ok(h);
            }
            if self.top == 0 {
                return Ok(None);
            }
            self.top -= 1;
        }
    }

    fn len_all(&mut self) -> Result<usize, StacksErr> {
        let n = self.slots.len() as u32;
        (0..n).map(|id| self.len_block(id)).sum()
    }

    fn len_block(&mut self, block: u32) -> Result<usize, StacksErr> {
        let slot = self.checked(block)?;
        Ok(slot.stack.len())
    }

    fn run_read(
        &mut self,
        block: u32,
        f: ReadFn<T, P>,
        resp: kanal::Sender<Result<Box<dyn Any + Send>, StacksErr>>,
    ) {
        let slot = match self.checked(block) {
            Ok(s) => s,
            Err(e) => {
                let _ = resp.send(Err(e));
                return;
            }
        };
        let heap = Heap::new(slot.stack.block());
        //read closures cannot mutate owner state; the owner keeps serving
        let out = panic::catch_unwind(AssertUnwindSafe(|| f(&slot.stack, &heap)));
        let _ = resp.send(match out {
            Ok(v) => Ok(v),
            Err(p) => Err(StacksErr::Panicked(p)),
        });
    }

    fn grant_lease(&mut self, block: u32) -> Result<Arc<Vec<u8>>, StacksErr> {
        let slot = self.checked(block)?;
        let rec = Arc::new(slot.stack.to_record());
        slot.readers += 1;
        Ok(rec)
    }

    fn checked(&mut self, block: u32) -> Result<&mut Slot<T, P>, StacksErr> {
        if block as usize >= self.slots.len() {
            return Err(StacksErr::UnknownBlock { block });
        }
        self.ensure(block)
    }

    ///residency for `block`: touches if resident, else loads from file (a
    ///read of an evicted block transparently reloads).
    fn ensure(&mut self, block: u32) -> Result<&mut Slot<T, P>, StacksErr> {
        let i = block as usize;
        if self.slots[i].is_none() {
            let stack = self.record_at(block)?;
            self.slots[i] = Some(Slot { stack, dirty: false, readers: 0 });
            self.resident.push_back(block);
        } else if let Some(p) = self.resident.iter().position(|&c| c == block)
            && p + 1 != self.resident.len()
        {
            let v = self.resident.remove(p).unwrap();
            self.resident.push_back(v);
        }
        Ok(self.slots[i].as_mut().unwrap())
    }

    ///the block's file record, validated (per-record cap must match the
    ///file's stride — from_record asserts that, so pre-check to stay panic-free).
    fn record_at(&mut self, block: u32) -> Result<Stack<T, P>, StacksErr> {
        let mut rec = vec![0u8; self.stride];
        self.file.seek(SeekFrom::Start(block as u64 * self.stride as u64))?;
        self.file.read_exact(&mut rec)?;
        let cap = u32::from_le_bytes([rec[8], rec[9], rec[10], rec[11]]) as usize;
        if 16 + cap != self.stride {
            return Err(StacksErr::Load(LoadErr::StrideMismatch { file: self.stride, stride: 16 + cap }));
        }
        Stack::from_record(&rec).map_err(|e| StacksErr::Record { block, err: e })
    }

    ///a fresh empty record: the file grows, the slot is born dirty (an
    ///unflushed birth reads back as the zero record).
    fn append_block(&mut self) -> Result<u32, StacksErr> {
        let id = self.slots.len() as u32;
        let end = (id as u64 + 1) * self.stride as u64;
        if end > self.file_len {
            self.file.set_len(end)?;
            self.file_len = end;
        }
        self.slots.push(None);
        let slot = Slot { stack: Stack::with_cap(self.stride - 16), dirty: true, readers: 0 };
        //the birth record writes immediately, so even an unflushed block
        //reads back as a valid empty record (zeroed cap would read corrupt)
        self.file.seek(SeekFrom::Start(id as u64 * self.stride as u64))?;
        self.file.write_all(&slot.stack.to_record())?;
        self.slots[id as usize] = Some(slot);
        self.resident.push_back(id);
        Ok(id)
    }

    ///LRU eviction to budget: skips read-locked slots and the MRU (deferral
    ///— budget pressure never takes a reader's block or the just-used one).
    fn evict_budget(&mut self) {
        while self.resident.len() > self.budget {
            let pos = (0..self.resident.len().saturating_sub(1)).find(|&i| {
                self.slots[self.resident[i] as usize].as_ref().unwrap().readers == 0
            });
            let Some(pos) = pos else { break };
            let id = self.resident.remove(pos).unwrap();
            //best-effort: a failed write-back leaves the file's previous
            //record; explicit flush() is the honest surface
            let _ = self.flush_slot(id);
            self.slots[id as usize] = None;
        }
    }

    fn flush_slot(&mut self, id: u32) -> Result<(), StacksErr> {
        let Some(slot) = self.slots[id as usize].as_mut() else { return Ok(()) };
        if !slot.dirty {
            return Ok(());
        }
        let rec = slot.stack.to_record();
        self.file.seek(SeekFrom::Start(id as u64 * self.stride as u64))?;
        self.file.write_all(&rec)?;
        slot.dirty = false;
        Ok(())
    }

    fn flush_all(&mut self) -> Result<(), StacksErr> {
        let mut first = None;
        for i in 0..self.slots.len() as u32 {
            if let Err(e) = self.flush_slot(i) {
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(()), Err)
    }
}