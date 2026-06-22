use crate::tiny_btree::*;

#[test]
fn test_fixed_len_key_find_position_u64() {
    let haystack: [u64; 8] = [10, 20, 30, 40, 50, 60, 70, 80];
    assert_eq!(u64::find_position(&10, &haystack), 0);
    assert_eq!(u64::find_position(&25, &haystack), 2);
    assert_eq!(u64::find_position(&80, &haystack), 7);
    assert_eq!(u64::find_position(&90, &haystack), 8);
    // Partial slice
    assert_eq!(u64::find_position(&55, &haystack[..5]), 5);
    // Empty
    assert_eq!(u64::find_position(&1, &haystack[..0]), 0);
}

#[test]
fn test_fixed_len_key_find_position_u32() {
    let haystack: [u32; 8] = [1, 3, 5, 7, 9, 11, 13, 15];
    assert_eq!(u32::find_position(&5, &haystack), 2);
    assert_eq!(u32::find_position(&6, &haystack), 3);
    assert_eq!(u32::find_position(&0, &haystack), 0);
    assert_eq!(u32::find_position(&16, &haystack), 8);
}

#[test]
fn test_fixed_len_key_find_position_u16() {
    let haystack: [u16; 8] = [100, 200, 300, 400, 500, 600, 700, 800];
    assert_eq!(u16::find_position(&300, &haystack), 2);
    assert_eq!(u16::find_position(&250, &haystack), 2);
    assert_eq!(u16::find_position(&800, &haystack), 7);
    assert_eq!(u16::find_position(&900, &haystack), 8);
}

#[test]
fn test_fixed_len_key_find_position_u8() {
    let haystack: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
    assert_eq!(u8::find_position(&0, &haystack), 0);
    assert_eq!(u8::find_position(&7, &haystack), 7);
    assert_eq!(u8::find_position(&15, &haystack), 15);
    assert_eq!(u8::find_position(&16, &haystack), 16);
}

#[test]
fn test_ctree_insert_and_get() {
    let mut tree: CTree<u64, u64, u16, 4> = CTree::new();
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
    let mut tree: CTree<u64, u64, u16, 4> = CTree::new();
    tree.insert(10, 100).unwrap();
    let err = tree.insert(10, 200);
    assert!(err.is_err());
    assert_eq!(tree.get(&10), Some(&100));
}

#[test]
fn test_var_len_key_box_u8() {
    let k: Box<[u8]> = Box::new([1u8, 2, 3]);
    assert_eq!(k.as_chunks(), &[1u8, 2, 3]);
    assert_eq!(k.chunk_len(), 3);
}