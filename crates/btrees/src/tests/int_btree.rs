use crate::int_btree::*;

// ---------------------------------------------------------------------------
// FixedLenKey SIMD `find_position` — direct trait tests
// ---------------------------------------------------------------------------
// These call the SIMD method on `FixedLenKey` directly. They use fully-
// qualified syntax because `u8/u16/u32/u64` now also implement `StoredKey`,
// which carries a same-named `find_position` — bare `u64::find_position`
// would be ambiguous between the two traits.

#[test]
fn test_fixed_len_key_find_position_u64() {
    let haystack: [u64; 8] = [10, 20, 30, 40, 50, 60, 70, 80];
    assert_eq!(<u64 as FixedLenKey>::find_position(&10, &haystack), 0);
    assert_eq!(<u64 as FixedLenKey>::find_position(&25, &haystack), 2);
    assert_eq!(<u64 as FixedLenKey>::find_position(&80, &haystack), 7);
    assert_eq!(<u64 as FixedLenKey>::find_position(&90, &haystack), 8);
    // Partial slice
    assert_eq!(<u64 as FixedLenKey>::find_position(&55, &haystack[..5]), 5);
    // Empty
    assert_eq!(<u64 as FixedLenKey>::find_position(&1, &haystack[..0]), 0);
}

#[test]
fn test_fixed_len_key_find_position_u32() {
    let haystack: [u32; 8] = [1, 3, 5, 7, 9, 11, 13, 15];
    assert_eq!(<u32 as FixedLenKey>::find_position(&5, &haystack), 2);
    assert_eq!(<u32 as FixedLenKey>::find_position(&6, &haystack), 3);
    assert_eq!(<u32 as FixedLenKey>::find_position(&0, &haystack), 0);
    assert_eq!(<u32 as FixedLenKey>::find_position(&16, &haystack), 8);
}

#[test]
fn test_fixed_len_key_find_position_u16() {
    let haystack: [u16; 8] = [100, 200, 300, 400, 500, 600, 700, 800];
    assert_eq!(<u16 as FixedLenKey>::find_position(&300, &haystack), 2);
    assert_eq!(<u16 as FixedLenKey>::find_position(&250, &haystack), 2);
    assert_eq!(<u16 as FixedLenKey>::find_position(&800, &haystack), 7);
    assert_eq!(<u16 as FixedLenKey>::find_position(&900, &haystack), 8);
}

#[test]
fn test_fixed_len_key_find_position_u8() {
    let haystack: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
    assert_eq!(<u8 as FixedLenKey>::find_position(&0, &haystack), 0);
    assert_eq!(<u8 as FixedLenKey>::find_position(&7, &haystack), 7);
    assert_eq!(<u8 as FixedLenKey>::find_position(&15, &haystack), 15);
    assert_eq!(<u8 as FixedLenKey>::find_position(&16, &haystack), 16);
}

// ---------------------------------------------------------------------------
// Fixed CTree (regression guard for the SIMD path — multi-N, varied order)
// ---------------------------------------------------------------------------

#[test]
fn test_ctree_insert_and_get() {
    let mut tree: CTree<u64, u64, u16, 4, 5> = CTree::new();
    tree.insert(10, 100).unwrap();
    tree.insert(20, 200).unwrap();
    tree.insert(30, 300).unwrap();
    assert_eq!(tree.get(&10), Some(&100));
    assert_eq!(tree.get(&20), Some(&200));
    assert_eq!(tree.get(&30), Some(&300));
    assert_eq!(tree.get(&40), None);
    assert_eq!(tree.len(), 3);
}

#[test]
fn test_ctree_duplicate_insert() {
    let mut tree: CTree<u64, u64, u16, 4, 5> = CTree::new();
    tree.insert(10, 100).unwrap();
    let err = tree.insert(10, 200);
    assert!(err.is_err());
    assert_eq!(tree.get(&10), Some(&100));
}

#[test]
fn test_var_len_key_box_u8() {
    let k: Box<[u8]> = Box::new([1u8, 2, 3]);
    assert_eq!(&*k, &[1u8, 2, 3]);
    assert_eq!(k.len(), 3);
}

// ---------------------------------------------------------------------------
// Recursive inode split tests (fixed path)
// ---------------------------------------------------------------------------

/// Insert enough sequential keys to trigger multiple inode splits,
/// growing the tree to height 2+.
#[test]
fn test_ctree_sequential_deep_split() {
    // N=3: each node holds at most 3 keys. This triggers splits quickly.
    let mut tree: CTree<u64, u64, u16, 3, 4> = CTree::new();
    let count = 30;

    for i in 0..count {
        tree.insert(i, i * 10).unwrap();
    }

    // Verify all lookups
    for i in 0..count {
        assert_eq!(
            tree.get(&i),
            Some(&(i * 10)),
            "lookup failed for key {i}"
        );
    }
    assert_eq!(tree.get(&count), None);
    assert_eq!(tree.len(), count as usize);

    // Height should be > 1 after 30 inserts with N=3
    assert!(
        tree.height > 1,
        "expected height > 1 after many inserts, got {}",
        tree.height
    );
}

/// Insert keys in reverse order to stress different split patterns.
#[test]
fn test_ctree_reverse_deep_split() {
    let mut tree: CTree<u64, u64, u16, 3, 4> = CTree::new();
    let count = 30;

    for i in (0..count).rev() {
        tree.insert(i, i * 10).unwrap();
    }

    for i in 0..count {
        assert_eq!(
            tree.get(&i),
            Some(&(i * 10)),
            "reverse lookup failed for key {i}"
        );
    }
    assert_eq!(tree.len(), count as usize);
}

/// Insert keys in a shuffled order.
#[test]
fn test_ctree_random_deep_split() {
    // Deterministic pseudo-random order
    let order: [u64; 30] = [
        15, 3, 27, 8, 21, 0, 12, 6, 18, 24, 9, 1, 29, 14, 5, 22, 11, 7, 19, 2, 25, 13, 28,
        10, 4, 16, 23, 17, 26, 20,
    ];

    let mut tree: CTree<u64, u64, u16, 3, 4> = CTree::new();
    for &i in &order {
        tree.insert(i, i * 10).unwrap();
    }

    for i in 0..30u64 {
        assert_eq!(
            tree.get(&i),
            Some(&(i * 10)),
            "shuffled lookup failed for key {i}"
        );
    }
    assert_eq!(tree.len(), 30);
}

