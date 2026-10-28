//!Stacks integration tests (task 6). The thread-spawning test is gated off
//!miri; every test needs a file — run the miri subsetting with
//!`MIRIFLAGS=-Zmiri-disable-isolation cargo +nightly miri test -p block --test stacks`

use std::cmp::Ordering;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;

use block::{LoadErr, Ptr, RelOrd, Stacks, StacksErr};

///u64 unit = snap 4 + pad 4 + handle 8 = 16 bytes; 255 per record
const PER_BLOCK: usize = 255;

fn tmp_path(name: &str) -> PathBuf {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir()
        .join(format!("block-stacks-{name}-{}-{n}", std::process::id()))
}

fn u64_items(s: &Stacks<u64>, block: u32) -> Vec<u64> {
    s.read(block, |st, _| st.items().map(|(_, h)| *h).collect::<Vec<u64>>()).unwrap()
}

#[test]
fn push_pop_lifo_across_blocks() {
    let path = tmp_path("lifo");
    let s = Stacks::<u64>::create(&path).unwrap();
    for i in 0..600u64 {
        s.push(i).unwrap();
    }
    assert_eq!(s.len().unwrap(), 600);
    assert_eq!(s.blocks().unwrap(), 3, "255 x 2 + 90");
    assert_eq!(s.top().unwrap(), 2);
    //strict LIFO order across the block boundary
    for i in (0..600u64).rev() {
        assert_eq!(s.pop().unwrap(), Some(i), "pop {i}");
    }
    assert_eq!(s.pop().unwrap(), None);
    //emptied blocks stay; push retreats into the emptied current block
    assert_eq!(s.blocks().unwrap(), 3);
    s.push(7).unwrap();
    assert_eq!(s.top().unwrap(), 0, "push lands in the retreated block");
    assert_eq!(s.len().unwrap(), 1);
    drop(s);
    let _ = std::fs::remove_file(&path);
}

///consumer clones from N threads pushing concurrently — count integrity:
///every pushed value present exactly once across every block.
#[test]
#[cfg(not(miri))]
fn concurrent_push_integrity() {
    const THREADS: usize = 4;
    const PER: usize = 200;
    let path = tmp_path("concurrent");
    let s = Stacks::<u64>::create(&path).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|t| {
            let s = s.clone();
            let b = barrier.clone();
            std::thread::spawn(move || {
                b.wait();
                for i in 0..PER {
                    s.push((t * 1000 + i) as u64).unwrap();
                }
            })
        })
        .collect();
    drop(s); //owner exits (flushes) once the threads' clones drop too
    for h in handles {
        h.join().unwrap();
    }
    let s = Stacks::<u64>::load(&path).unwrap();
    assert_eq!(s.len().unwrap(), THREADS * PER);
    let mut got = Vec::new();
    for b in 0..s.blocks().unwrap() {
        got.extend(u64_items(&s, b));
    }
    got.sort_unstable();
    let mut expected: Vec<u64> =
        (0..THREADS as u64).flat_map(|t| (0..PER as u64).map(move |i| t * 1000 + i)).collect();
    expected.sort_unstable();
    assert_eq!(got.len(), expected.len());
    assert_eq!(got, expected);
    drop(s);
    let _ = std::fs::remove_file(&path);
}

///budget 1: a held lease refuses eviction (deferral — two blocks resident),
///releasing it lets the next request rebalance back to budget.
#[test]
fn lease_blocks_eviction_budget_one() {
    let path = tmp_path("lease-evict");
    let s = Stacks::<u64>::with_budget(&path, 1).unwrap();
    for i in 0..(PER_BLOCK + 2) as u64 {
        s.push(i).unwrap();
    }
    assert_eq!(s.blocks().unwrap(), 2);
    let lease = s.lease(0).unwrap();
    assert_eq!(lease.view().unwrap().len(), PER_BLOCK, "local view is a real copy");
    //lease(0) evicted the other block before the grant: LRU honored
    assert_eq!(s.resident_ids().unwrap(), vec![0]);
    //now pressure: a push needs block 1 — with the reader pinned, the
    //deferral leaves BOTH resident over a budget of 1
    s.push(9999).unwrap();
    let ids = s.resident_ids().unwrap();
    assert_eq!(ids.len(), 2, "eviction refused the read-locked block: {ids:?}");
    assert!(ids.contains(&0));
    //release → the next request evicts back down to budget
    drop(lease);
    s.blocks().unwrap();
    assert_eq!(s.resident_ids().unwrap(), vec![1]);
    drop(s);
    let _ = std::fs::remove_file(&path);
}

