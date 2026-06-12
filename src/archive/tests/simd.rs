use super::*;

fn make_inode_buf(symbols_offset: usize, symbols: &[u8]) -> Vec<u8> {
    let total = symbols_offset + symbols.len();
    let mut buf = vec![0u8; total.max(16).max(32)];
    buf[0] = symbols.len() as u8;
    for (i, &s) in symbols.iter().enumerate() {
        buf[symbols_offset + i] = s;
    }
    buf
}

fn make_disc_buf(discs: &[u8]) -> Vec<u8> {
    let len = discs.len();
    let mut buf = vec![0u8; len.max(16) + 32];
    buf[..len].copy_from_slice(discs);
    buf
}

#[test]
fn test_inode_find_child_basic() {
    let symbols_offset = 2;
    let symbols = [10u8, 20, 30, 40, 50, 60];
    let buf = make_inode_buf(symbols_offset, &symbols);

    assert_eq!(
        inode_find_child(buf.as_ptr(), symbols_offset, 6, 10),
        Some(0)
    );
    assert_eq!(
        inode_find_child(buf.as_ptr(), symbols_offset, 6, 30),
        Some(2)
    );
    assert_eq!(
        inode_find_child(buf.as_ptr(), symbols_offset, 6, 60),
        Some(5)
    );
    assert_eq!(
        inode_find_child(buf.as_ptr(), symbols_offset, 6, 15),
        None
    );
    assert_eq!(
        inode_find_child(buf.as_ptr(), symbols_offset, 6, 5),
        None
    );
    assert_eq!(
        inode_find_child(buf.as_ptr(), symbols_offset, 6, 65),
        None
    );
}

#[test]
fn test_inode_find_child_min_count() {
    let symbols_offset = 2;
    let buf = make_inode_buf(symbols_offset, &[100, 200]);
    assert_eq!(
        inode_find_child(buf.as_ptr(), symbols_offset, 2, 100),
        Some(0)
    );
    assert_eq!(
        inode_find_child(buf.as_ptr(), symbols_offset, 2, 200),
        Some(1)
    );
    assert_eq!(
        inode_find_child(buf.as_ptr(), symbols_offset, 2, 150),
        None
    );
}

#[test]
fn test_inode_find_child_boundary_bytes() {
    let symbols_offset = 2;
    for &byte in &[0x00, 0x7F, 0x80, 0xFF] {
        let buf = make_inode_buf(symbols_offset, &[byte, byte.wrapping_add(1)]);
        assert_eq!(
            inode_find_child(buf.as_ptr(), symbols_offset, 2, byte),
            Some(0)
        );
    }
}

#[test]
fn test_inode_find_child_lower_bound_basic() {
    let symbols_offset = 2;
    let symbols = [10u8, 20, 30, 40, 50, 60];
    let buf = make_inode_buf(symbols_offset, &symbols);

    assert_eq!(
        inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 6, 10),
        0
    );
    assert_eq!(
        inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 6, 30),
        2
    );
    assert_eq!(
        inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 6, 60),
        5
    );
    assert_eq!(
        inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 6, 15),
        1
    );
    assert_eq!(
        inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 6, 5),
        0
    );
    assert_eq!(
        inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 6, 65),
        6
    );
}

#[test]
fn test_inode_find_child_lower_bound_boundary_bytes() {
    let symbols_offset = 2;
    let buf = make_inode_buf(symbols_offset, &[0x00, 0x7F, 0x80, 0xFF]);

    assert_eq!(
        inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 4, 0x00),
        0
    );
    assert_eq!(
        inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 4, 0x7F),
        1
    );
    assert_eq!(
        inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 4, 0x80),
        2
    );
    assert_eq!(
        inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 4, 0xFF),
        3
    );
    assert_eq!(
        inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 4, 0x40),
        1
    );
    assert_eq!(
        inode_find_child_lower_bound(buf.as_ptr(), symbols_offset, 4, 0x90),
        3
    );
}

#[test]
fn test_hnode_find_child_basic() {
    let discs: Vec<u8> = (10..=70).step_by(10).collect();
    let buf = make_disc_buf(&discs);

    assert_eq!(hnode_find_child(buf.as_ptr(), 7, 10), Some(0));
    assert_eq!(hnode_find_child(buf.as_ptr(), 7, 40), Some(3));
    assert_eq!(hnode_find_child(buf.as_ptr(), 7, 70), Some(6));
    assert_eq!(hnode_find_child(buf.as_ptr(), 7, 15), None);
    assert_eq!(hnode_find_child(buf.as_ptr(), 7, 5), None);
    assert_eq!(hnode_find_child(buf.as_ptr(), 7, 75), None);
}

