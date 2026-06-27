use super::*;

#[test]
fn node_ref_size() {
    assert_eq!(std::mem::size_of::<NodeRef>(), 8);
}

#[test]
fn node_ref_discriminant_values() {
    // NodeRef uses #[repr(u8)] with explicit discriminants.
    // Verify discriminant values by reading byte 0 of each variant.
    fn discriminant(n: NodeRef) -> u8 {
        unsafe { *(&n as *const NodeRef as *const u8) }
    }
    assert_eq!(discriminant(NodeRef::Empty), 0);
    assert_eq!(discriminant(NodeRef::Leaf { prefix_len: 0, idx: 0 }), 1);
    assert_eq!(discriminant(NodeRef::Node2 { prefix_len: 0, idx: 0 }), 2);
    assert_eq!(discriminant(NodeRef::Node4 { prefix_len: 0, idx: 0 }), 3);
    assert_eq!(discriminant(NodeRef::Node16 { prefix_len: 0, idx: 0 }), 4);
}

#[test]
fn node_ref_constructors() {
    // Verify that convenience constructors work correctly
    let leaf = NodeRef::leaf(42, 100);
    assert_eq!(leaf, NodeRef::Leaf { prefix_len: 42, idx: 100 });

    let node2 = NodeRef::node2(7, 3);
    assert_eq!(node2, NodeRef::Node2 { prefix_len: 7, idx: 3 });

    let node4 = NodeRef::node4(10, 5);
    assert_eq!(node4, NodeRef::Node4 { prefix_len: 10, idx: 5 });

    let node16 = NodeRef::node16(20, 8);
    assert_eq!(node16, NodeRef::Node16 { prefix_len: 20, idx: 8 });
}

#[test]
fn node_ref_accessors() {
    // prefix_len and idx accessors
    let leaf = NodeRef::leaf(42, 100);
    assert_eq!(leaf.prefix_len(), 42);
    assert_eq!(leaf.idx(), 100);
    assert!(!leaf.is_internal());

    let node2 = NodeRef::node2(7, 3);
    assert_eq!(node2.prefix_len(), 7);
    assert_eq!(node2.idx(), 3);
    assert!(node2.is_internal());
    assert_eq!(node2.width(), 2);
    assert_eq!(node2.radix_bits(), 1);

    let node4 = NodeRef::node4(10, 5);
    assert_eq!(node4.width(), 4);
    assert_eq!(node4.radix_bits(), 2);

    let node16 = NodeRef::node16(20, 8);
    assert_eq!(node16.width(), 16);
    assert_eq!(node16.radix_bits(), 4);

    // Empty accessors
    assert_eq!(NodeRef::Empty.prefix_len(), 0);
    assert_eq!(NodeRef::Empty.idx(), 0);
    assert!(!NodeRef::Empty.is_internal());
}

#[test]
fn insert_empty_and_get() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let idx = trie.insert(b"hello".to_vec(), 42).unwrap();
    assert_eq!(trie.get(b"hello\0"), Some(idx));
    assert_eq!(trie.get_value(b"hello\0"), Some(&42));
    assert_eq!(trie.get(b"world\0"), None);
}

#[test]
fn insert_two_keys_split() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc\0"), Some(i1));
    assert_eq!(trie.get(b"abd\0"), Some(i2));
    assert_eq!(trie.get(b"abe\0"), None);
    assert_eq!(trie.len(), 2);
}

#[test]
fn insert_duplicate_returns_error() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.insert(b"hello".to_vec(), 1).unwrap();
    let result = trie.insert(b"hello".to_vec(), 2);
    assert_eq!(result, Err(()));
    assert_eq!(trie.len(), 1);
}

#[test]
fn insert_rejects_null_byte() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let result = trie.insert(b"hel\0lo".to_vec(), 1);
    assert_eq!(result, Err(()));
}

#[test]
fn insert_prefix_key() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abcd".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc\0"), Some(i1));
    assert_eq!(trie.get(b"abcd\0"), Some(i2));
}

#[test]
fn insert_reverse_prefix_key() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let i1 = trie.insert(b"abcd".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abc".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abcd\0"), Some(i1));
    assert_eq!(trie.get(b"abc\0"), Some(i2));
}