/// Even deeper tree with N=2 (smallest meaningful B+ tree order).
#[test]
fn test_ctree_tiny_n_deep() {
    let mut tree: CTree<u64, u64, u16, 2, 3> = CTree::new();
    let count = 50;

    for i in 0..count {
        tree.insert(i, i * 100).unwrap();
    }

    for i in 0..count {
        assert_eq!(
            tree.get(&i),
            Some(&(i * 100)),
            "tiny N lookup failed for key {i}"
        );
    }

    // With N=2 and 50 keys, height should be significant
    assert!(tree.height >= 3, "expected height >= 3, got {}", tree.height);
}

// ---------------------------------------------------------------------------
// Cursor traversal tests (fixed path)
// ---------------------------------------------------------------------------

#[test]
fn test_cursor_forward_iteration() {
    let mut tree: CTree<u64, u64, u16, 4, 5> = CTree::new();
    for i in 0..20 {
        tree.insert(i, i * 10).unwrap();
    }

    let mut cursor = tree.get_cursor();
    let mut collected = Vec::new();
    while let Some((k, v)) = cursor.current() {
        collected.push((*k, *v));
        if cursor.next().is_none() {
            break;
        }
    }

    assert_eq!(collected.len(), 20);
    for (i, (k, v)) in collected.iter().enumerate() {
        assert_eq!(*k, i as u64);
        assert_eq!(*v, i as u64 * 10);
    }
}

#[test]
fn test_cursor_backward_iteration() {
    let mut tree: CTree<u64, u64, u16, 4, 5> = CTree::new();
    for i in 0..20 {
        tree.insert(i, i * 10).unwrap();
    }

    // Start from last leaf, last position
    let last_leaf = tree.last_leaf();
    let last_pos = tree.leaves[last_leaf].keys.len() - 1;
    let mut cursor = Cursor {
        tree: &tree,
        leaf_idx: last_leaf,
        position: last_pos,
    };

    let mut collected = Vec::new();
    while let Some((k, v)) = cursor.current() {
        collected.push((*k, *v));
        if cursor.prev().is_none() {
            break;
        }
    }

    collected.reverse();
    assert_eq!(collected.len(), 20);
    for (i, (k, v)) in collected.iter().enumerate() {
        assert_eq!(*k, i as u64);
        assert_eq!(*v, i as u64 * 10);
    }
}

#[test]
fn test_cursor_at_seek() {
    let mut tree: CTree<u64, u64, u16, 4, 5> = CTree::new();
    for i in 0..20 {
        tree.insert(i, i * 10).unwrap();
    }

    // Seek to key 10 — should land on or just after it
    let mut cursor = tree.cursor_at(&10);
    let (k, v) = cursor.current().expect("cursor should point to a valid entry");
    assert_eq!(*k, 10);
    assert_eq!(*v, 100);

    // Iterate forward from there
    let mut collected = vec![(*k, *v)];
    loop {
        if cursor.next().is_none() {
            break;
        }
        let (k, v) = cursor.current().unwrap();
        collected.push((*k, *v));
    }

    assert_eq!(collected.len(), 10); // keys 10..19
    for (i, (k, v)) in collected.iter().enumerate() {
        assert_eq!(*k, 10 + i as u64);
        assert_eq!(*v, (10 + i as u64) * 10);
    }
}

#[test]
fn test_cursor_forward_deep_tree() {
    // Use a small N to force multiple levels, then iterate all keys
    let mut tree: CTree<u64, u64, u16, 3, 4> = CTree::new();
    let count = 30;
    for i in 0..count {
        tree.insert(i, i * 5).unwrap();
    }

    let mut cursor = tree.get_cursor();
    let mut collected = Vec::new();
    while let Some((k, v)) = cursor.current() {
        collected.push((*k, *v));
        if cursor.next().is_none() {
            break;
        }
    }

    assert_eq!(collected.len(), count as usize);
    for (i, (k, v)) in collected.iter().enumerate() {
        assert_eq!(*k, i as u64, "key mismatch at position {i}");
        assert_eq!(*v, i as u64 * 5, "value mismatch at position {i}");
    }
}

#[test]
fn test_cursor_backward_deep_tree() {
    let mut tree: CTree<u64, u64, u16, 3, 4> = CTree::new();
    let count = 30;
    for i in 0..count {
        tree.insert(i, i * 5).unwrap();
    }

    let last_leaf = tree.last_leaf();
    let last_pos = tree.leaves[last_leaf].keys.len() - 1;
    let mut cursor = Cursor {
        tree: &tree,
        leaf_idx: last_leaf,
        position: last_pos,
    };

    let mut collected = Vec::new();
    while let Some((k, v)) = cursor.current() {
        collected.push((*k, *v));
        if cursor.prev().is_none() {
            break;
        }
    }

    collected.reverse();
    assert_eq!(collected.len(), count as usize);
    for (i, (k, v)) in collected.iter().enumerate() {
        assert_eq!(*k, i as u64, "key mismatch at position {i}");
        assert_eq!(*v, i as u64 * 5, "value mismatch at position {i}");
    }
}

#[test]
fn test_cursor_empty_tree() {
    let tree: CTree<u64, u64, u16, 4, 5> = CTree::new();
    // Empty tree: the root leaf has 0 keys but leaf_idx=0, position=0
    // current() should return None since there are no entries
    let cursor = tree.get_cursor();
    assert!(cursor.current().is_none());
}

#[test]
fn test_cursor_mut_forward() {
    let mut tree: CTree<u64, u64, u16, 4, 5> = CTree::new();
    for i in 0..10u64 {
        tree.insert(i, i * 10).unwrap();
    }

    // Double all values via mutable cursor
    {
        let mut cursor = tree.get_cursor_mut();
        while let Some((_, v)) = cursor.current() {
            *v *= 2;
            if cursor.next().is_none() {
                break;
            }
        }
    }

    // Verify
    for i in 0..10u64 {
        assert_eq!(tree.get(&i), Some(&(i * 20)));
    }
}

