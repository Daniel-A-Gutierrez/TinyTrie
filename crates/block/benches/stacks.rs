//!Stacks threading-layer overhead bench (devlog §5e, task 8): fair cells for
//!push routing, the read-vs-lease pattern choice, eviction churn, flush
//!amortization, and clone-handle concurrency scaling. keys cross the
//!StrHandle cutoff: short = 8 bytes (inline, no heap), long = 32 bytes (heap
//!append). every rep builds a fresh file in temp_dir; cells are capped at
//!RUN_BUDGET with partial reps over the completed prefix (doa bench style).
//!decomposition the harness leans on:
//!- `Stacks::push` has no fire-and-forget form (send_req always awaits a
//!  oneshot), so the channel round-trip floor is measured separately as an
//!  mpsc echo with the same per-request shape (fresh channel pair + keyed
//!  message per op) — `stacks − echo` ≈ owner-side work, `echo` = RTT floor.
//!- `archive-only` preallocates one big Block outside the timing (no spill
//!  churn) to isolate `String::archive`; `stack-direct` uses real 4080-byte
//!  records and spills to a fresh Stack when full, mirroring Stacks' append.
//!- long32 keys pay 56 stack+heap bytes/item (unit = snap 4 + pad 4 + handle
//!  16): block 0 fills at 72 items, and a `Stacks::push` stream appends a
//!  fresh record (with an immediate 4096-byte birth write) every 72 items —
//!  that file churn is honest cost.

use block::{Archive, Heap, Ptr, Stacks, Stack};
use std::hint::black_box;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Barrier};
use std::time::{Duration, Instant};

const RUNS: usize = 5;
const RUN_BUDGET: Duration = Duration::from_secs(3);
const PUSHS: usize = 4096;
///long32 items a 4080-byte record holds (56 bytes/item: unit 24 + heap 32)
const FILL_ITEMS: usize = 72;
const ACCESSES: usize = 2000;
const FLUSH_N: usize = 2000;
const THREAD_ITEMS: usize = 2000;

///deterministic key material
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0
    }
    ///hex key of `len` chars (< = 8 rides the prefix, > = 9 appends the heap)
    fn keys(&mut self, len: usize, n: usize) -> Vec<String> {
        (0..n)
            .map(|_| {
                let mut s = String::with_capacity(len);
                while s.len() < len {
                    s.push_str(&format!("{:016x}", self.next()));
                    s.truncate(len);
                }
                s
            })
            .collect()
    }
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("block-stacks-bench-{name}.tmp"))
}

///best/med/avg ns per op over the reps that fit the cell budget
struct Cell {
    best: f64,
    med: f64,
    avg: f64,
    partial: Option<usize>,
}

impl Cell {
    fn fmt(&self, name: &str, w: usize) -> String {
        let partial = match self.partial {
            Some(done) => format!(" [partial {done}]"),
            None => String::new(),
        };
        format!(
            "{name:<32} {best:>w$.1} {med:>w$.1} {avg:>w$.1}{partial}",
            best = self.best, med = self.med, avg = self.avg
        )
    }
}

fn reps(mut rep: impl FnMut(&Instant) -> (usize, bool, f64)) -> Cell {
    let deadline = Instant::now() + RUN_BUDGET;
    let mut vals = Vec::new();
    let mut partial = None;
    while vals.len() < RUNS && Instant::now() < deadline {
        let (done, cut, ns) = rep(&deadline);
        vals.push(ns);
        if cut {
            partial = Some(done);
            break;
        }
    }
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = vals.len() as f64;
    let mut med = vals[n as usize / 2];
    if n % 2.0 == 0.0 {
        med = (vals[n as usize / 2 - 1] + vals[n as usize / 2]) / 2.0;
    }
    let mut avg = 0.0;
    for v in &vals {
        avg += v / n;
    }
    Cell { best: vals[0], med, avg, partial }
}

fn elapsed(t: Instant, done: usize) -> f64 {
    t.elapsed().as_nanos() as f64 / done.max(1) as f64
}

///mid-loop deadline probe (checked every `tick` items)
fn past(deadline: &Instant, done: usize, tick: usize) -> bool {
    done % tick == 0 && Instant::now() > *deadline
}

fn timed_cell(name: &str, w: usize, rep: impl FnMut(&Instant) -> (usize, bool, f64)) {
    println!("{}", reps(rep).fmt(name, w));
}

///checksum of every stack item (inline keys ride the prefix, heap keys chase)
fn scan_items(s: &Stack<String, u32>, hp: &Heap<u32>) -> usize {
    let mut c = 0usize;
    for (_, sh) in s.items() {
        c = c.wrapping_add(if sh.h.0.as_usize() == 0 {
            sh.h.1.as_usize() ^ sh.pfx as usize
        } else {
            let b = hp.bytes(sh.h);
            b.len() ^ b.first().map_or(0, |&x| x as usize)
        });
    }
    c
}