#[test]
fn insert_no_common_prefix() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"xyz".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc\0"), Some(i1));
    assert_eq!(trie.get(b"xyz\0"), Some(i2));
    assert_eq!(trie.get(b"ab\0"), None);
}

#[test]
fn insert_three_keys() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let i1 = trie.insert(b"abc".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"abd".to_vec(), 2).unwrap();
    let i3 = trie.insert(b"abe".to_vec(), 3).unwrap();
    assert_eq!(trie.get(b"abc\0"), Some(i1));
    assert_eq!(trie.get(b"abd\0"), Some(i2));
    assert_eq!(trie.get(b"abe\0"), Some(i3));
}

#[test]
fn insert_many_keys() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    for i in 0..100 {
        let key = format!("key_{:03}\0", i);
        let result = trie.get(key.as_bytes());
        assert!(result.is_some(), "get({:?}) returned None for i={}", key, i);
    }
    assert_eq!(trie.len(), 100);
}

#[test]
fn len_and_is_empty() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    assert!(trie.is_empty());
    assert_eq!(trie.len(), 0);
    trie.insert(b"hello".to_vec(), 1).unwrap();
    assert!(!trie.is_empty());
    assert_eq!(trie.len(), 1);
    trie.insert(b"world".to_vec(), 2).unwrap();
    assert_eq!(trie.len(), 2);
}

#[test]
fn insert_single_char_keys() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let mut indices = Vec::new();
    for c in b'a'..=b'f' {
        let idx = trie.insert(vec![c], c as i32).unwrap();
        indices.push(idx);
    }
    for (i, c) in (b'a'..=b'f').enumerate() {
        let key = vec![c, 0];
        assert_eq!(trie.get(&key), Some(indices[i]));
    }
}

#[test]
fn insert_deeply_nested() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let mut key = Vec::new();
    for i in 0..100 {
        key.push(b'a');
        let idx = trie.insert(key.clone(), i).unwrap();
        let mut nt_key = key.clone();
        nt_key.push(0);
        assert_eq!(trie.get(&nt_key), Some(idx));
    }
}

#[test]
fn into_keys_values_roundtrip() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"def".to_vec(), 2).unwrap();
    assert_eq!(trie.get(b"abc\0"), Some(0));
    assert_eq!(trie.get(b"def\0"), Some(1));
    assert_eq!(trie.len(), 2);
}

#[test]
fn arena_and_ref_keys_populated() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    // After inserting two keys, arena slots may be 2 (Node2) or 4 (Node4
    // if graduation occurred), depending on whether the Node2 was graduated.
    // Either way, ref_keys should stay in sync with arena capacity.
    assert!(trie.arena.len() >= 2);
    assert_eq!(trie.ref_keys.len(), trie.arena.capacity());
}

#[test]
fn insert_stress() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let n: usize = 500;
    for i in 0..n {
        let key = format!("key_{:05}", i);
        let result = trie.insert(key.into_bytes(), i as i32);
        assert!(result.is_ok(), "insert failed at i={}", i);
    }
    assert_eq!(trie.len(), n);
    for i in 0..n {
        let key = format!("key_{:05}\0", i);
        let result = trie.get(key.as_bytes());
        assert!(result.is_some(), "get failed at i={}", i);
        assert_eq!(result.unwrap(), i);
    }
    // Non-existent keys
    assert_eq!(trie.get(b"key_99999\0"), None);
    assert_eq!(trie.get(b"aaa\0"), None);
    assert_eq!(trie.get(b"zzz\0"), None);
}

#[test]
fn insert_reverse_order() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    // Insert keys in reverse order to test node splits at different positions
    for i in (0..20).rev() {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    for i in 0..20 {
        let key = format!("key_{:03}\0", i);
        assert!(trie.get(key.as_bytes()).is_some());
    }
}

#[test]
fn insert_same_first_byte() {
    // Keys that all start with the same byte — tests deeper trie levels
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let keys: Vec<Vec<u8>> = vec![
        b"a".to_vec(),
        b"ab".to_vec(),
        b"abc".to_vec(),
        b"abd".to_vec(),
        b"abde".to_vec(),
        b"abdef".to_vec(),
    ];
    for (i, key) in keys.iter().enumerate() {
        trie.insert(key.clone(), i as i32).unwrap();
    }
    for (i, key) in keys.iter().enumerate() {
        let mut nt_key = key.clone();
        nt_key.push(0);
        assert_eq!(trie.get(&nt_key), Some(i));
    }
}

