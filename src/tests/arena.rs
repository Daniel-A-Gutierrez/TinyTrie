use super::*;

#[test]
fn alloc_and_get() {
    let mut arena: Arena<[u32; 2], u16> = Arena::new();
    let a = arena.alloc([10, 20]);
    let b = arena.alloc([30, 40]);
    let c = arena.alloc([50, 60]);
    assert_eq!(*arena.get(a), [10, 20]);
    assert_eq!(*arena.get(b), [30, 40]);
    assert_eq!(*arena.get(c), [50, 60]);
    assert_eq!(arena.len(), 3);
}

#[test]
fn free_and_reuse() {
    let mut arena: Arena<[u32; 2], u16> = Arena::new();
    let _a = arena.alloc([10, 20]);
    let b = arena.alloc([30, 40]);
    let _c = arena.alloc([50, 60]);
    arena.free(b);
    assert_eq!(arena.len(), 2);

    let d = arena.alloc([99, 88]); // reuses b
    assert_eq!(d, b);
    assert_eq!(*arena.get(d), [99, 88]);
    assert_eq!(arena.len(), 3);
}

#[test]
fn stable_indices() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    let a = arena.alloc([1, 2]);
    let b = arena.alloc([3, 4]);
    let _c = arena.alloc([5, 6]);
    arena.free(b);
    // a is still valid — freeing b doesn't shift anything
    assert_eq!(*arena.get(a), [1, 2]);
}

#[test]
fn free_chain() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    let a = arena.alloc([10, 0]);
    let _b = arena.alloc([20, 0]);
    let c = arena.alloc([30, 0]);
    arena.free(a);
    arena.free(c);
    // Free list: c -> a (LIFO)
    let d = arena.alloc([100, 0]); // reuses c
    let e = arena.alloc([200, 0]); // reuses a
    assert_eq!(d, c);
    assert_eq!(e, a);
    assert_eq!(*arena.get(d), [100, 0]);
    assert_eq!(*arena.get(e), [200, 0]);
    assert_eq!(arena.len(), 3); // _b, d, e
}

#[test]
fn u16_index_type() {
    let mut arena: Arena<[u32; 2], u16> = Arena::new();
    let idx = arena.alloc([42, 0]);
    assert_eq!(idx.to_usize(), 0);
    assert_eq!(*arena.get(idx), [42, 0]);
}

#[test]
fn with_capacity() {
    let mut arena: Arena<[u32; 2], u32> = Arena::with_capacity(100);
    for i in 0..100u32 {
        arena.alloc([i, i + 1]);
    }
    assert_eq!(arena.len(), 100);
}

#[test]
fn alloc_n_basic() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    let start = arena.alloc_n(4, [0xFF, 0xAA]);
    assert_eq!(start.to_usize(), 0);
    for i in 0..4 {
        assert_eq!(*arena.get(u32::from_usize(i)), [0xFF, 0xAA]);
    }
    assert_eq!(arena.len(), 4);
    assert_eq!(arena.capacity(), 4);
}

#[test]
fn alloc_n_after_singles() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    let a = arena.alloc([1, 2]);  // index 0
    let b = arena.alloc([3, 4]);  // index 1
    let start = arena.alloc_n(3, [9, 9]); // indices 2, 3, 4
    assert_eq!(a.to_usize(), 0);
    assert_eq!(b.to_usize(), 1);
    assert_eq!(start.to_usize(), 2);
    assert_eq!(*arena.get(a), [1, 2]);
    assert_eq!(*arena.get(b), [3, 4]);
    for i in 2..=4 {
        assert_eq!(*arena.get(u32::from_usize(i)), [9, 9]);
    }
    assert_eq!(arena.len(), 5);
}

#[test]
fn alloc_n_u16_overflow() {
    let mut arena: Arena<[u32; 2], u16> = Arena::new();
    // Fill up to near u16 max
    for _ in 0..65534 {
        arena.alloc([0, 0]);
    }
    assert_eq!(arena.len(), 65534);
    // Can still alloc_n a small range
    let s = arena.alloc_n(2, [1, 1]); // indices 65534, 65535
    assert_eq!(s.to_usize(), 65534);
    assert_eq!(arena.len(), 65536);
    // Next alloc_n should panic (overflow)
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut a: Arena<[u32; 2], u16> = Arena::new();
        for _ in 0..65535 {
            a.alloc([0, 0]);
        }
        a.alloc_n(2, [0, 0]); // would need index 65536, which overflows u16
    }));
    assert!(result.is_err());
}