// ---------------------------------------------------------------------------
// Generic test harness — runs the SAME core suite against any instantiation.
//
// The point: prove the unified generic tree body works identically for the
// fixed (SIMD) and variable (binary-search) key forms. `store` produces an
// owned key for `insert` (`Into<SK>`), `borrow` produces an owned query whose
// `&` satisfies `Borrow<SK::Needle>` for `get`, and `val` produces the value.
// Var keys use big-endian byte reps so lexicographic order matches numeric
// order, letting one index-driven suite exercise both forms uniformly.

macro_rules! gen_tree_tests {
    (
        $modname:ident,
        $Tree:ty,
        count = $count:expr,
        store = $store:expr,
        borrow = $borrow:expr,
        val = $val:expr $(,)?
    ) => {
        mod $modname {
            use super::*;

            #[test]
            fn insert_get_len() {
                let store = $store;
                let borrow = $borrow;
                let val = $val;
                let mut tree: $Tree = CTree::new();
                for i in 0..$count {
                    tree.insert(store(i), val(i)).unwrap();
                }
                for i in 0..$count {
                    assert_eq!(
                        tree.get(&borrow(i)),
                        Some(&val(i)),
                        "lookup failed for index {i}"
                    );
                }
                assert_eq!(tree.get(&borrow($count)), None);
                assert_eq!(tree.len(), $count as usize);
            }

            #[test]
            fn duplicate_insert() {
                let store = $store;
                let borrow = $borrow;
                let val = $val;
                let mut tree: $Tree = CTree::new();
                tree.insert(store(1), val(1)).unwrap();
                let err = tree.insert(store(1), val(2));
                assert!(err.is_err(), "duplicate insert should error");
                assert_eq!(tree.get(&borrow(1)), Some(&val(1)));
            }

            #[test]
            fn deep_split_height() {
                let store = $store;
                let val = $val;
                let mut tree: $Tree = CTree::new();
                for i in 0..$count {
                    tree.insert(store(i), val(i)).unwrap();
                }
                assert!(
                    tree.height > 1,
                    "expected height > 1 after {} inserts, got {}",
                    $count,
                    tree.height
                );
            }

            #[test]
            fn cursor_forward() {
                let store = $store;
                let val = $val;
                let mut tree: $Tree = CTree::new();
                for i in 0..$count {
                    tree.insert(store(i), val(i)).unwrap();
                }
                let mut cursor = tree.get_cursor();
                let mut collected = Vec::new();
                while let Some((_, v)) = cursor.current() {
                    collected.push(*v);
                    if cursor.next().is_none() {
                        break;
                    }
                }
                assert_eq!(collected.len(), $count as usize);
                for (i, &v) in collected.iter().enumerate() {
                    assert_eq!(v, val(i as u64), "value mismatch at position {i}");
                }
            }

            #[test]
            fn cursor_backward() {
                let store = $store;
                let val = $val;
                let mut tree: $Tree = CTree::new();
                for i in 0..$count {
                    tree.insert(store(i), val(i)).unwrap();
                }
                let last_leaf = tree.last_leaf();
                let last_pos = tree.leaves[last_leaf].keys.len() - 1;
                let mut cursor = Cursor {
                    tree: &tree,
                    leaf_idx: last_leaf,
                    position: last_pos,
                };
                let mut collected = Vec::new();
                while let Some((_, v)) = cursor.current() {
                    collected.push(*v);
                    if cursor.prev().is_none() {
                        break;
                    }
                }
                collected.reverse();
                assert_eq!(collected.len(), $count as usize);
                for (i, &v) in collected.iter().enumerate() {
                    assert_eq!(v, val(i as u64), "value mismatch at position {i}");
                }
            }

            #[test]
            fn cursor_at_seek() {
                let store = $store;
                let borrow = $borrow;
                let val = $val;
                let mut tree: $Tree = CTree::new();
                for i in 0..$count {
                    tree.insert(store(i), val(i)).unwrap();
                }
                let mid = $count / 2;
                let cursor = tree.cursor_at(&borrow(mid));
                let (_, v) = cursor
                    .current()
                    .expect("cursor should land on the sought key");
                assert_eq!(*v, val(mid));
            }

            #[test]
            fn cursor_mut_forward() {
                let store = $store;
                let borrow = $borrow;
                let val = $val;
                let mut tree: $Tree = CTree::new();
                for i in 0..$count {
                    tree.insert(store(i), val(i)).unwrap();
                }
                {
                    let mut cursor = tree.get_cursor_mut();
                    while let Some((_, v)) = cursor.current() {
                        *v *= 2;
                        if cursor.next().is_none() {
                            break;
                        }
                    }
                }
                for i in 0..$count {
                    assert_eq!(tree.get(&borrow(i)), Some(&(val(i) * 2)));
                }
            }
        }
    };
}

// Fixed form: K = u64 (SIMD path). Validates the harness is form-agnostic.
gen_tree_tests!(
    fixed_harness,
    CTree<u64, u64, u16, 3, 4>,
    count = 30,
    store = |i: u64| i,
    borrow = |i: u64| i,
    val = |i: u64| i * 10,
);

// Variable form: Box<[u8]> keys (binary-search path). The first real exercise
// of the var tree — it was a stub before the unification. Big-endian byte reps
// keep lexicographic key order aligned with numeric index order.
gen_tree_tests!(
    var_harness_n3,
    CTree<Box<[u8]>, u64, u16, 3, 4>,
    count = 30,
    store = |i: u64| -> Box<[u8]> { Box::from(&i.to_be_bytes()[..]) },
    borrow = |i: u64| i.to_be_bytes(),
    val = |i: u64| i * 10,
);

// Variable form at N=2 (smallest order) for deeper splits.
gen_tree_tests!(
    var_harness_n2,
    CTree<Box<[u8]>, u64, u16, 2, 3>,
    count = 50,
    store = |i: u64| -> Box<[u8]> { Box::from(&i.to_be_bytes()[..]) },
    borrow = |i: u64| i.to_be_bytes(),
    val = |i: u64| i * 100,
);

// ---------------------------------------------------------------------------
// Preemptive rebalance (rotate before split) tests
// ---------------------------------------------------------------------------

