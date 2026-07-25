// Test 3: fn-ptr specialization. A "block" stores its v2p as a fn ptr chosen by
// set_params to a pre-baked specialized body that skips zero-param ops
// (straight-line, no per-iter branch). The indirect call target is stable for
// the life of the loop -> BTB-predicted. Question: does the call/ret latency
// (on the critical x-dependency chain) beat the inline-branchy win, or eat it?
use std::hint::black_box;
use std::time::Instant;
type V2p = fn(u8, u8, u32, u32) -> u8;
// 8 pre-baked specialized bodies. Each ignores the args it doesn't need.
fn f_id(x: u8, _o: u8, _s: u32, _r: u32) -> u8 {
    x
}
fn f_o(x: u8, o: u8, _s: u32, _r: u32) -> u8 {
    x.wrapping_add(o)
}
fn f_s(x: u8, _o: u8, s: u32, _r: u32) -> u8 {
    x.wrapping_shl(s)
}
fn f_r(x: u8, _o: u8, _s: u32, r: u32) -> u8 {
    x.rotate_right(r)
}
fn f_os(x: u8, o: u8, s: u32, _r: u32) -> u8 {
    x.wrapping_add(o).wrapping_shl(s)
}
fn f_or(x: u8, o: u8, _s: u32, r: u32) -> u8 {
    x.wrapping_add(o).rotate_right(r)
}
fn f_sr(x: u8, _o: u8, s: u32, r: u32) -> u8 {
    x.wrapping_shl(s).rotate_right(r)
}
fn f_osr(x: u8, o: u8, s: u32, r: u32) -> u8 {
    x.wrapping_add(o).wrapping_shl(s).rotate_right(r)
}
struct Block {
    offset:   u8,
    shift:    u32,
    rotation: u32,
    v2p:      V2p,
}
impl Block {
    fn set_params(&mut self, offset: u8, shift: u32, rotation: u32) {
        self.offset = offset;
        self.shift = shift;
        self.rotation = rotation;
        self.v2p = match (offset != 0, shift != 0, rotation != 0) {
            (false, false, false) => f_id,
            (true, false, false) => f_o,
            (false, true, false) => f_s,
            (false, false, true) => f_r,
            (true, true, false) => f_os,
            (true, false, true) => f_or,
            (false, true, true) => f_sr,
            (true, true, true) => f_osr,
        };
    }
}
#[inline(never)]
fn run_fnptr(arr: &[u8; 256], offset: u8, shift: u32, rotation: u32, iters: u64) -> u8 {
    // build the block; black_box params so nothing folds
    let mut blk = Block { offset: 0, shift: 0, rotation: 0, v2p: f_id };
    blk.set_params(black_box(offset), black_box(shift), black_box(rotation));
    let blk = black_box(&blk);
    let arr = black_box(arr);
    // fn ptr is opaque (came through black_box) -> real indirect call, not inlined.
    let v2p = blk.v2p;
    let o = blk.offset;
    let s = blk.shift;
    let r = blk.rotation;
    let mut x: u8 = black_box(0);
    for _ in 0..iters {
        x = v2p(arr[x as usize], o, s, r);
    }
    black_box(x)
}
// baselines, same as before, for direct comparison in one run
#[inline(always)]
fn v2p_straight(v: u8, o: u8, s: u32, r: u32) -> u8 {
    v.wrapping_add(o).wrapping_shl(s).rotate_right(r)
}
#[inline(never)]
fn run_straight(arr: &[u8; 256], o: u8, s: u32, r: u32, iters: u64) -> u8 {
    let o = black_box(o);
    let s = black_box(s);
    let r = black_box(r);
    let arr = black_box(arr);
    let mut x: u8 = black_box(0);
    for _ in 0..iters {
        x = v2p_straight(arr[x as usize], o, s, r);
    }
    black_box(x)
}
#[inline(always)]
fn v2p_branchy(v: u8, o: u8, s: u32, r: u32) -> u8 {
    let v = if o != 0 { v.wrapping_add(o) } else { v };
    let v = if s != 0 { v.wrapping_shl(s) } else { v };
    if r != 0 { v.rotate_right(r) } else { v }
}
#[inline(never)]
fn run_branchy(arr: &[u8; 256], o: u8, s: u32, r: u32, iters: u64) -> u8 {
    let o = black_box(o);
    let s = black_box(s);
    let r = black_box(r);
    let arr = black_box(arr);
    let mut x: u8 = black_box(0);
    for _ in 0..iters {
        x = v2p_branchy(arr[x as usize], o, s, r);
    }
    black_box(x)
}
fn time<F: Fn() -> u8>(name: &str, iters: u64, f: F) -> f64 {
    let _ = f(); // warmup (small)
    // re-warm at full-ish scale to prime BTB/RSB/iCache for the fn-ptr path
    let _ = f();
    let t = Instant::now();
    let out = f();
    let elapsed = t.elapsed();
    let ns = elapsed.as_nanos() as f64 / iters as f64;
    println!("{:26} {:>8.1}ms {:>9.3}ns   (x={})", name, elapsed.as_secs_f64() * 1e3, ns, out);
    ns
}
fn main() {
    let mut arr = [0u8; 256];
    let mut i = 0u16;
    while i < 256 {
        arr[i as usize] = ((i.wrapping_mul(5)).wrapping_add(1) & 0xFF) as u8;
        i += 1;
    }
    let iters: u64 = 1 << 28;
    println!("iters = {} ({:.0}M)\n", iters, iters as f64 / 1e6);
    for (label, o, s, r) in [("all zero", 0u8, 0u32, 0u32), ("all nonzero", 17u8, 3u32, 5u32)] {
        println!("=== {} (o={}, s={}, r={}) ===", label, o, s, r);
        let st = time("straight (inline)", iters, || run_straight(&arr, o, s, r, iters));
        let br = time("branchy (inline)", iters, || run_branchy(&arr, o, s, r, iters));
        let fp = time("fn-ptr specialize", iters, || run_fnptr(&arr, o, s, r, iters));
        println!("  fn-ptr vs straight: {:+.3} ns", fp - st);
        println!("  fn-ptr vs branchy : {:+.3} ns\n", fp - br);
    }
}