///a read of an evicted block transparently reloads — identical items after
///any eviction/reload churn at budget 1.
#[test]
fn reload_transparent_after_eviction() {
    let path = tmp_path("reload");
    let s = Stacks::<u64>::with_budget(&path, 1).unwrap();
    for i in 0..600u64 {
        s.push(i).unwrap();
    }
    let a0 = u64_items(&s, 0);
    assert_eq!(a0, (0..255).collect::<Vec<u64>>());
    let a2 = u64_items(&s, 2);
    assert_eq!(a2, (510..600).collect::<Vec<u64>>());
    //block 0 was evicted by the block-2 read — reload returns identical items
    assert_eq!(u64_items(&s, 0), a0);
    assert_eq!(u64_items(&s, 2), a2);
    drop(s);
    let _ = std::fs::remove_file(&path);
}

///drop → owner flushes on exit → reopen → contents identical, including the
///pop-erasure state (a popped item leaves no bytes in any record).
#[test]
fn reopen_roundtrip_with_pop_erasure() {
    let path = tmp_path("reopen");
    let secret = "a-secret-string-that-must-not-resurrect____";
    let s = Stacks::<String>::create(&path).unwrap();
    for i in 0..300u32 {
        s.push(format!("string-item-{i}____paddingpadding")).unwrap();
    }
    //push the secret LAST, then pop it — its erasure must leave no bytes
    let (seclen, slen) = (secret.len(), "string-item-299____paddingpadding".len());
    assert_eq!(s.push(secret.to_string()).unwrap().0, s.top().unwrap());
    assert_eq!(s.pop().unwrap().unwrap().h.1.as_usize(), seclen);
    assert_eq!(s.pop().unwrap().unwrap().h.1.as_usize(), slen);
    let snap: Vec<Vec<u8>> =
        (0..s.blocks().unwrap()).map(|b| s.lease(b).unwrap().bytes().to_vec()).collect();
    drop(s);
    //reopen: byte-identical records, same counts
    let s2 = Stacks::<String>::load(&path).unwrap();
    assert_eq!(s2.blocks().unwrap(), snap.len() as u32);
    assert_eq!(s2.len().unwrap(), 299);
    for b in 0..snap.len() as u32 {
        assert_eq!(s2.lease(b).unwrap().bytes(), snap[b as usize], "block {b}");
    }
    //no residue of the popped secret anywhere in the file
    let bytes = std::fs::read(&path).unwrap();
    assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_bytes()));
    //and the map still answers: string 298 is the new top
    let top = s2.top().unwrap();
    s2.read(top, |st, heap| {
        let (snap, h) = st.last().unwrap();
        assert_eq!(
            String::rel_cmp(h, &"string-item-298____paddingpadding".to_string(), heap),
            Ordering::Equal
        );
        snap
    })
    .unwrap();
    drop(s2);
    let _ = std::fs::remove_file(&path);
}

///a read closure that panics: the payload resumes on the caller thread and
///the owner keeps serving afterwards.
#[test]
fn closure_panic_isolation() {
    let path = tmp_path("panic");
    let s = Stacks::<u64>::create(&path).unwrap();
    s.push(11).unwrap();
    let r = panic::catch_unwind(AssertUnwindSafe(|| s.read(0, |_, _| panic!("boom"))));
    assert_eq!(r.unwrap_err().downcast_ref::<&str>(), Some(&"boom"));
    //owner unharmed
    assert_eq!(u64_items(&s, 0), vec![11]);
    s.push(12).unwrap();
    assert_eq!(s.len().unwrap(), 2);
    drop(s);
    let _ = std::fs::remove_file(&path);
}