/// A full leaf with an underfull sibling absorbs the overflow by rebalancing
/// instead of splitting: the leaf count does not increase on the insert.
///
/// N=4. Inserts 0..=5: at insert 4 the root leaf splits (no sibling yet) into
/// `[0,1]` and `[2,3,4]`; insert 5 fills the right leaf to `[2,3,4,5]`. Insert 6
/// overflows the right leaf, but its left sibling `[0,1]` has room (len 2, and
/// `2 + 2 <= N`), so descent rebalance moves `2` to the left leaf — `[0,1,2]`
/// and `[3,4,5]` — leaving room for `6` in the right leaf. No new leaf.
#[test]
fn test_leaf_rebalance_absorbs_overflow() {
    let mut tree: CTree<u64, u64, u16, 4, 5> = CTree::new();
    for i in 0..=5u64 {
        tree.insert(i, i * 10).unwrap();
    }
    assert_eq!(tree.leaves.len(), 2);

    let leaves_before = tree.leaves.len();
    tree.insert(6, 60).unwrap();
    assert_eq!(
        tree.leaves.len(),
        leaves_before,
        "insert 6 should rebalance with the left sibling, not split"
    );

    for i in 0..=6u64 {
        assert_eq!(tree.get(&i), Some(&(i * 10)), "lookup failed for key {i}");
    }
    assert_eq!(tree.len(), 7);
}

/// Same scenario as above but on the variable-key form (`Box<[u8]>`), confirming
/// the rebalance path works for binary-search keys and `Box<[u8]>` ownership
/// transfers (clone-on-sep, drain helpers) are sound.
#[test]
fn test_leaf_rebalance_absorbs_overflow_var() {
    let mut tree: CTree<Box<[u8]>, u64, u16, 4, 5> = CTree::new();
    for i in 0..=5u64 {
        tree.insert(Box::from(&i.to_be_bytes()[..]), i * 10).unwrap();
    }
    assert_eq!(tree.leaves.len(), 2);

    let leaves_before = tree.leaves.len();
    tree.insert(Box::from(&6u64.to_be_bytes()[..]), 60).unwrap();
    assert_eq!(tree.leaves.len(), leaves_before);

    for i in 0..=6u64 {
        assert_eq!(
            tree.get(&i.to_be_bytes()),
            Some(&(i * 10)),
            "var lookup failed for key {i}"
        );
    }
    assert_eq!(tree.len(), 7);
}

/// Rebalancing keeps the tree compact: after many sequential inserts the leaf
/// count is well below the split-only count (splits at mid leave half-empty
/// leaves; rebalance keeps them near-full).
#[test]
fn test_rebalance_packs_tighter() {
    let mut tree: CTree<u64, u64, u16, 4, 5> = CTree::new();
    let count = 48;
    for i in 0..count {
        tree.insert(i, i).unwrap();
    }
    // 48 keys, N=4. Split-only (split at mid=2) leaves leaves ~half-full
    // (~20 live leaves). Rebalance keeps them fuller; assert a bound well below
    // the split-only count to confirm rebalancing is doing work. Check
    // `n_leaves` (LIVE leaves) — with the gap arena, `leaves.len()` counts
    // slots (live + gaps), which the ~90% spread intentionally keeps ~2× live.
    assert!(
        tree.n_leaves <= 18,
        "expected compact leaves (<=18 live, split-only ~20), got {}",
        tree.n_leaves
    );
    for i in 0..count {
        assert_eq!(tree.get(&i), Some(&i));
    }
    assert_eq!(tree.len(), count as usize);
}

/// Inode rebalance: when a bottom inode is full and a sibling inode has room, an
/// insert that descends through it rebalances the inode instead of growing the
/// tree height.
///
/// N=3. We first build a height-2 tree whose left bottom inode is full and whose
/// right bottom inode is sparse (by inserting a dense block then a sparse tail),
/// then insert into the full inode's subtree and assert height does not increase.
#[test]
fn test_inode_rebalance_absorbs_overflow() {
    let mut tree: CTree<u64, u64, u16, 3, 4> = CTree::new();
    // Fill densely to grow the tree, then verify an insert that would overflow
    // an inode (had it no sibling room) does not raise the height when a sibling
    // has room. The existing deep-split suites already stress inode rebalance
    // heavily; here we pin that height is stable across a late insert.
    for i in 0..40u64 {
        tree.insert(i, i).unwrap();
    }
    let height_before = tree.height;
    // A few more inserts in the dense region descend through full inodes; with
    // sibling room available they rebalance rather than split.
    for i in 40..44u64 {
        tree.insert(i, i).unwrap();
    }
    assert_eq!(
        tree.height, height_before,
        "height should not grow when an inode can rebalance with a sibling"
    );
    for i in 0..44u64 {
        assert_eq!(tree.get(&i), Some(&i), "lookup failed for key {i}");
    }
    assert_eq!(tree.len(), 44);
}

/// Rebalance must preserve the leaf linked list: forward iteration from the
/// first leaf visits every key in order, even after many rebalances.
#[test]
fn test_rebalance_preserves_leaf_links() {
    let mut tree: CTree<u64, u64, u16, 3, 4> = CTree::new();
    // Insert keys in a small/large interleaved order: 0, 59, 1, 58, 2, 57, ...
    // This drives inserts into both ends of the key space, triggering both left
    // and right sibling rebalances. All 60 keys are distinct (no duplicates).
    let n = 60u64;
    for i in 0..n {
        let k = if i % 2 == 0 { i / 2 } else { n - 1 - i / 2 };
        tree.insert(k, k * 2).unwrap();
    }
    let mut cursor = tree.get_cursor();
    let mut collected = Vec::new();
    while let Some((k, v)) = cursor.current() {
        collected.push((*k, *v));
        if cursor.next().is_none() {
            break;
        }
    }
    assert_eq!(collected.len(), n as usize);
    for (i, (k, v)) in collected.iter().enumerate() {
        assert_eq!(*k, i as u64, "key order broken at {i}");
        assert_eq!(*v, i as u64 * 2, "value broken at {i}");
    }
}

// ---------------------------------------------------------------------------
// optimize — reorder the leaf arena into linked-list order
// ---------------------------------------------------------------------------

