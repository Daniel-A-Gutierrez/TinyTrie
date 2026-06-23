use crate::tiny_array::TinyArray;

#[test]
fn test_insert_and_get() {
    let mut arr: TinyArray<u64, 8> = TinyArray::new();
    arr.insert_at(0, 10);
    arr.insert_at(1, 20);
    arr.insert_at(2, 30);
    assert_eq!(arr.len(), 3);
    assert_eq!(*arr.get(0), 10);
    assert_eq!(*arr.get(1), 20);
    assert_eq!(*arr.get(2), 30);
}

#[test]
fn test_insert_middle() {
    let mut arr: TinyArray<u64, 8> = TinyArray::new();
    arr.insert_at(0, 10);
    arr.insert_at(1, 30);
    arr.insert_at(1, 20); // insert 20 between 10 and 30
    assert_eq!(arr.as_slice(), &[10, 20, 30]);
}

#[test]
fn test_remove_at() {
    let mut arr: TinyArray<u64, 8> = TinyArray::new();
    arr.insert_at(0, 10);
    arr.insert_at(1, 20);
    arr.insert_at(2, 30);
    let val = arr.remove_at(1);
    assert_eq!(val, 20);
    assert_eq!(arr.as_slice(), &[10, 30]);
    assert_eq!(arr.len(), 2);
}

#[test]
fn test_push() {
    let mut arr: TinyArray<u64, 4> = TinyArray::new();
    arr.push(1);
    arr.push(2);
    arr.push(3);
    assert_eq!(arr.as_slice(), &[1, 2, 3]);
}

#[test]
fn test_truncate() {
    let mut arr: TinyArray<u64, 8> = TinyArray::new();
    arr.push(10);
    arr.push(20);
    arr.push(30);
    arr.truncate(1);
    assert_eq!(arr.len(), 1);
    assert_eq!(*arr.get(0), 10);
}

#[test]
fn test_is_full() {
    let mut arr: TinyArray<u64, 2> = TinyArray::new();
    assert!(!arr.is_full());
    arr.push(1);
    assert!(!arr.is_full());
    arr.push(2);
    assert!(arr.is_full());
}

#[test]
fn test_drop_non_copy() {
    let mut arr: TinyArray<String, 4> = TinyArray::new();
    arr.push("hello".to_string());
    arr.push("world".to_string());
    assert_eq!(arr.len(), 2);
    // Drop should free the Strings
    drop(arr);
}

#[test]
fn test_box_keys() {
    let mut arr: TinyArray<Box<[u8]>, 4> = TinyArray::new();
    arr.push(Box::new([1u8, 2, 3]));
    arr.push(Box::new([4u8, 5, 6]));
    assert_eq!(arr.len(), 2);
    assert_eq!(arr.get(0).as_ref(), &[1u8, 2, 3][..]);
    assert_eq!(arr.get(1).as_ref(), &[4u8, 5, 6][..]);
    // Drop should free the Boxes
    drop(arr);
}

#[test]
fn test_drain_into() {
    let mut src: TinyArray<u64, 8> = TinyArray::new();
    src.push(10);
    src.push(20);
    src.push(30);
    src.push(40);
    src.push(50);

    let mut dst: TinyArray<u64, 8> = TinyArray::new();
    src.drain_into(2, &mut dst);

    assert_eq!(src.as_slice(), &[10, 20]);
    assert_eq!(src.len(), 2);
    assert_eq!(dst.as_slice(), &[30, 40, 50]);
    assert_eq!(dst.len(), 3);
}

#[test]
fn test_drain_into_at_boundary() {
    let mut src: TinyArray<u64, 4> = TinyArray::new();
    src.push(1);
    src.push(2);
    src.push(3);
    src.push(4);

    let mut dst: TinyArray<u64, 4> = TinyArray::new();
    src.drain_into(0, &mut dst);

    assert_eq!(src.len(), 0);
    assert_eq!(dst.as_slice(), &[1, 2, 3, 4]);
}

#[test]
fn test_drain_into_non_copy() {
    let mut src: TinyArray<String, 4> = TinyArray::new();
    src.push("a".to_string());
    src.push("b".to_string());
    src.push("c".to_string());

    let mut dst: TinyArray<String, 4> = TinyArray::new();
    src.drain_into(1, &mut dst);

    assert_eq!(src.as_slice(), &["a".to_string()]);
    assert_eq!(dst.as_slice(), &["b".to_string(), "c".to_string()]);
    // src truncated to 1, dst has 2 — both drop correctly
}