///budget 0: every request still works (correctness over budget); the MRU
///deferral keeps exactly the just-used block resident.
#[test]
fn budget_zero_degenerate() {
    let path = tmp_path("budget0");
    let s = Stacks::<u64>::with_budget(&path, 0).unwrap();
    for i in 0..10u64 {
        s.push(i).unwrap();
    }
    assert_eq!(s.len().unwrap(), 10);
    //only the MRU (block 0) survives eviction at budget 0
    assert_eq!(s.resident_ids().unwrap(), vec![0]);
    assert_eq!(u64_items(&s, 0), (0..10).collect::<Vec<u64>>());
    drop(s); //write-back happens at owner exit
    let s2 = Stacks::<u64>::load(&path).unwrap();
    assert_eq!(s2.len().unwrap(), 10);
    drop(s2);
    let _ = std::fs::remove_file(&path);
}

///an item larger than a whole record errs `Cap` after appending one fresh
///(empty) record; following pushes land there and everything stays readable.
#[test]
fn oversize_item_is_cap_err() {
    let path = tmp_path("oversize");
    let s = Stacks::<String>::create(&path).unwrap();
    s.push("fits".into()).unwrap();
    match s.push("x".repeat(5000)) {
        Err(StacksErr::Cap(_)) => {}
        other => panic!("expected Cap, got {other:?}"),
    }
    assert_eq!(s.blocks().unwrap(), 2, "the spill record exists (empty)");
    assert_eq!(s.len().unwrap(), 1);
    assert_eq!(s.top().unwrap(), 1, "pushes continue in the fresh block");
    s.push("after".into()).unwrap();
    assert_eq!(s.len().unwrap(), 2);
    assert_eq!(s.read(0, |st, _| st.len()).unwrap(), 1);
    assert_eq!(s.read(1, |st, _| st.len()).unwrap(), 1);
    drop(s);
    let _ = std::fs::remove_file(&path);
}

///structural load errors are clean, never panics: truncation → StrideMismatch,
///bad cap headers → BadCap; an empty file is a valid 0-record stacks.
#[test]
fn load_rejects_corrupt_files() {
    //empty file: 0 records
    let path = tmp_path("empty");
    std::fs::File::create(&path).unwrap();
    let s = Stacks::<u64>::load(&path).unwrap();
    assert_eq!(s.blocks().unwrap(), 0);
    drop(s);
    let _ = std::fs::remove_file(&path);

    //truncated mid-record (sane cap header, length not a whole record)
    let path = tmp_path("truncated");
    let mut rec = vec![0u8; 4090];
    rec[8..12].copy_from_slice(&4080u32.to_le_bytes());
    std::fs::write(&path, rec).unwrap();
    let e = Stacks::<u64>::load(&path).err().unwrap();
    assert!(
        matches!(e, StacksErr::Load(LoadErr::StrideMismatch { file: 4090, stride: 4096 })),
        "{e:?}"
    );
    let _ = std::fs::remove_file(&path);

    //cap header below 16
    let path = tmp_path("badcap");
    let mut rec = vec![0u8; 256];
    rec[8..12].copy_from_slice(&17u32.to_le_bytes());
    std::fs::write(&path, rec).unwrap();
    let e = Stacks::<u64>::load(&path).err().unwrap();
    assert!(matches!(e, StacksErr::Load(LoadErr::BadCap { cap: 17 })), "{e:?}");
    let _ = std::fs::remove_file(&path);

    //cap header beyond the pointer width (u16 pointers: 4096 caps out)
    let path = tmp_path("widecap");
    let mut rec = vec![0u8; 8192];
    rec[8..12].copy_from_slice(&65552u32.to_le_bytes());
    std::fs::write(&path, rec).unwrap();
    let e = Stacks::<u64, u16>::load(&path).err().unwrap();
    assert!(matches!(e, StacksErr::Load(LoadErr::BadCap { cap: 65552 })), "{e:?}");
    let _ = std::fs::remove_file(&path);

    //missing file
    let e = Stacks::<u64>::load(tmp_path("missing")).err().unwrap();
    assert!(matches!(e, StacksErr::Load(LoadErr::Io(_))), "{e:?}");
}