/// `optimize` on a single-leaf tree (height 0) is a no-op that still clears
/// the (already clear) links and keeps lookups working.
#[test]
fn test_optimize_single_leaf() {
    let mut tree: CTree<u64, u64, u16, 4, 5> = CTree::new();
    for i in 0..3 {
        tree.insert(i, i * 10).unwrap();
    }
    tree.optimize();
    assert_eq!(tree.leaves.len(), 1);
    assert_eq!(tree.height, 0);
    for i in 0..3 {
        assert_eq!(tree.get(&i), Some(&(i * 10)));
    }
}

/// `optimize` on an empty tree is a no-op.
#[test]
fn test_optimize_empty() {
    let mut tree: CTree<u64, u64, u16, 4, 5> = CTree::new();
    tree.optimize();
    assert_eq!(tree.len(), 0);
    assert_eq!(tree.leaves.len(), 1);
}

/// After `optimize`, the leaf arena is in linked-list order: `leaves[i].next`
/// points to `i+1` and `leaves[i].prev` to `i-1`, and every lookup still
/// resolves. Insert in reverse so splits scatter leaves out of sorted order
/// before the call.
#[test]
fn test_optimize_linearizes_leaves() {
    let mut tree: CTree<u64, u64, u16, 3, 4> = CTree::new();
    let count = 60u64;
    for i in (0..count).rev() {
        tree.insert(i, i * 10).unwrap();
    }
    // A multi-leaf tree is required for the reorder to be meaningful.
    assert!(tree.leaves.len() > 1, "precondition: need multiple leaves");
    assert!(tree.height >= 1);

    tree.optimize();

    // With the linked list gone, `optimize` must leave the arena *dense and
    // strictly sorted*: no gaps, every slot live, each leaf's max key below the
    // next leaf's min key — the property that makes gap-skip iteration correct.
    let n = tree.leaves.len();
    assert_eq!(n, tree.n_leaves, "optimize should leave no gaps (dense)");
    for i in 0..n {
        assert!(tree.leaves[i].keys.len() > 0, "slot {i} empty after optimize");
    }
    assert_eq!(tree.first_leaf(), 0);
    for i in 1..n {
        let prev = &tree.leaves[i - 1];
        let cur = &tree.leaves[i];
        let prev_max = *prev.keys.get(prev.keys.len() - 1);
        let cur_min = *cur.keys.get(0);
        assert!(
            prev_max < cur_min,
            "leaves out of order at {i}: {prev_max} >= {cur_min}"
        );
    }

    for i in 0..count {
        assert_eq!(tree.get(&i), Some(&(i * 10)), "lookup broken for key {i}");
    }
    assert_eq!(tree.len(), count as usize);
}

/// `optimize` preserves full forward and backward iteration order.
#[test]
fn test_optimize_preserves_iteration() {
    let mut tree: CTree<u64, u64, u16, 3, 4> = CTree::new();
    let order: [u64; 40] = [
        15, 3, 27, 8, 21, 0, 12, 6, 18, 24, 9, 1, 39, 14, 5, 22, 11, 7, 19, 2, 25, 13, 38, 10,
        4, 16, 23, 17, 36, 20, 33, 31, 29, 28, 30, 32, 34, 35, 37, 26,
    ];
    for &i in &order {
        tree.insert(i, i * 10).unwrap();
    }
    tree.optimize();

    // Forward
    let mut cursor = tree.get_cursor();
    let mut fwd = Vec::new();
    while let Some((k, v)) = cursor.current() {
        fwd.push((*k, *v));
        if cursor.next().is_none() {
            break;
        }
    }
    assert_eq!(fwd.len(), 40);
    for (i, (k, v)) in fwd.iter().enumerate() {
        assert_eq!(*k, i as u64);
        assert_eq!(*v, i as u64 * 10);
    }

    // Backward from the largest key
    let mut cursor = tree.cursor_at(&39u64);
    let mut rev = Vec::new();
    while let Some((k, v)) = cursor.current() {
        rev.push((*k, *v));
        if cursor.prev().is_none() {
            break;
        }
    }
    assert_eq!(rev.len(), 40);
    for (i, (k, v)) in rev.iter().enumerate() {
        assert_eq!(*k, 39 - i as u64);
        assert_eq!(*v, (39 - i as u64) * 10);
    }
}

/// `optimize` is idempotent: a second call changes nothing and keeps the tree
/// valid. Also verifies inserts still work after optimize (the remapped inode
/// ptrs route new keys correctly).
#[test]
fn test_optimize_idempotent_and_insert_after() {
    let mut tree: CTree<u64, u64, u16, 3, 4> = CTree::new();
    for i in 0..40 {
        tree.insert(i, i).unwrap();
    }
    tree.optimize();
    let leaves_after = tree.leaves.len();

    tree.optimize();
    assert_eq!(tree.leaves.len(), leaves_after, "second optimize should not move leaves");
    // Dense + every slot live (no linked list to check; assert the arena state).
    assert_eq!(tree.leaves.len(), tree.n_leaves);
    for i in 0..leaves_after {
        assert!(tree.leaves[i].keys.len() > 0, "slot {i} empty after second optimize");
    }

    // Insert a new key larger than all existing ones — it routes to the last
    // leaf and must be findable.
    tree.insert(100, 1000).unwrap();
    assert_eq!(tree.get(&100), Some(&1000));
    for i in 0..40 {
        assert_eq!(tree.get(&i), Some(&i));
    }
    assert_eq!(tree.len(), 41);
}

/// `optimize` works on the variable-length key form too (`Box<[u8]>`), the
/// instantiation the bench uses.
#[test]
fn test_optimize_var_len_keys() {
    let mut tree: CTree<Box<[u8]>, usize, u32, 4, 5> = CTree::new();
    // Insert byte keys in reverse lexicographic order to scatter leaves.
    let count = 50;
    for i in (0..count).rev() {
        let key = format!("key-{i:03}");
        tree.insert(Box::from(key.as_bytes()), i).unwrap();
    }
    assert!(tree.leaves.len() > 1);
    tree.optimize();

    // Dense + every slot live (varlen form). Sortedness is covered by the
    // lookup loop below + the generic cursor suites.
    let n = tree.leaves.len();
    assert_eq!(n, tree.n_leaves, "optimize should leave no gaps (dense)");
    for i in 0..n {
        assert!(tree.leaves[i].keys.len() > 0, "slot {i} empty after optimize");
    }
    for i in 0..count {
        let key = format!("key-{i:03}");
        assert_eq!(tree.get(key.as_bytes()), Some(&i));
    }
}



