// Test: does the CPU skip runtime no-ops in the v2p (virt->phys) translation?
// v2p(v) = v.wrapping_add(offset).wrapping_shl(shift).rotate_right(rotation)
// Parameters are runtime values (black_boxed) so the compiler cannot fold
// rotate_right(0) / shl(0) / add(0) into nothing. The CPU must actually issue
// `ror r, 0`, `shl r, 0`, `add r, 0`. If those zero-param loops run as fast as
// the real-work loops, the CPU is NOT skipping no-ops — it executes every uop.
//
// Bonus: v2p_branchy guards each op with a branch so a *predictable* branch
// can skip the no-op instead of executing it. Compared on all-zero (branch
// never taken), all-nonzero (always taken), and 50/50 (mispredicted).
use std::hint::black_box;
use std::time::Instant;
#[derive(Copy, Clone)]
struct Cfg {
    offset:   u8,
    shift:    u32,
    rotation: u32,
    name:     &'static str,
}
#[inline(always)]
fn v2p(v: u8, offset: u8, shift: u32, rotation: u32) -> u8 {
    v.wrapping_add(offset).wrapping_shl(shift).rotate_right(rotation)
}
#[inline(always)]
fn v2p_branchy(v: u8, offset: u8, shift: u32, rotation: u32) -> u8 {
    let v = if offset != 0 { v.wrapping_add(offset) } else { v };
    let v = if shift != 0 { v.wrapping_shl(shift) } else { v };
    if rotation != 0 { v.rotate_right(rotation) } else { v }
}
#[inline(never)]
fn run(arr: &[u8; 256], cfg: Cfg, iters: u64) -> u8 {
    let offset = black_box(cfg.offset);
    let shift = black_box(cfg.shift);
    let rotation = black_box(cfg.rotation);
    let arr = black_box(arr);
    let mut x: u8 = black_box(0);
    for _ in 0..iters {
        x = v2p(arr[x as usize], offset, shift, rotation);
    }
    black_box(x)
}
#[inline(never)]
fn run_branchy(arr: &[u8; 256], cfg: Cfg, iters: u64) -> u8 {
    let offset = black_box(cfg.offset);
    let shift = black_box(cfg.shift);
    let rotation = black_box(cfg.rotation);
    let arr = black_box(arr);
    let mut x: u8 = black_box(0);
    for _ in 0..iters {
        x = v2p_branchy(arr[x as usize], offset, shift, rotation);
    }
    black_box(x)
}
// 50/50 alternating offset: offset flips 0<->17 every iter so the offset guard
// branch mispredicts every time. The floor for "branch NOT predictable."
#[inline(never)]
fn run_branchy_alt(arr: &[u8; 256], shift: u32, rotation: u32, iters: u64) -> u8 {
    let shift = black_box(shift);
    let rotation = black_box(rotation);
    let arr = black_box(arr);
    let mut x: u8 = black_box(0);
    let mut toggle = black_box(false);
    for _ in 0..iters {
        let offset = if toggle { 17 } else { 0 };
        x = v2p_branchy(arr[x as usize], offset, shift, rotation);
        toggle = !toggle;
    }
    black_box(x)
}
fn main() {
    // Fixed pseudo-random permutation of 0..255 (bijection: *5+1 mod 256).
    let mut arr = [0u8; 256];
    let mut i = 0u16;
    while i < 256 {
        arr[i as usize] = ((i.wrapping_mul(5)).wrapping_add(1) & 0xFF) as u8;
        i += 1;
    }
    let iters: u64 = 1 << 28;
    let cfgs = [
        Cfg { offset: 0, shift: 0, rotation: 0, name: "all zero            " },
        Cfg { offset: 17, shift: 3, rotation: 5, name: "all nonzero         " },
    ];
    println!("iters per config = {} ({:.0}M)", iters, iters as f64 / 1e6);
    println!();
    println!("straight v2p (no branch guards):");
    println!("{:24} {:>10} {:>12}", "config", "ms", "ns/iter");
    println!("{}", "-".repeat(48));
    let _ = run(&arr, cfgs[0], 1 << 24); // warmup
    let mut straight = [0f64; 2];
    for (k, cfg) in cfgs.iter().enumerate() {
        let t = Instant::now();
        let out = run(&arr, *cfg, iters);
        let elapsed = t.elapsed();
        let ns = elapsed.as_nanos() as f64 / iters as f64;
        straight[k] = ns;
        println!(
            "{:24} {:>8.1}ms {:>10.3}ns   (x={})",
            cfg.name,
            elapsed.as_secs_f64() * 1e3,
            ns,
            out
        );
    }
    println!();
    println!("branchy v2p (skip op when param==0):");
    println!("{:24} {:>10} {:>12}", "config", "ms", "ns/iter");
    println!("{}", "-".repeat(48));
    let _ = run_branchy(&arr, cfgs[0], 1 << 24); // warmup
    let mut branchy = [0f64; 2];
    for (k, cfg) in cfgs.iter().enumerate() {
        let t = Instant::now();
        let out = run_branchy(&arr, *cfg, iters);
        let elapsed = t.elapsed();
        let ns = elapsed.as_nanos() as f64 / iters as f64;
        branchy[k] = ns;
        println!(
            "{:24} {:>8.1}ms {:>10.3}ns   (x={})",
            cfg.name,
            elapsed.as_secs_f64() * 1e3,
            ns,
            out
        );
    }
    println!();
    println!("branchy, 50/50 alternating offset (mispredict floor):");
    println!("{:24} {:>10} {:>12}", "config", "ms", "ns/iter");
    println!("{}", "-".repeat(48));
    let _ = run_branchy_alt(&arr, 3, 5, 1 << 24); // warmup
    let t = Instant::now();
    let out = run_branchy_alt(&arr, 3, 5, iters);
    let elapsed = t.elapsed();
    let ns_alt = elapsed.as_nanos() as f64 / iters as f64;
    println!(
        "{:24} {:>8.1}ms {:>10.3}ns   (x={})",
        "50/50 mispredict     ",
        elapsed.as_secs_f64() * 1e3,
        ns_alt,
        out
    );
    println!();
    println!("delta (branchy - straight), ns/iter:");
    println!(
        "  all zero   : {:+.3}  (branch never taken -> best case to skip no-ops)",
        branchy[0] - straight[0]
    );
    println!(
        "  all nonzero: {:+.3}  (branch always taken -> pays cmp/jne on top of work)",
        branchy[1] - straight[1]
    );
    println!("  50/50      : {:+.3}  (mispredict floor)", ns_alt - straight[1]);
}
