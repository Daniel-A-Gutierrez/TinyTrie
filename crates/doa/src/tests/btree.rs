use super::*;

type Map = BTreeMap<u64, u64>;

#[test]
fn new_empty() {
    let m = Map::new();
    assert!(m.is_empty());
    assert_eq!(m.len(), 0);
    assert!(m.get(&0).is_none());
}

#[test]
fn insert_get() {
    let mut m = Map::new();
    assert!(m.insert(3, 30).is_none());
    assert!(m.insert(1, 10).is_none());
    assert!(m.insert(2, 20).is_none());
    assert_eq!(m.len(), 3);
    assert_eq!(m.get(&1), Some(&10));
    assert_eq!(m.get(&2), Some(&20));
    assert_eq!(m.get(&3), Some(&30));
    assert!(m.get(&4).is_none());
}

#[test]
fn insert_replaces() {
    let mut m = Map::new();
    assert!(m.insert(1, 10).is_none());
    assert_eq!(m.insert(1, 99), Some(10));
    assert_eq!(m.len(), 1);
    assert_eq!(m.get(&1), Some(&99));
}

#[test]
fn get_mut() {
    let mut m = Map::new();
    m.insert(1, 10);
    *m.get_mut(&1).unwrap() = 42;
    assert_eq!(m.get(&1), Some(&42));
}

#[test]
fn remove() {
    let mut m = Map::new();
    for i in 0..5 {
        m.insert(i, i * 10);
    }
    assert_eq!(m.remove(&2), Some(20));
    assert_eq!(m.len(), 4);
    assert!(m.get(&2).is_none());
    assert_eq!(m.get(&0), Some(&0));
    assert_eq!(m.get(&4), Some(&40));
}

#[test]
fn remove_missing() {
    let mut m = Map::new();
    m.insert(1, 10);
    assert!(m.remove(&99).is_none());
    assert_eq!(m.len(), 1);
}

#[test]
fn out_of_order_insertion_sorted_retrieval() {
    let mut m = Map::new();
    let keys = [7, 2, 9, 1, 5, 8, 3, 6, 0, 4];
    for k in keys {
        m.insert(k, k * 10);
    }
    assert_eq!(m.len(), keys.len());
    for k in 0..10u64 {
        assert_eq!(m.get(&k), Some(&(k * 10)));
    }
}

#[test]
fn batch_roundtrip() {
    let mut m = Map::new();
    // < LEAF_MAX (15) so no split fires
    for k in 0..12 {
        m.insert(k, k * k);
    }
    assert_eq!(m.len(), 12);
    for k in 0..12 {
        assert_eq!(m.get(&k), Some(&(k * k)));
    }
    for k in (0..12).step_by(2) {
        assert_eq!(m.remove(&k), Some(k * k));
    }
    assert_eq!(m.len(), 6);
    for k in (1..12).step_by(2) {
        assert_eq!(m.get(&k), Some(&(k * k)));
    }
    for k in (0..12).step_by(2) {
        assert!(m.get(&k).is_none());
    }
}