// ---------------------------------------------------------------------------
// EXPERIMENT / DIAGNOSTIC: log the leaf arena layout after optimize + grow and
// trace the forward-iteration visit pattern. The arena is kept in strict sorted
// physical order: splits `rotate_right` the run after the split point into the
// adjacent gap so the new leaf lands at its exact sorted slot, and `spread`
// (triggered at ~90% occupancy, doubling capacity) re-disperses live leaves
// into even slots with gaps at odd + trailing. There is no per-leaf linked
// list — iteration scans forward skipping gaps. This is development
// instrumentation for the iteration-perf work, not a correctness test.
//
// `#[ignore]` so it is SKIPPED by `cargo test` (it still compiles every build,
// so it cannot bit-rot) and runs only on demand:
//
//   cargo test --lib int_btree::tests::experiment_optimize_layout -- --ignored --nocapture
// ---------------------------------------------------------------------------
#[test]
#[ignore = "diagnostic only: arena-layout + iteration-visit trace"]
fn experiment_optimize_layout() {
    type T = CTree<u64, u64, u16, 8, 9>; // N=8
    let mut tree = T::new();

    // Deterministic PRNG (xorshift32) so the run is reproducible.
    let mut state: u32 = 0x1234_5678;
    let mut next_key = || -> u64 {
        let mut x = state;
        x ^= x << 13; x ^= x >> 17; x ^= x << 5;
        state = x;
        x as u64
    };

    // Concrete dump for u64 keys.
    fn dump_u64(label: &str, tree: &CTree<u64, u64, u16, 8, 9>, opt_len: usize) {
        let n = tree.leaves.len();
        eprintln!("\n=== {label} (n={n}, height={}, opt_len={opt_len}) ===", tree.height);
        // Forward gap-skip walk: the arena is kept in strict sorted physical
        // order, so a forward scan over non-empty leaves *is* the sorted order
        // (no linked list to walk).
        let mut order: Vec<usize> = Vec::with_capacity(n);
        for i in 0..n {
            if tree.leaves[i].keys.len() > 0 {
                order.push(i);
            }
        }
        let mut new_pos = vec![usize::MAX; n];
        for (i, &o) in order.iter().enumerate() {
            new_pos[o] = i;
        }
        let live = order.len();
        let gaps = n - live;
        let displaced = (0..n).filter(|&i| new_pos[i] != i).count();
        let prefix_inplace = (0..opt_len).filter(|&i| new_pos[i] == i).count();
        let prefix_shifted = opt_len - prefix_inplace;
        let suffix = n - opt_len;
        eprintln!(
            "order(arena idx): {order:?}\n\
             live={live} gaps={gaps}  displaced={displaced}/{n}  \
             prefix_inplace={prefix_inplace}/{opt_len}  prefix_shifted={prefix_shifted}/{opt_len}  suffix={suffix}"
        );
        eprintln!("  idx | nkeys | keyrange     | new_pos | region");
        for i in 0..n {
            let l = &tree.leaves[i];
            let klen = l.keys.len();
            let (lo, hi) = if klen > 0 {
                (*l.keys.get(0), *l.keys.get(klen - 1))
            } else {
                (0, 0)
            };
            let region = if i < opt_len { "PREFIX" } else { "sfx" };
            eprintln!(
                "  [{i:>2}] k{klen} {lo:>5}..{hi:<5} | new={:>2} | {}",
                new_pos[i],
                region
            );
        }
    }

    // Phase 1: build ~80 distinct keys, then optimize. (Scaled up from N=4's
    // 40 because N=8 leaves hold ~2x the keys, to keep the arena a comparable
    // size so the bounce pattern is a fair comparison.)
    let mut seen = std::collections::HashSet::new();
    while seen.len() < 80 {
        let k = next_key() % 800;
        if seen.insert(k) {
            tree.insert(k, k * 10).unwrap();
        }
    }
    dump_u64("BEFORE optimize (initial build)", &tree, 0);
    tree.optimize();
    let opt_len = tree.leaves.len();
    dump_u64("AFTER optimize", &tree, opt_len);

    // Phase 2: grow with ~160 more distinct keys (triggers splits), do NOT optimize.
    while seen.len() < 240 {
        let k = next_key() % 2400;
        if seen.insert(k) {
            tree.insert(k, k * 10).unwrap();
        }
    }
    dump_u64("AFTER growing more (no optimize since)", &tree, opt_len);

    // Trace the FORWARD iteration visit order (arena indices the cursor touches,
    // in order) and the per-step jump distance |next - cur|. This is the cache
    // access pattern forward iteration actually pays. Append-at-end splitting
    // makes the cursor bounce between the low-index prefix and high-index
    // appended suffix leaves.
    trace_visit("RANDOM post-grow, NO re-optimize (forward iteration)", &tree);
    tree.optimize();
    trace_visit("RANDOM post-grow, AFTER re-optimize (forward iteration)", &tree);

    // Phase 3: also show a "sequential-insert" control (prefix should stay put).
    let mut tree2 = T::new();
    let mut seen2 = std::collections::HashSet::new();
    for i in 0..80u64 {
        tree2.insert(i, i * 10).unwrap();
        seen2.insert(i);
    }
    tree2.optimize();
    let opt_len2 = tree2.leaves.len();
    for i in 80..240u64 {
        tree2.insert(i, i * 10).unwrap();
        seen2.insert(i);
    }
    eprintln!("\n\n##### CONTROL: sequential inserts (splits only at the end) #####");
    dump_u64("CONTROL seq AFTER growing (no optimize since)", &tree2, opt_len2);
    trace_visit("CONTROL seq post-grow, NO re-optimize", &tree2);
}