#[test]
fn get_value_found_and_missing() {
    let mut trie: PolyTrie<String> = PolyTrie::new();
    trie.insert(b"hello".to_vec(), "world".to_string()).unwrap();
    assert_eq!(trie.get_value(b"hello\0"), Some(&"world".to_string()));
    assert_eq!(trie.get_value(b"world\0"), None);
}

#[test]
fn stress_large_keys() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    // Keys of varying lengths
    let keys = vec![
        vec![0x01],
        vec![0x01, 0x02],
        vec![0x01, 0x02, 0x03],
        vec![0x01, 0x02, 0x03, 0x04],
        vec![0x01, 0x02, 0x03, 0x04, 0x05],
        vec![0xFF],
        vec![0xFF, 0xFE],
        vec![0xFF, 0xFE, 0xFD],
    ];
    for (i, key) in keys.iter().enumerate() {
        trie.insert(key.clone(), i as i32).unwrap();
    }
    for (i, key) in keys.iter().enumerate() {
        let mut nt_key = key.clone();
        nt_key.push(0);
        assert_eq!(trie.get(&nt_key), Some(i));
    }
}

#[test]
fn structure_report_empty() {
    let trie: PolyTrie<i32> = PolyTrie::new();
    let report = trie.structure_report();
    assert_eq!(report.total_keys, 0);
    assert_eq!(report.leaves, 0);
    assert_eq!(report.total_internal, 0);
    assert_eq!(report.depth, 0);
    assert_eq!(report.empty_slots, 0);
    assert_eq!(report.node2, 0);
    assert_eq!(report.node4, 0);
    assert_eq!(report.node16, 0);
}

#[test]
fn structure_report_single_key() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.insert(b"hello".to_vec(), 1).unwrap();
    let report = trie.structure_report();
    assert_eq!(report.total_keys, 1);
    assert_eq!(report.leaves, 1);
    assert_eq!(report.total_internal, 0);
    assert_eq!(report.depth, 1); // root is a leaf
    assert_eq!(report.empty_slots, 0);
}

#[test]
fn structure_report_two_keys() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    let report = trie.structure_report();
    assert_eq!(report.total_keys, 2);
    assert_eq!(report.leaves, 2);
    // Two keys that differ in one bit may trigger graduation
    // from Node2 to Node4, so we check for at least 1 internal node
    assert!(report.total_internal >= 1);
    assert!(report.depth >= 2);
}

#[test]
fn graduation_two_leaves() {
    // Insert two keys that fill a Node2 with both leaves.
    // "a" and "b" diverge at bit 6, so Node2@6 with two leaves.
    // Both leaves are always placeable, so graduation to Node4@6 should happen.
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let i1 = trie.insert(b"a".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"b".to_vec(), 2).unwrap();
    // After graduation, should still be able to look up both keys
    assert_eq!(trie.get(b"a\0"), Some(i1));
    assert_eq!(trie.get(b"b\0"), Some(i2));
    let report = trie.structure_report();
    // Should have graduated from Node2 to Node4
    assert!(report.node4 >= 1, "expected at least 1 Node4, got {}", report.node4);
}

#[test]
fn graduation_three_keys() {
    // Insert "a", "b", "c". After "a" and "b", graduation creates Node4.
    // Then "c" should still work.
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let i1 = trie.insert(b"a".to_vec(), 1).unwrap();
    let i2 = trie.insert(b"b".to_vec(), 2).unwrap();
    let i3 = trie.insert(b"c".to_vec(), 3).unwrap();
    assert_eq!(trie.get(b"a\0"), Some(i1));
    assert_eq!(trie.get(b"b\0"), Some(i2));
    assert_eq!(trie.get(b"c\0"), Some(i3));
}