fn main() {
    let w = 10;
    println!("block::Stacks overhead bench — ns/item or ns/access, columns best/med/avg, cell budget {RUN_BUDGET:?}");
    println!();

    // 1. push (ns/item)
    println!("== push (ns/item): Stacks::push vs Stack::push_borrowed vs String::archive vs channel RTT ==");
    for (shape, klen) in [("short8", 8usize), ("long32", 32usize)] {
        let path = temp_path(&format!("push-{shape}"));
        timed_cell(&format!("push/{shape}/archive-only"), w, |deadline| {
            let cap = (PUSHS * 32).next_multiple_of(16);
            let mut b = block::Block::<u32>::new(cap);
            let ks = Lcg(0xDEAD_BEEF_CAFE_F00D).keys(klen, PUSHS);
            let t = Instant::now();
            let mut done = 0;
            for k in &ks {
                let h = { let mut al = b.allocator(); String::archive(k, &mut al).unwrap() };
                black_box(&h);
                done += 1;
                if past(deadline, done, 256) {
                    return (done, true, elapsed(t, done));
                }
            }
            (done, false, elapsed(t, done))
        });
        timed_cell(&format!("push/{shape}/stack-direct"), w, |deadline| {
            let ks = Lcg(0xDEAD_BEEF_CAFE_F00D).keys(klen, PUSHS);
            let mut st = Stack::<String>::new();
            let t = Instant::now();
            let mut done = 0;
            for k in &ks {
                if st.push_borrowed(k).is_err() {
                    st = Stack::new();
                    st.push_borrowed(k).unwrap(); //mirror Stacks' spill: item lands in the fresh record
                }
                done += 1;
                if past(deadline, done, 256) {
                    return (done, true, elapsed(t, done));
                }
            }
            black_box(&st);
            (done, false, elapsed(t, done))
        });
        timed_cell(&format!("push/{shape}/stacks"), w, |deadline| {
            let s = Stacks::<String>::create(&path).unwrap();
            let ks = Lcg(0xDEAD_BEEF_CAFE_F00D).keys(klen, PUSHS);
            let t = Instant::now();
            let mut done = 0;
            for k in ks {
                let r = s.push(k).unwrap();
                black_box(&r);
                done += 1;
                if past(deadline, done, 256) {
                    return (done, true, elapsed(t, done));
                }
            }
            (done, false, elapsed(t, done))
        });
        timed_cell(&format!("push/{shape}/echo-rtt-floor"), w, |deadline| {
            //same per-op shape as Stacks::push: fresh oneshot pair, keyed
            //request, owner replies; no owner work (std mpsc — the original
            //floor before the kanal swap)
            let (stx, srx) = mpsc::channel::<(String, mpsc::Sender<String>)>();
            std::thread::spawn(move || {
                for (k, rtx) in srx {
                    let _ = rtx.send(k);
                }
            });
            let ks = Lcg(0xDEAD_BEEF_CAFE_F00D).keys(klen, PUSHS);
            let t = Instant::now();
            let mut done = 0;
            for k in ks {
                let (rtx, rrx) = mpsc::channel::<String>();
                stx.send((k, rtx)).unwrap();
                black_box(rrx.recv().unwrap());
                done += 1;
                if past(deadline, done, 256) {
                    return (done, true, elapsed(t, done));
                }
            }
            (done, false, elapsed(t, done))
        });
        timed_cell(&format!("push/{shape}/echo-rtt-floor-kanal"), w, |deadline| {
            //kanal echo: identical per-op shape, bounded(1) oneshot — the
            //transport A/B against the std floor above
            let (stx, srx) = kanal::unbounded::<(String, kanal::Sender<String>)>();
            std::thread::spawn(move || {
                for _ in 0..PUSHS {
                    let (k, rtx) = srx.recv().unwrap();
                    let _ = rtx.send(k);
                }
            });
            let ks = Lcg(0xDEAD_BEEF_CAFE_F00D).keys(klen, PUSHS);
            let t = Instant::now();
            let mut done = 0;
            for k in ks {
                let (rtx, rrx) = kanal::bounded::<String>(1);
                stx.send((k, rtx)).unwrap();
                black_box(rrx.recv().unwrap());
                done += 1;
                if past(deadline, done, 256) {
                    return (done, true, elapsed(t, done));
                }
            }
            (done, false, elapsed(t, done))
        });
        timed_cell(&format!("push/{shape}/echo-rtt-floor-kanal-unbounded"), w, |deadline| {
            //kanal echo with unbounded replies — bounded(1) vs unbounded
            //oneshot A/B for the per-request response channel
            let (stx, srx) = kanal::unbounded::<(String, kanal::Sender<String>)>();
            std::thread::spawn(move || {
                for _ in 0..PUSHS {
                    let (k, rtx) = srx.recv().unwrap();
                    let _ = rtx.send(k);
                }
            });
            let ks = Lcg(0xDEAD_BEEF_CAFE_F00D).keys(klen, PUSHS);
            let t = Instant::now();
            let mut done = 0;
            for k in ks {
                let (rtx, rrx) = kanal::unbounded::<String>();
                stx.send((k, rtx)).unwrap();
                black_box(rrx.recv().unwrap());
                done += 1;
                if past(deadline, done, 256) {
                    return (done, true, elapsed(t, done));
                }
            }
            (done, false, elapsed(t, done))
        });
        let _ = std::fs::remove_file(&path);
    }
    println!();

    // 2. read vs lease (per-access ns): block 0 preloaded with {FILL_ITEMS}
    // long32 items (~4KB of stack+heap content), no eviction under the
    // default budget
    println!("== read pattern (per-access ns) — block 0 = {FILL_ITEMS} long32 items; scan = all-items checksum ==");
    let path = temp_path("read");
    {
        let s = Stacks::<String>::create(&path).unwrap();
        let ks = Lcg(0x0B1C_2D3E_4F50_6172).keys(32, FILL_ITEMS);
        for k in &ks {
            s.push(k.clone()).unwrap();
        }
        assert_eq!(s.top().unwrap(), 0, "block-0 fill assumption ({FILL_ITEMS} long32)");
        s.flush().unwrap();
    }
    timed_cell("read/1-item/read-closure", w, |deadline| {
        let s = Stacks::<String>::load(&path).unwrap();
        let t = Instant::now();
        let mut done = 0;
        for _ in 0..ACCESSES {
            let c = s.read(0, |s, _| s.last().unwrap().1.pfx as usize).unwrap();
            black_box(&c);
            done += 1;
            if past(deadline, done, 256) {
                return (done, true, elapsed(t, done));
            }
        }
        (done, false, elapsed(t, done))
    });
    timed_cell("read/4k-items/read-closure", w, |deadline| {
        let s = Stacks::<String>::load(&path).unwrap();
        let t = Instant::now();
        let mut done = 0;
        for _ in 0..ACCESSES {
            let c = s.read(0, |s, hp| scan_items(s, hp)).unwrap();
            black_box(&c);
            done += 1;
            if past(deadline, done, 256) {
                return (done, true, elapsed(t, done));
            }
        }
        (done, false, elapsed(t, done))
    });
    timed_cell("lease/full-per-access", w, |deadline| {
        let s = Stacks::<String>::load(&path).unwrap();
        let t = Instant::now();
        let mut done = 0;
        for _ in 0..ACCESSES {
            let l = s.lease(0).unwrap();
            let v = l.view().unwrap();
            black_box(scan_items(&v, &v.heap()));
            drop(l);
            done += 1;
            if past(deadline, done, 128) {
                return (done, true, elapsed(t, done));
            }
        }
        (done, false, elapsed(t, done))
    });
    timed_cell("lease/grant-only", w, |deadline| {
        let s = Stacks::<String>::load(&path).unwrap();
        let t = Instant::now();
        let mut done = 0;
        for _ in 0..ACCESSES {
            let l = s.lease(0).unwrap();
            black_box(l.block());
            drop(l);
            done += 1;
            if past(deadline, done, 128) {
                return (done, true, elapsed(t, done));
            }
        }
        (done, false, elapsed(t, done))
    });
    timed_cell("lease/grant-reuse/view-build", w, |deadline| {
        //grant paid once; the 4KB view copy still rides every access
        let s = Stacks::<String>::load(&path).unwrap();
        let l = s.lease(0).unwrap();
        let t = Instant::now();
        let mut done = 0;
        for _ in 0..ACCESSES {
            let v = l.view().unwrap();
            black_box(scan_items(&v, &v.heap()));
            done += 1;
            if past(deadline, done, 128) {
                return (done, true, elapsed(t, done));
            }
        }
        (done, false, elapsed(t, done))
    });
    timed_cell("lease/held-view/scan-only", w, |deadline| {
        //the floor of local reading: one grant, one view, repeated scans
        let s = Stacks::<String>::load(&path).unwrap();
        let l = s.lease(0).unwrap();
        let v = l.view().unwrap();
        let t = Instant::now();
        let mut done = 0;
        for _ in 0..ACCESSES {
            black_box(scan_items(&v, &v.heap()));
            done += 1;
            if past(deadline, done, 128) {
                return (done, true, elapsed(t, done));
            }
        }
        (done, false, elapsed(t, done))
    });
    println!();

    // 3. eviction (per-access ns): light read (top item's prefix) alternating
    // b0/b1 — budget 1 evicts + reloads every access
    println!("== eviction (per-access ns) — 2 blocks (300 inline keys), light read, alternate b0/b1 ==");
    let ks = Lcg(0x9E37_79B9_7F4A_7C15).keys(8, 300);
    let cells = [
        ("evict/budget1/alternating", 1usize, 0),
        ("evict/budget16/alternating", 16, 0),
        ("evict/budget1/same-block", 1, 1),
    ];
    for (i, (name, budget, same)) in cells.into_iter().enumerate() {
        let path = temp_path(&format!("evict-{i}"));
        timed_cell(name, w, |deadline| {
            let s = Stacks::<String>::with_budget(&path, budget).unwrap();
            for k in &ks {
                s.push(k.clone()).unwrap();
            }
            s.flush().unwrap();
            let t = Instant::now();
            let mut done = 0;
            for j in 0..ACCESSES {
                let b = if same == 1 { 0 } else { (j & 1) as u32 };
                let c = s.read(b, |s, _| s.last().unwrap().1.pfx as usize).unwrap();
                black_box(&c);
                done += 1;
                if past(deadline, done, 128) {
                    return (done, true, elapsed(t, done));
                }
            }
            (done, false, elapsed(t, done))
        });
        let _ = std::fs::remove_file(&path);
    }
    println!();

    // 4. flush amortization: {FLUSH_N} long32 pushes (~26 records; push costs
    // already include the per-append birth-record writes), then one flush
    // of every dirty resident record; a second flush shows the clean floor
    println!("== flush amortization — {FLUSH_N} long32 pushes then flush ==");
    let path = temp_path("flush");
    for i in 0..3 {
        let s = Stacks::<String>::create(&path).unwrap();
        let ks = Lcg(0xFACE_B00C_D15E_A5E).keys(32, FLUSH_N);
        let t0 = Instant::now();
        for k in ks {
            s.push(k).unwrap();
        }
        let pushes = t0.elapsed().as_nanos() as f64 / FLUSH_N as f64;
        let t1 = Instant::now();
        s.flush().unwrap();
        let flush = t1.elapsed();
        let t2 = Instant::now();
        s.flush().unwrap();
        let clean = t2.elapsed();
        println!(
            "rep {i}: push {pushes:.1} ns/op | flush {} µs ({} ns/push amortized) | clean flush {} µs",
            flush.as_micros(), flush.as_nanos() as f64 / FLUSH_N as f64, clean.as_micros()
        );
    }
    println!();

    // 5. concurrency: T cloned handles pushing through one owner; aggregate
    // ns/item = the slowest thread's wall / total items (all run concurrently)
    println!("== concurrency (ns/item through one owner) — {THREAD_ITEMS} long32 items per thread ==");
    for threads in [1usize, 2, 4] {
        let path = temp_path(&format!("conc-{threads}"));
        timed_cell(&format!("conc/threads{threads}"), w, |deadline| {
            let s = Stacks::<String>::create(&path).unwrap();
            let bar = Arc::new(Barrier::new(threads + 1));
            let (tx, rx) = mpsc::channel::<usize>();
            for t in 0..threads {
                let (s, bar, tx) = (s.clone(), bar.clone(), tx.clone());
                let mut lcg = Lcg(0xC0FF_EE00_0000_0000 ^ t as u64);
                std::thread::spawn(move || {
                    let ks = lcg.keys(32, THREAD_ITEMS);
                    bar.wait();
                    let t0 = Instant::now();
                    let mut done = 0;
                    for k in ks {
                        s.push(k).unwrap();
                        done += 1;
                    }
                    black_box(done);
                    let wall = t0.elapsed().as_nanos() as usize;
                    let _ = tx.send(wall);
                });
            }
            drop(tx);
            bar.wait();
            let max_wall = rx.iter().max().unwrap();
            assert_eq!(s.len().unwrap(), threads * THREAD_ITEMS);
            black_box(max_wall);
            if Instant::now() > *deadline {
                return (threads * THREAD_ITEMS, true, max_wall as f64 / (threads * THREAD_ITEMS) as f64);
            }
            (threads * THREAD_ITEMS, false, max_wall as f64 / (threads * THREAD_ITEMS) as f64)
        });
        let _ = std::fs::remove_file(&path);
    }

    for p in ["read", "flush", "push-short8", "push-long32"] {
        let _ = std::fs::remove_file(temp_path(p));
    }

    std::io::stdout().flush().unwrap();
}