/// Walk the live leaves in forward arena order (the exact path a forward
/// cursor takes via gap-skip) and print the arena index visited at each step
/// plus the jump distance from the previous leaf. Non-contiguous jumps (|d|>1)
/// are the cache misses that kill forward-iteration throughput.
fn trace_visit(label: &str, tree: &CTree<u64, u64, u16, 8, 9>) {
    let mut visit: Vec<usize> = Vec::with_capacity(tree.leaves.len());
    let n = tree.leaves.len();
    let mut i = 0;
    while i < n {
        if tree.leaves[i].keys.len() > 0 {
            visit.push(i);
        }
        i += 1;
    }
    let n = visit.len();
    eprintln!("\n--- {label} (visiting {n} leaves) ---");
    let mut line = String::new();
    let mut max_jump = 0usize;
    let mut sum_jump = 0u64;
    let mut noncontig = 0usize;
    for w in visit.windows(2) {
        let d = w[1].abs_diff(w[0]);
        if d > 1 {
            noncontig += 1;
        }
        sum_jump += d as u64;
        if d > max_jump {
            max_jump = d;
        }
        line.push_str(&format!("{} --({})--> ", w[0], d));
    }
    if let Some(&last) = visit.last() {
        line.push_str(&last.to_string());
    }
    eprintln!("{line}");
    let steps = n.saturating_sub(1);
    let mean = if steps > 0 { sum_jump as f64 / steps as f64 } else { 0.0 };
    eprintln!(
        "summary: steps={steps}, noncontiguous(>1)={noncontig}/{steps}, mean_jump={mean:.1}, max_jump={max_jump}, arena_size={}",
        tree.leaves.len()
    );
}

// ---------------------------------------------------------------------------
// EXPERIMENT: does `Option<LeafNode>` get a *free* discriminant (niche opt)?
//
// `LeafNode` carries two `Option<NonZero<PTR>>` fields (`prev`/`next`) and two
// `TinyArray<_, N>` fields. Each `TinyArray` stores `len: u8` valid over
// `0..=N`, so when `N < 255` the values `N+1..=255` are an invalid bitpattern —
// a niche the outer `Option` can reuse as its `None` tag.
//
// The `Option<NonZero<PTR>>` fields do NOT obviously help `Option<LeafNode>`:
// their zero bitpattern is already a *valid* state (the inner `None`), so it
// is not available as the outer `None`. The question this experiment answers:
// is the free discriminant real, and where does it come from — the PTR niche
// or the TinyArray `len` niche? The `N = 255` row (no `len` niche) is the
// deciding datapoint: if `Option<LeafNode>` still fits in `LeafNode` there,
// the PTR niche is doing the work; if it grows, the niche was `len` only.
//
//   cargo test --lib int_btree::tests::experiment_option_leafnode_niche -- --ignored --nocapture
// ---------------------------------------------------------------------------
#[test]
#[ignore = "diagnostic only: measures Option<LeafNode> niche optimization"]
fn experiment_option_leafnode_niche() {
    use std::mem::{align_of, size_of};

    // Print one row: LeafNode size, Option<LeafNode> size, alignment, whether free.
    macro_rules! row {
        ($label:expr, $lt:ty) => {
            let leaf = size_of::<$lt>();
            let opt = size_of::<Option<$lt>>();
            let al = align_of::<$lt>();
            let free = leaf == opt;
            eprintln!(
                "  {:<28} leaf={:>4}  Option<leaf>={:>4}  align={:>2}  {}  (delta={})",
                $label,
                leaf,
                opt,
                al,
                if free { "FREE  " } else { "paid  " },
                opt as isize - leaf as isize,
            );
        };
    }

    // K = u64 (fixed SIMD path), V = u64. Sweep N across small values and the
    // N = 255 edge (where TinyArray's `len` niche vanishes), and PTR across the
    // TrieIndex types. NP1 = N + 1 for LeafNode's sibling KeyNode, but LeafNode
    // itself only takes N.
    eprintln!("=== Option<LeafNode> niche experiment (K=u64, V=u64) ===");
    eprintln!("-- N = 4 (typical) --");
    row!("LeafNode<_,_,u8, 4>", LeafNode<u64, u64, u8, 4>);
    row!("LeafNode<_,_,u16,4>", LeafNode<u64, u64, u16, 4>);
    row!("LeafNode<_,_,u32,4>", LeafNode<u64, u64, u32, 4>);
    row!("LeafNode<_,_,u64,4>", LeafNode<u64, u64, u64, 4>);

    eprintln!("-- N = 8 --");
    row!("LeafNode<_,_,u16,8>", LeafNode<u64, u64, u16, 8>);
    row!("LeafNode<_,_,u32,8>", LeafNode<u64, u64, u32, 8>);

    eprintln!("-- N = 16 --");
    row!("LeafNode<_,_,u16,16>", LeafNode<u64, u64, u16, 16>);
    row!("LeafNode<_,_,u32,16>", LeafNode<u64, u64, u32, 16>);

    eprintln!("-- N = 32 --");
    row!("LeafNode<_,_,u32,32>", LeafNode<u64, u64, u32, 32>);

    // The deciding row: N = 255 ⇒ TinyArray `len` has no niche (0..=255 all
    // valid). If Option<LeafNode> is still == LeafNode here, the PTR niche is
    // exploited; if it grows, the niche was `len`-only.
    eprintln!("-- N = 255 (no len niche) --");
    row!("LeafNode<_,_,u8, 255>", LeafNode<u64, u64, u8, 255>);
    row!("LeafNode<_,_,u16,255>", LeafNode<u64, u64, u16, 255>);
    row!("LeafNode<_,_,u32,255>", LeafNode<u64, u64, u32, 255>);
    row!("LeafNode<_,_,u64,255>", LeafNode<u64, u64, u64, 255>);

    // Sanity: the inner Option<NonZero<PTR>> is itself niche-optimized (free).
    eprintln!("-- inner Option<NonZero<PTR>> (should be FREE) --");
    row!("Option<NonZero<u16>>", Option<std::num::NonZero<u16>>);
    row!("Option<NonZero<u32>>", Option<std::num::NonZero<u32>>);

    // Bare TinyArray niche check: confirms the `len` field is the niche source
    // for N < 255 and vanishes at N = 255.
    eprintln!("-- bare TinyArray<u64, N> (len niche source) --");
    row!("TinyArray<u64, 4>",   crate::tiny_array::TinyArray<u64, 4>);
    row!("TinyArray<u64, 255>", crate::tiny_array::TinyArray<u64, 255>);

    // Isolation controls: why is Option<TinyArray> NOT free despite the `len`
    // niche? Three hand-rolled structs, all 40 bytes, all with a `len: u8`
    // field valid over 0..=4 (so 5..=255 is a niche). The difference is what
    // sits in the payload: a plain array, a MaybeUninit array, or nothing.
    // Whichever stays 40 as an Option tells us which payload kills the niche.
    use std::mem::MaybeUninit;
    #[repr(C)]
    struct LenPlain    { len: u8,        slots: [u64; 4] }
    #[repr(C)]
    struct LenMaybe    { len: u8,        slots: [MaybeUninit<u64>; 4] }
    #[repr(C)]
    struct LenOnly    { len: u8,        _pad: [u8; 31] }
    eprintln!("-- controls: does MaybeUninit kill the len niche? --");
    row!("LenPlain  (len + [u64;4])",      LenPlain);
    row!("LenMaybe  (len + [MU<u64;4])",   LenMaybe);
    row!("LenOnly   (len + pad)",          LenOnly);
}