#[test]
fn graduation_debug_key_prefix() {
    // Debug: insert "key_000" through "key_009" and verify lookups
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for i in 0..10 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    #[cfg(debug_assertions)]
    trie.dump();
    for i in 0..10 {
        let key = format!("key_{:03}\0", i);
        let result = trie.get(key.as_bytes());
        if result != Some(i) {
            eprintln!("FAIL: get({:?}) = {:?}, expected Some({})", key, result, i);
        }
    }
    for i in 0..10 {
        let key = format!("key_{:03}\0", i);
        let result = trie.get(key.as_bytes());
        assert_eq!(result, Some(i), "get({:?}) failed for i={}", key, i);
    }
}

#[test]
fn structure_report_many_keys() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    let report = trie.structure_report();
    assert_eq!(report.total_keys, 100);
    assert_eq!(report.leaves, 100);
    assert!(report.total_internal > 0);
    assert!(report.depth > 1);
    // Invariants
    assert_eq!(
        report.total_internal,
        report.node2 + report.node4 + report.node16
    );
}

#[test]
fn structure_report_display() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    let report = trie.structure_report();
    let s = format!("{report}");
    assert!(s.contains("Keys:"));
    assert!(s.contains("Node2:"));
    assert!(s.contains("Depth:"));
}

#[test]
fn aligned_graduation_creates_node4() {
    // Keys that diverge at bit 6 (even position) should allow Node2→Node4
    // graduation when both slots fill. "a" and "b" differ at bit 6 (0x61 vs 0x62).
    // In MSB ordering: bit 6 is even, so graduation is allowed.
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.insert(b"a".to_vec(), 1).unwrap();
    trie.insert(b"b".to_vec(), 2).unwrap();
    let report = trie.structure_report();
    assert!(report.node4 >= 1, "expected Node4 from aligned graduation, got node4={}", report.node4);
    assert_eq!(trie.get(b"a\0"), Some(0));
    assert_eq!(trie.get(b"b\0"), Some(1));
}

#[test]
fn aligned_graduation_stress_1000() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for i in 0..1000u32 {
        let key = format!("key_{:05}", i);
        let result = trie.insert(key.into_bytes(), i as i32);
        assert!(result.is_ok(), "insert failed at i={}", i);
    }
    let report = trie.structure_report();
    println!("Stress: keys={}, node2={}, node4={}, node16={}",
        report.total_keys, report.node2, report.node4, report.node16);
    // All lookups must succeed
    for i in 0..1000u32 {
        let key = format!("key_{:05}\0", i);
        let result = trie.get(key.as_bytes());
        assert_eq!(result, Some(i as usize), "lookup failed at i={}", i);
    }
}

#[test]
fn aligned_graduation_byte_boundary_keys() {
    // Keys that diverge at byte boundaries (bit positions 0, 8, 16, etc.)
    // are always aligned for all radix widths. Skip 0x00 (null byte rejected).
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for b in 1u8..=255 {
        trie.insert(vec![b], b as i32).unwrap();
    }
    assert_eq!(trie.len(), 255);
    let report = trie.structure_report();
    println!("Byte keys: node2={}, node4={}, node16={}",
        report.node2, report.node4, report.node16);
    // Verify all lookups
    for b in 1u8..=255 {
        let key = vec![b, 0];
        assert_eq!(trie.get(&key), Some(b as usize - 1), "lookup failed for byte {}", b);
    }
}

// -----------------------------------------------------------------------
// Iterator tests
// -----------------------------------------------------------------------

#[test]
fn iter_empty() {
    let trie: PolyTrie<i32> = PolyTrie::new();
    let mut it = trie.iter();
    assert!(it.next().is_none());
    assert!(it.prev().is_none());
    assert!(it.current().is_none());
}

#[test]
fn iter_single_key() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.insert(b"hello".to_vec(), 42).unwrap();
    let mut it = trie.iter();
    assert!(it.current().is_none()); // before first
    let (k, v) = it.next().unwrap();
    assert_eq!(k, b"hello");
    assert_eq!(*v, 42);
    assert!(it.next().is_none()); // exhausted
}

#[test]
fn iter_forward() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let keys = vec![b"abc".to_vec(), b"abd".to_vec(), b"abz".to_vec()];
    for (i, key) in keys.iter().enumerate() {
        trie.insert(key.clone(), i as i32).unwrap();
    }
    let mut it = trie.iter();
    assert_eq!(it.next().unwrap().0, b"abc");
    assert_eq!(it.next().unwrap().0, b"abd");
    assert_eq!(it.next().unwrap().0, b"abz");
    assert!(it.next().is_none());
}

