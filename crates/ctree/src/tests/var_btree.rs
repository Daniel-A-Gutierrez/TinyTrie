use super::*;

fn make_tree<K, V, PTR, const N: usize, const NP1: usize>() -> VarCTree<K, V, PTR, N, NP1>
where
    K: VarKey,
    PTR: TrieIndex,
    V: Sized,
    [(); N]:,
    [(); NP1]:,
{
    VarCTree::new()
}

type TestTree = VarCTree<Vec<u8>, usize, u32, 4, 5>;

#[test]
fn test_var_insert_and_get() {
    let mut tree: TestTree = TestTree::new();
    tree.insert(vec![1, 2, 3], 100).unwrap();
    tree.insert(vec![4, 5, 6], 200).unwrap();
    tree.insert(vec![7, 8, 9], 300).unwrap();

    assert_eq!(tree.len(), 3);
    assert_eq!(*tree.get(&[1, 2, 3]).unwrap(), 100);
    assert_eq!(*tree.get(&[4, 5, 6]).unwrap(), 200);
    assert_eq!(*tree.get(&[7, 8, 9]).unwrap(), 300);
    assert!(tree.get(&[2, 3, 4]).is_none());
}

#[test]
fn test_var_duplicate_insert() {
    let mut tree: TestTree = TestTree::new();
    tree.insert(vec![1, 2, 3], 100).unwrap();
    let result = tree.insert(vec![1, 2, 3], 200);
    assert!(result.is_err());
    assert_eq!(tree.len(), 1);
}

#[test]
fn test_var_long_keys() {
    let mut tree: TestTree = TestTree::new();
    let key1 = vec![0u8; 50]; // Long key (overflow)
    let key2 = vec![1u8; 50];
    let key3 = vec![2u8; 50];
    tree.insert(key1.clone(), 1).unwrap();
    tree.insert(key2.clone(), 2).unwrap();
    tree.insert(key3.clone(), 3).unwrap();

    assert_eq!(*tree.get(&key1).unwrap(), 1);
    assert_eq!(*tree.get(&key2).unwrap(), 2);
    assert_eq!(*tree.get(&key3).unwrap(), 3);
}

#[test]
fn test_var_mixed_inline_overflow() {
    let mut tree: TestTree = TestTree::new();
    // Short (inline) keys mixed with long (overflow) keys
    tree.insert(vec![1], 1).unwrap();
    tree.insert(vec![0u8; 30], 2).unwrap(); // overflow
    tree.insert(vec![2], 3).unwrap();
    tree.insert(vec![0u8; 50], 4).unwrap(); // overflow

    assert_eq!(*tree.get(&[1]).unwrap(), 1);
    assert_eq!(*tree.get(&vec![0u8; 30]).unwrap(), 2);
    assert_eq!(*tree.get(&[2]).unwrap(), 3);
    assert_eq!(*tree.get(&vec![0u8; 50]).unwrap(), 4);
}

/// Test that redistribute_leaf_right correctly moves values with keys.
/// With N=4, insert enough keys to force splits, then verify all key-value
/// pairs are intact. This specifically exercises the right-redistribution
/// path where keys and values must move together.
#[test]
fn test_var_redistribute_right_values() {
    let mut tree: TestTree = TestTree::new();
    // Insert enough keys to trigger multiple splits and rebalances.
    // With N=4, splits happen after every 4th insert into a full leaf.
    // The rebalance may move keys/values to the right sibling.
    let n = 50;
    for i in 0..n {
        let key = vec![(i % 256) as u8; (i % 7) + 1]; // variable-length keys
        tree.insert(key, i).unwrap();
    }

    // Verify all key-value pairs
    for i in 0..n {
        let key = vec![(i % 256) as u8; (i % 7) + 1];
        let val = tree.get(&key).expect(&format!("key {} not found", i));
        assert_eq!(*val, i, "key {} has wrong value", i);
    }
}

/// Test that many sequential inserts produce a valid B+ tree.
#[test]
fn test_var_many_inserts_sequential() {
    let mut tree: TestTree = TestTree::new();
    let n = 200;
    for i in 0..n {
        let key = vec![(i / 256) as u8, (i % 256) as u8];
        tree.insert(key, i).unwrap();
    }
    assert_eq!(tree.len(), n);

    for i in 0..n {
        let key = vec![(i / 256) as u8, (i % 256) as u8];
        assert_eq!(*tree.get(&key).unwrap(), i, "mismatch at key {}", i);
    }
}

/// Test that reverse-order inserts also work with rebalancing.
#[test]
fn test_var_many_inserts_reverse() {
    let mut tree: TestTree = TestTree::new();
    let n = 200;
    for i in (0..n).rev() {
        let key = vec![(i / 256) as u8, (i % 256) as u8];
        tree.insert(key, i).unwrap();
    }
    assert_eq!(tree.len(), n);

    for i in 0..n {
        let key = vec![(i / 256) as u8, (i % 256) as u8];
        assert_eq!(*tree.get(&key).unwrap(), i, "mismatch at key {}", i);
    }
}