#[test]
fn test_hnode_find_child_chunk_boundaries() {
    // len=16 (exactly one chunk)
    let discs16: Vec<u8> = (0..16).map(|i| i * 10).collect();
    let buf16 = make_disc_buf(&discs16);
    assert_eq!(hnode_find_child(buf16.as_ptr(), 16, 0), Some(0));
    assert_eq!(hnode_find_child(buf16.as_ptr(), 16, 150), Some(15));
    assert_eq!(hnode_find_child(buf16.as_ptr(), 16, 85), None);

    // len=17 (one chunk + 1-byte tail)
    let discs17: Vec<u8> = (0..17).map(|i| i * 10).collect();
    let buf17 = make_disc_buf(&discs17);
    assert_eq!(hnode_find_child(buf17.as_ptr(), 17, 160), Some(16));

    // len=31 (one chunk + 15-byte tail)
    let discs31: Vec<u8> = (0..31).map(|i| i * 8).collect();
    let buf31 = make_disc_buf(&discs31);
    assert_eq!(hnode_find_child(buf31.as_ptr(), 31, 0), Some(0));
    assert_eq!(hnode_find_child(buf31.as_ptr(), 31, 240), Some(30));
}

#[test]
fn test_hnode_find_child_boundary_bytes() {
    let discs = [0x00, 0x40, 0x7F, 0x80, 0xC0, 0xFF];
    let buf = make_disc_buf(&discs);

    for &byte in &discs {
        assert_eq!(
            hnode_find_child(buf.as_ptr(), discs.len(), byte),
            Some(discs.iter().position(|&d| d == byte).unwrap())
        );
    }
}

#[test]
fn test_hnode_find_child_lower_bound_basic() {
    let discs: Vec<u8> = (10..=70).step_by(10).collect();
    let buf = make_disc_buf(&discs);

    assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 7, 10), 0);
    assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 7, 40), 3);
    assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 7, 70), 6);
    assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 7, 15), 1);
    assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 7, 25), 2);
    assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 7, 5), 0);
    assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 7, 75), 7);
}

#[test]
fn test_hnode_find_child_lower_bound_boundary_bytes() {
    let discs = [0x00, 0x40, 0x7F, 0x80, 0xC0, 0xFF];
    let buf = make_disc_buf(&discs);

    assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 6, 0x40), 1);
    assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 6, 0x7F), 2);
    assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 6, 0x80), 3);
    assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 6, 0x90), 4);
    assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 6, 0xFF), 5);
}

#[test]
fn test_children_mask() {
    let mut children = [0u32; 16];
    assert_eq!(children_mask(&children), 0);

    children[0] = 1;
    assert_eq!(children_mask(&children), 0b0000_0000_0000_0001);

    children[7] = 42;
    assert_eq!(children_mask(&children), 0b0000_0000_1000_0001);

    children[15] = 255;
    assert_eq!(children_mask(&children), 0b1000_0000_1000_0001);

    // All non-zero
    for i in 0..16 {
        children[i] = (i + 1) as u32;
    }
    assert_eq!(children_mask(&children), 0xFFFF);
}

#[test]
fn test_children_mask_u16() {
    let mut children = [0u16; 16];
    assert_eq!(children_mask_u16(&children), 0);

    children[0] = 1;
    assert_eq!(children_mask_u16(&children), 0b0000_0000_0000_0001);

    children[7] = 42;
    assert_eq!(children_mask_u16(&children), 0b0000_0000_1000_0001);

    children[15] = 255;
    assert_eq!(children_mask_u16(&children), 0b1000_0000_1000_0001);

    // All non-zero
    for i in 0..16 {
        children[i] = (i + 1) as u16;
    }
    assert_eq!(children_mask_u16(&children), 0xFFFF);
}

#[test]
fn test_children_mask_u64() {
    let mut children = [0u64; 16];
    assert_eq!(children_mask_u64(&children), 0);

    children[0] = 1;
    assert_eq!(children_mask_u64(&children), 0b0000_0000_0000_0001);

    children[7] = 42;
    assert_eq!(children_mask_u64(&children), 0b0000_0000_1000_0001);

    children[15] = 255;
    assert_eq!(children_mask_u64(&children), 0b1000_0000_1000_0001);

    // All non-zero
    for i in 0..16 {
        children[i] = (i + 1) as u64;
    }
    assert_eq!(children_mask_u64(&children), 0xFFFF);
}

#[test]
fn test_children_mask_u8() {
    let mut children = [0u8; 16];
    assert_eq!(children_mask_u8(&children), 0);

    children[0] = 1;
    assert_eq!(children_mask_u8(&children), 0b0000_0000_0000_0001);

    children[7] = 42;
    assert_eq!(children_mask_u8(&children), 0b0000_0000_1000_0001);

    children[15] = 255;
    assert_eq!(children_mask_u8(&children), 0b1000_0000_1000_0001);

    // All non-zero
    for i in 0..16 {
        children[i] = (i + 1) as u8;
    }
    assert_eq!(children_mask_u8(&children), 0xFFFF);
}

#[test]
fn test_hnode_large_discriminants() {
    // 32 discriminants (2 chunks, no tail)
    let discs: Vec<u8> = (0..32).collect();
    let buf = make_disc_buf(&discs);
    for i in 0u8..32 {
        assert_eq!(hnode_find_child(buf.as_ptr(), 32, i), Some(i as usize));
        assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 32, i), i as usize);
    }
    assert_eq!(hnode_find_child(buf.as_ptr(), 32, 32), None);
    assert_eq!(hnode_find_child_lower_bound(buf.as_ptr(), 32, 32), 32);
}