#[test]
fn iter_backward() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let keys = vec![b"abc".to_vec(), b"abd".to_vec(), b"abz".to_vec()];
    for (i, key) in keys.iter().enumerate() {
        trie.insert(key.clone(), i as i32).unwrap();
    }
    let mut it = trie.iter_last();
    // current() should give the last key
    assert_eq!(it.current().unwrap().0, b"abz");
    assert_eq!(it.prev().unwrap().0, b"abd");
    assert_eq!(it.prev().unwrap().0, b"abc");
    assert!(it.prev().is_none());
}

#[test]
fn iter_backward_full() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let keys = vec![b"abc".to_vec(), b"abd".to_vec(), b"abz".to_vec()];
    for (i, key) in keys.iter().enumerate() {
        trie.insert(key.clone(), i as i32).unwrap();
    }
    // Start at last, walk backward without calling current()
    let mut it = trie.iter_last();
    let mut collected: Vec<Vec<u8>> = Vec::new();
    // First prev() gives second-to-last key
    while let Some((k, _)) = it.prev() {
        collected.push(k.to_vec());
    }
    // We should get 2 keys (abd, abc) since current() wasn't called first
    // Wait - iter_last positions at the LAST key, so prev() starts from 2nd-to-last
    // If we want ALL keys, we need current() first
    assert_eq!(collected.len(), 2); // abd, abc (without calling current() first)
    assert_eq!(collected[0], b"abd");
    assert_eq!(collected[1], b"abc");
}

#[test]
fn iter_forward_backward_interleaved() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for c in b'a'..=b'f' {
        trie.insert(vec![c], c as i32).unwrap();
    }
    // Test forward iteration
    let mut it = trie.iter();
    assert_eq!(it.next().unwrap().0, b"a");
    assert_eq!(it.next().unwrap().0, b"b");
    assert_eq!(it.next().unwrap().0, b"c");
    // Now backward
    assert_eq!(it.prev().unwrap().0, b"b");
    assert_eq!(it.prev().unwrap().0, b"a");
    // Can't go further back
    assert!(it.prev().is_none());
}

#[test]
fn iter_seek_exact() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abz".to_vec(), 3).unwrap();
    let mut it = trie.iter();
    let (k, v) = it.seek(b"abd\0").unwrap();
    assert_eq!(k, b"abd");
    assert_eq!(*v, 2);
}

#[test]
fn iter_seek_between() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();
    trie.insert(b"abz".to_vec(), 3).unwrap();
    let mut it = trie.iter();
    // "abe" is between "abd" and "abz"
    let (k, _) = it.seek(b"abe\0").unwrap();
    assert_eq!(k, b"abz");
}

#[test]
fn iter_seek_prefix_key() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abcd".to_vec(), 2).unwrap();
    // Seek to "abc" should find "abc" exactly
    let mut it = trie.iter();
    let (k, v) = it.seek(b"abc\0").unwrap();
    assert_eq!(k, b"abc");
    assert_eq!(*v, 1);
}

#[test]
fn iter_seek_past_end() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.insert(b"abc".to_vec(), 1).unwrap();
    trie.insert(b"abd".to_vec(), 2).unwrap();

    // Forward iteration should return both keys
    let mut it = trie.iter();
    assert_eq!(it.next().unwrap().0, b"abc");
    assert_eq!(it.next().unwrap().0, b"abd");
    assert!(it.next().is_none());

    // Seek past all keys should return None
    let mut it = trie.iter();
    assert!(it.seek(b"zzz\0").is_none());

    // Seek between existing keys should find the next one
    let mut it = trie.iter();
    let (k, _) = it.seek(b"abcd\0").unwrap();
    assert_eq!(k, b"abd");

    // Seek to exact key should find it
    let mut it = trie.iter();
    let (k, _) = it.seek(b"abc\0").unwrap();
    assert_eq!(k, b"abc");
}

#[test]
fn iter_seek_before_all() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.insert(b"def".to_vec(), 1).unwrap();
    trie.insert(b"xyz".to_vec(), 2).unwrap();
    let mut it = trie.iter();
    let (k, _) = it.seek(b"abc\0").unwrap();
    assert_eq!(k, b"def");
}