#[test]
fn alloc_slice_basic() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    let values: Vec<[u32; 2]> = (0..5).map(|i| [i, i * 10]).collect();
    let start = arena.alloc_slice(&values);
    assert_eq!(start.to_usize(), 0);
    for i in 0..5 {
        assert_eq!(*arena.get(u32::from_usize(i)), [i as u32, i as u32 * 10]);
    }
    assert_eq!(arena.len(), 5);
}

#[test]
fn alloc_slice_after_alloc() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    let first = arena.alloc([100, 200]);
    let values: Vec<[u32; 2]> = (0..3).map(|i| [i, i + 1]).collect();
    let start = arena.alloc_slice(&values);
    assert_eq!(first.to_usize(), 0);
    assert_eq!(start.to_usize(), 1);
    assert_eq!(*arena.get(first), [100, 200]);
    for i in 0..3 {
        assert_eq!(*arena.get(u32::from_usize(1 + i)), [i as u32, i as u32 + 1]);
    }
}

#[test]
fn free_n_basic() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    let start = arena.alloc_n(4, [42, 0]);
    let single = arena.alloc([99, 88]);
    assert_eq!(arena.len(), 5);

    arena.free_n(start, 4);
    assert_eq!(arena.len(), 1);
    // single allocation is still valid
    assert_eq!(*arena.get(single), [99, 88]);
}

#[test]
fn free_n_then_alloc_n_reuse() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    // Allocate a block of 3, then free it.
    let block = arena.alloc_n(3, [0, 0]); // indices 0, 1, 2
    assert_eq!(arena.len(), 3);
    assert_eq!(arena.capacity(), 3);

    arena.free_n(block, 3);
    assert_eq!(arena.len(), 0);
    assert_eq!(arena.capacity(), 3); // slots still present

    // alloc_n(3, …) should reuse the freed block at index 0.
    let reused = arena.alloc_n(3, [7, 7]);
    assert_eq!(reused, block); // same start index
    for i in 0..3 {
        assert_eq!(*arena.get(u32::from_usize(i)), [7, 7]);
    }
    assert_eq!(arena.len(), 3);
    assert_eq!(arena.capacity(), 3); // didn't grow
}

#[test]
fn free_n_then_alloc_slice_reuse() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    let values: Vec<[u32; 2]> = (0..4).map(|i| [i, i * 10]).collect();
    let block = arena.alloc_slice(&values);
    arena.free_n(block, 4);

    // alloc_slice of same size reuses the block.
    let new_vals: Vec<[u32; 2]> = (0..4).map(|i| [i * 100, i]).collect();
    let reused = arena.alloc_slice(&new_vals);
    assert_eq!(reused, block);
    for i in 0..4 {
        assert_eq!(*arena.get(u32::from_usize(i)), [i as u32 * 100, i as u32]);
    }
    assert_eq!(arena.capacity(), 4); // didn't grow
}

#[test]
fn free_n_different_sizes_no_cross_reuse() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    // Allocate blocks of size 3 and 5.
    let block3 = arena.alloc_n(3, [1, 1]);
    let block5 = arena.alloc_n(5, [2, 2]);

    arena.free_n(block3, 3);
    arena.free_n(block5, 5);

    // alloc_n(4, …) should NOT reuse size-3 or size-5 blocks — must append.
    let block4 = arena.alloc_n(4, [3, 3]);
    assert_eq!(block4.to_usize(), 8); // appended after existing slots
    assert_eq!(arena.capacity(), 12); // 3 + 5 + 4, no reuse

    // alloc_n(3, …) reuses the freed size-3 block.
    let reused3 = arena.alloc_n(3, [4, 4]);
    assert_eq!(reused3, block3);

    // alloc_n(5, …) reuses the freed size-5 block.
    let reused5 = arena.alloc_n(5, [5, 5]);
    assert_eq!(reused5, block5);
}

#[test]
fn free_n_multiple_blocks_same_size() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    let a = arena.alloc_n(4, [1, 1]); // 0..4
    let b = arena.alloc_n(4, [2, 2]); // 4..8
    let c = arena.alloc_n(4, [3, 3]); // 8..12

    // Free all three — they form a LIFO chain on the size-4 free list.
    arena.free_n(a, 4);
    arena.free_n(b, 4);
    arena.free_n(c, 4);

    // alloc_n(4) reuses in LIFO order: c first, then b, then a.
    let first = arena.alloc_n(4, [10, 10]);
    assert_eq!(first, c); // most recently freed
    let second = arena.alloc_n(4, [20, 20]);
    assert_eq!(second, b);
    let third = arena.alloc_n(4, [30, 30]);
    assert_eq!(third, a);
}