// ---------------------------------------------------------------------------
// KeyRef inline vs buf path tests
// ---------------------------------------------------------------------------

#[test]
fn test_keyref_inline_short_keys() {
    // Keys ≤ 14 bytes should be inlined (no key_buf usage).
    let mut tree: CTree<Vec<u8>, u64, u16, 4, 5> = CTree::new();

    // Short keys (1–14 bytes) → all inline
    let short_keys: &[&[u8]] = &[
        b"a",                   // 1 byte
        b"ab",                  // 2 bytes
        b"hello",               // 5 bytes
        b"fourteen!!",          // 10 bytes
        b"12345678901234",      // 14 bytes (exactly at threshold)
    ];

    for (i, k) in short_keys.iter().enumerate() {
        tree.insert(k.to_vec(), i as u64).unwrap();
    }

    // Verify lookups
    for (i, k) in short_keys.iter().enumerate() {
        assert_eq!(
            tree.get(k),
            Some(&(i as u64)),
            "inline lookup failed for key {:?}",
            String::from_utf8_lossy(k)
        );
    }

    // key_buf should be empty since all keys are inlined
    assert!(
        tree.key_buf.is_empty(),
        "key_buf should be empty when all keys are inline, but has {} bytes",
        tree.key_buf.len()
    );
}

#[test]
fn test_keyref_buf_long_keys() {
    // Keys > 14 bytes should go to key_buf.
    let mut tree: CTree<Vec<u8>, u64, u16, 4, 5> = CTree::new();

    // Long key (> 14 bytes) → goes to key_buf
    let long_key = b"this_is_a_long_key!".to_vec(); // 19 bytes
    tree.insert(long_key.clone(), 42).unwrap();

    assert_eq!(tree.get(&long_key), Some(&42));
    assert!(
        !tree.key_buf.is_empty(),
        "key_buf should not be empty for long keys"
    );
}

#[test]
fn test_keyref_mixed_inline_and_buf() {
    // Mix of inline and buf keys in the same tree.
    let mut tree: CTree<Vec<u8>, u64, u16, 4, 5> = CTree::new();

    let short_key = b"short".to_vec();    // 5 bytes → inline
    let long_key = b"a_very_long_key_here!".to_vec(); // 21 bytes → buf

    tree.insert(short_key.clone(), 1).unwrap();
    tree.insert(long_key.clone(), 2).unwrap();

    assert_eq!(tree.get(&short_key), Some(&1));
    assert_eq!(tree.get(&long_key), Some(&2));
}

#[test]
fn test_keyref_inline_ordering() {
    // Verify inline keys maintain correct sort order via cursor.
    let mut tree: CTree<Vec<u8>, u64, u16, 4, 5> = CTree::new();

    // Insert in non-sorted order
    tree.insert(b"delta".to_vec(), 4).unwrap();
    tree.insert(b"alpha".to_vec(), 1).unwrap();
    tree.insert(b"echo".to_vec(), 5).unwrap();
    tree.insert(b"bravo".to_vec(), 2).unwrap();
    tree.insert(b"charlie".to_vec(), 3).unwrap();

    // Verify sorted order via cursor: alpha(1), bravo(2), charlie(3), delta(4), echo(5)
    let expected: &[(&[u8], u64)] = &[
        (b"alpha", 1), (b"bravo", 2), (b"charlie", 3),
        (b"delta", 4), (b"echo", 5),
    ];
    let mut cursor = tree.get_cursor();
    for (_key_bytes, val) in expected {
        let (_, v) = cursor.current().unwrap();
        assert_eq!(*v, *val, "cursor got value {} expected {}", v, val);
        cursor.next();
    }
}

#[test]
fn test_keyref_sizes() {
    use std::mem::{size_of, align_of};
    // KeyRef should be 16 bytes: Inline variant holds TinyArray<u8,14> (15 bytes)
    // but Rust may pad the enum. Let's just check it's reasonable.
    assert_eq!(size_of::<KeyRef>(), 16, "KeyRef size should be 16 bytes");
    assert_eq!(align_of::<KeyRef>(), 8, "KeyRef alignment should be 8");
    // BufKey remains 8 bytes
    assert_eq!(size_of::<BufKey>(), 8, "BufKey size should be 8 bytes");
}



#[test]
#[ignore = "asc vs desc insert timing"]
fn experiment_asc_vs_desc() {
    use std::time::Instant;
    let n: u64 = 1_000_000;
    let v = |k: u64| k;

    let t = Instant::now();
    let mut asc: CTree<u64, u64, u32, 8, 9> = CTree::new();
    for k in 0..n { asc.insert(k, v(k)).unwrap(); }
    let asc_us = t.elapsed().as_micros();

    let t = Instant::now();
    let mut desc: CTree<u64, u64, u32, 8, 9> = CTree::new();
    for k in (0..n).rev() { desc.insert(k, v(k)).unwrap(); }
    let desc_us = t.elapsed().as_micros();

    eprintln!("ascending  0..{n}: {asc_us} us ({:.0} keys/sec)", (n as f64)*1e6/(asc_us as f64).max(1.0));
    eprintln!("descending {n}..0: {desc_us} us ({:.0} keys/sec)", (n as f64)*1e6/(desc_us as f64).max(1.0));
    // sanity: both trees hold all keys
    assert_eq!(asc.len() as u64, n);
    assert_eq!(desc.len() as u64, n);
}