#[test]
fn iter_stress_forward() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let n = 200;
    for i in 0..n {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    let mut it = trie.iter();
    let mut keys: Vec<Vec<u8>> = Vec::new();
    while let Some((k, _)) = it.next() {
        keys.push(k.to_vec());
    }
    assert_eq!(keys.len(), n);
    for i in 1..keys.len() {
        assert!(keys[i] > keys[i - 1], "not in sorted order at index {}", i);
    }
}

#[test]
fn iter_stress_backward() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    let n = 200;
    for i in 0..n {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    let mut it = trie.iter_last();
    let mut keys: Vec<Vec<u8>> = Vec::new();
    loop {
        match it.current() {
            Some((k, _)) => keys.push(k.to_vec()),
            None => break,
        }
        if it.prev().is_none() {
            break;
        }
    }
    assert_eq!(keys.len(), n);
    for i in 1..keys.len() {
        assert!(keys[i] < keys[i - 1], "not in reverse order at index {}", i);
    }
}

#[test]
fn iter_with_graduation() {
    // Insert enough keys to trigger graduation through Node2 → Node4 → Node16
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for i in 0..100u32 {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    let report = trie.structure_report();
    // Should have some graduated nodes
    assert!(report.node4 + report.node16 > 0, "expected some graduated nodes");

    // Verify forward iteration
    let mut it = trie.iter();
    let mut forward_keys: Vec<Vec<u8>> = Vec::new();
    while let Some((k, _)) = it.next() {
        forward_keys.push(k.to_vec());
    }
    assert_eq!(forward_keys.len(), 100);
    for i in 1..forward_keys.len() {
        assert!(forward_keys[i] > forward_keys[i - 1]);
    }

    // Verify backward iteration
    let mut it = trie.iter_last();
    let mut backward_keys: Vec<Vec<u8>> = Vec::new();
    loop {
        match it.current() {
            Some((k, _)) => backward_keys.push(k.to_vec()),
            None => break,
        }
        if it.prev().is_none() {
            break;
        }
    }
    assert_eq!(backward_keys.len(), 100);
    for i in 1..backward_keys.len() {
        assert!(backward_keys[i] < backward_keys[i - 1]);
    }
}

#[test]
fn iter_byte_boundary_keys() {
    // 255 single-byte keys (0x01..=0xFF) — creates wider node types
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for b in 1u8..=255 {
        trie.insert(vec![b], b as i32).unwrap();
    }
    // Forward iteration — collect and verify order
    let mut it = trie.iter();
    let mut keys: Vec<u8> = Vec::new();
    while let Some((k, _)) = it.next() {
        assert_eq!(k.len(), 1, "single-byte key expected, got {:?}", k);
        keys.push(k[0]);
    }
    assert_eq!(keys.len(), 255);
    for i in 1..keys.len() {
        assert!(keys[i] > keys[i - 1], "not in order: {} <= {}", keys[i], keys[i - 1]);
    }

    // Backward iteration
    let mut it = trie.iter_last();
    keys.clear();
    loop {
        match it.current() {
            Some((k, _)) => { assert_eq!(k.len(), 1); keys.push(k[0]); }
            None => break,
        }
        if it.prev().is_none() { break; }
    }
    assert_eq!(keys.len(), 255);
    for i in 1..keys.len() {
        assert!(keys[i] < keys[i - 1], "not in reverse order");
    }
}

#[test]
fn iter_seek_stress() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for i in 0..100u32 {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    // Seek to each exact key (null-terminated)
    for i in 0..100u32 {
        let key = format!("key_{:05}\0", i);
        let mut it = trie.iter();
        let (k, v) = it.seek(key.as_bytes()).unwrap();
        assert_eq!(k, &format!("key_{:05}", i).into_bytes()[..]);
        assert_eq!(*v, i as i32);
    }
    // Seek between keys
    let mut it = trie.iter();
    let (k, _) = it.seek(b"key_00050\0").unwrap();
    assert_eq!(k, b"key_00050");

    let mut it = trie.iter();
    let (k, _) = it.seek(b"key_00049\x01\0").unwrap();
    // Between key_00049 and key_00050: should land on key_00050
    assert!(k >= b"key_00050", "expected key >= key_00050, got {:?}", k);
}

// -----------------------------------------------------------------------
// Optimize tests
// -----------------------------------------------------------------------

#[test]
fn optimize_empty() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.optimize();
    assert!(trie.is_empty());
}