#[test]
fn free_n_partial_range() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    let _a = arena.alloc([1, 0]);  // 0
    let start = arena.alloc_n(3, [2, 0]); // 1, 2, 3
    let _b = arena.alloc([4, 0]);  // 4
    assert_eq!(arena.len(), 5);

    arena.free_n(start, 3); // free indices 1, 2, 3 as a block
    assert_eq!(arena.len(), 2);

    // Surrounding allocations still valid
    assert_eq!(*arena.get(u32::from_usize(0)), [1, 0]);
    assert_eq!(*arena.get(u32::from_usize(4)), [4, 0]);

    // alloc_n(3, …) reuses the freed block.
    let reused = arena.alloc_n(3, [55, 0]);
    assert_eq!(reused, start);
    for i in 0..3 {
        assert_eq!(*arena.get(u32::from_usize(1 + i)), [55, 0]);
    }
}

#[test]
fn free_n_mixed_single_and_block() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    // Single-slot free list and block free lists are independent.
    let single = arena.alloc([1, 0]); // 0
    let block = arena.alloc_n(3, [2, 0]); // 1, 2, 3
    let _other = arena.alloc([5, 0]); // 4

    arena.free(single); // goes to single-slot free list
    arena.free_n(block, 3); // goes to block[3] free list

    // Single alloc reuses the freed single slot.
    let reused_single = arena.alloc([10, 0]);
    assert_eq!(reused_single, single);

    // Block alloc_n(3) reuses the freed block.
    let reused_block = arena.alloc_n(3, [20, 0]);
    assert_eq!(reused_block, block);
}

#[test]
#[should_panic(expected = "cannot allocate 0 slots")]
fn alloc_n_zero_panics() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    arena.alloc_n(0, [0, 0]);
}

#[test]
#[should_panic(expected = "cannot allocate 0 slots")]
fn alloc_slice_empty_panics() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    arena.alloc_slice(&[]);
}

#[test]
#[should_panic(expected = "cannot free 0 slots")]
fn free_n_zero_panics() {
    let mut arena: Arena<[u32; 2], u32> = Arena::new();
    let s = arena.alloc([1, 2]);
    arena.free_n(s, 0);
}

#[test]
fn get_range_basic() {
    let mut arena: Arena<u32, u32> = Arena::new();
    let start = arena.alloc_n(4, 0);
    // Mutate through get_range_mut
    let slice = arena.get_range_mut(start, 4);
    slice[0] = 10;
    slice[1] = 20;
    slice[2] = 30;
    slice[3] = 40;
    // Read through get_range
    let slice = arena.get_range(start, 4);
    assert_eq!(slice, &[10, 20, 30, 40]);
}

#[test]
fn get_range_after_singles() {
    let mut arena: Arena<u32, u32> = Arena::new();
    let _a = arena.alloc(100); // index 0
    let start = arena.alloc_n(3, 0); // indices 1, 2, 3
    let slice = arena.get_range_mut(start, 3);
    slice[0] = 1;
    slice[1] = 2;
    slice[2] = 3;
    // Verify single slot is unaffected
    assert_eq!(*arena.get(_a), 100);
    // Verify range
    assert_eq!(arena.get_range(start, 3), &[1, 2, 3]);
}

#[test]
fn get_range_with_u16_index() {
    let mut arena: Arena<u16, u16> = Arena::new();
    let start = arena.alloc_n(5, 99);
    let slice = arena.get_range_mut(start, 5);
    for (i, slot) in slice.iter_mut().enumerate() {
        *slot = i as u16;
    }
    assert_eq!(arena.get_range(start, 5), &[0, 1, 2, 3, 4]);
}

#[test]
fn get_range_on_reused_block() {
    let mut arena: Arena<u32, u32> = Arena::new();
    let block = arena.alloc_n(4, 0);
    arena.free_n(block, 4);
    let reused = arena.alloc_n(4, 0);
    assert_eq!(reused, block);
    // Mutate via get_range_mut on the reused block.
    let slice = arena.get_range_mut(reused, 4);
    slice[0] = 10;
    slice[1] = 20;
    slice[2] = 30;
    slice[3] = 40;
    assert_eq!(arena.get_range(reused, 4), &[10, 20, 30, 40]);
}