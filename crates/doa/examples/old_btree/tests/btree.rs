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

//Stage 1: leaf split (the 16th insert splits the full root leaf, height 0 -> 1).

#[test]
fn leaf_split_sequential() {
    let mut m = Map::new();
    for k in 0..30u64 {
        assert!(m.insert(k, k * 10).is_none(), "insert {} returned Some", k);
    }
    assert_eq!(m.len(), 30);
    for k in 0..30 {
        assert_eq!(m.get(&k), Some(&(k * 10)), "get {} after split", k);
    }
}

#[test]
fn leaf_split_shuffled() {
    let mut m = Map::new();
    let keys = [
        20, 5, 15, 10, 25, 0, 30, 12, 7, 18, 3, 22, 8, 14, 28, 1, 11, 27, 6, 19, 2, 13, 24, 9,
        16, 29, 4, 21, 26, 17, 23,
    ];
    for &k in &keys {
        m.insert(k, k * 100);
    }
    assert_eq!(m.len(), keys.len());
    for &k in &keys {
        assert_eq!(m.get(&k), Some(&(k * 100)), "get {} after shuffled split", k);
    }
}

#[test]
fn split_then_remove() {
    let mut m = Map::new();
    for k in 0..40u64 {
        m.insert(k, k * 10);
    }
    for k in (0..40).step_by(2) {
        assert_eq!(m.remove(&k), Some(k * 10));
    }
    assert_eq!(m.len(), 20);
    for k in 1..40 {
        if k % 2 == 0 {
            assert!(m.get(&k).is_none());
        } else {
            assert_eq!(m.get(&k), Some(&(k * 10)));
        }
    }
}

//a 2-level tree: several leaves (each split at 15). leaves hold ~7-8 keys after a
//split, so the root inode fills at ~16 leaves (~128 keys) — Stage 1 stayed below that.
//Stage 2: exceed it -> root-inode split (split_root_internal) -> depth 3.
#[test]
fn two_level_tree() {
    let mut m = Map::new();
    for k in 0..100u64 {
        m.insert(k, k * k);
    }
    assert_eq!(m.len(), 100);
    for k in 0..100 {
        assert_eq!(m.get(&k), Some(&(k * k)));
    }
    for k in (0..100).step_by(3) {
        assert_eq!(m.remove(&k), Some(k * k));
    }
    for k in 0..100 {
        if k % 3 == 0 {
            assert!(m.get(&k).is_none());
        } else {
            assert_eq!(m.get(&k), Some(&(k * k)));
        }
    }
}

//depth 3: >128 keys forces the root inode to split (split_root_internal), producing a
//2-level internal tree above the leaves. exercises internal-node split + propagation.
#[test]
fn depth3_sequential() {
    let mut m = Map::new();
    for k in 0..500u64 {
        m.insert(k, k * k);
    }
    assert_eq!(m.len(), 500);
    for k in 0..500 {
        assert_eq!(m.get(&k), Some(&(k * k)), "get {} in depth-3 tree", k);
    }
    for k in (0..500).step_by(5) {
        assert_eq!(m.remove(&k), Some(k * k));
    }
    for k in 0..500 {
        if k % 5 == 0 {
            assert!(m.get(&k).is_none());
        } else {
            assert_eq!(m.get(&k), Some(&(k * k)));
        }
    }
}

#[test]
fn depth3_shuffled() {
    let mut m = Map::new();
    let mut order: Vec<u64> = (0..400).collect();
    let mut s: u64 = 0x9E3779B97F4A7C15;
    for i in (1..order.len()).rev() {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let j = (s >> 33) as usize % (i + 1);
        order.swap(i, j);
    }
    for (i, &k) in order.iter().enumerate() {
        eprintln!("insert #{} k={}", i, k);
        m.insert(k, k * 7);
    }
    assert_eq!(m.len(), order.len());
    for &k in &order {
        assert_eq!(m.get(&k), Some(&(k * 7)), "get {} shuffled depth-3", k);
    }
}