#[test]
fn optimize_single_key() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    trie.insert(b"hello".to_vec(), 42).unwrap();
    trie.optimize();
    assert_eq!(trie.get(b"hello\0"), Some(0));
    assert_eq!(trie.len(), 1);
}

#[test]
fn optimize_preserves_lookups() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    trie.optimize();
    for i in 0..100 {
        let key = format!("key_{:03}\0", i);
        assert_eq!(trie.get(key.as_bytes()), Some(i),
            "lookup failed after optimize for i={}", i);
    }
    assert_eq!(trie.len(), 100);
}

#[test]
fn optimize_preserves_iteration() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for i in 0..100u32 {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    trie.optimize();

    // Forward
    let mut it = trie.iter();
    let mut keys: Vec<Vec<u8>> = Vec::new();
    while let Some((k, _)) = it.next() {
        keys.push(k.to_vec());
    }
    assert_eq!(keys.len(), 100);
    for i in 1..keys.len() {
        assert!(keys[i] > keys[i - 1], "not sorted after optimize at index {}", i);
    }

    // Backward
    let mut it = trie.iter_last();
    keys.clear();
    loop {
        match it.current() {
            Some((k, _)) => keys.push(k.to_vec()),
            None => break,
        }
        if it.prev().is_none() { break; }
    }
    assert_eq!(keys.len(), 100);
    for i in 1..keys.len() {
        assert!(keys[i] < keys[i - 1], "not reverse-sorted after optimize at index {}", i);
    }
}

#[test]
fn optimize_compacts_arena() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for i in 0..50 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    let before_occupied = trie.arena.len();
    let before_capacity = trie.arena.capacity();
    trie.optimize();
    let after_occupied = trie.arena.len();
    let after_capacity = trie.arena.capacity();
    // Occupied slots should be the same (same number of live nodes)
    assert_eq!(after_occupied, before_occupied);
    // Capacity should equal occupied (no freed gaps)
    assert_eq!(after_capacity, after_occupied,
        "arena not compact after optimize: capacity={} occupied={}",
        after_capacity, after_occupied);
    // Capacity should be <= before (freed slots from graduation are reclaimed)
    assert!(after_capacity <= before_capacity);
}

#[test]
fn optimize_byte_boundary_keys() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for b in 1u8..=255 {
        trie.insert(vec![b], b as i32).unwrap();
    }
    trie.optimize();
    for b in 1u8..=255 {
        let key = vec![b, 0];
        assert_eq!(trie.get(&key), Some(b as usize - 1),
            "lookup failed after optimize for byte {}", b);
    }
}

#[test]
fn optimize_seek_preserved() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for i in 0..50u32 {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    trie.optimize();
    let mut it = trie.iter();
    let (k, v) = it.seek(b"key_00025\0").unwrap();
    assert_eq!(k, b"key_00025");
    assert_eq!(*v, 25);
}

#[test]
fn optimize_idempotent() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for i in 0..100 {
        let key = format!("key_{:03}", i);
        trie.insert(key.into_bytes(), i).unwrap();
    }
    trie.optimize();
    let cap1 = trie.arena.capacity();
    trie.optimize();
    let cap2 = trie.arena.capacity();
    assert_eq!(cap1, cap2, "second optimize changed arena size");
    for i in 0..100 {
        let key = format!("key_{:03}\0", i);
        assert_eq!(trie.get(key.as_bytes()), Some(i));
    }
}

#[test]
fn optimize_stress_1000() {
    let mut trie: PolyTrie<i32> = PolyTrie::new();
    for i in 0..1000u32 {
        let key = format!("key_{:05}", i);
        trie.insert(key.into_bytes(), i as i32).unwrap();
    }
    trie.optimize();
    for i in 0..1000u32 {
        let key = format!("key_{:05}\0", i);
        assert_eq!(trie.get(key.as_bytes()), Some(i as usize),
            "lookup failed after optimize at i={}", i);
    }
}