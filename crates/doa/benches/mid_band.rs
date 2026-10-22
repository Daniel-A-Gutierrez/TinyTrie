//!mid-band run asymmetry: a dense 128-append block, then a descending
//!128-key band landing inside the previous range, then an ascending one
//!inside THAT — per-phase ns/insert + moved elements. question: do the
//!reverse-sorted band inserts pay materially more than the sorted ones?

use std::time::Instant;

use doa::blockset::BlockSet;

const RUNS: usize = 11;

fn main() {
    //base: 128 ascending appends (stride 8, [0, 1024)) — dense split-block
    //geometry. band 2: 128 keys stride 4 in (256, 768), inserted DESCENDING
//(each lands left of the previous — mid-block, no beyond-tail path).
    //band 3: 128 keys stride 2 (odds, (385, 639)), inserted ASCENDING,
    //nested inside band 2's range
    let base: Vec<u64> = (0..128).map(|i| i as u64 * 8).collect();
    let band2: Vec<u64> = (0..128u64).rev().map(|i| 258 + i * 4).collect();
    let band3: Vec<u64> = (0..128).map(|i| 385 + i as u64 * 2).collect();

    //each band measured on its own fresh prefill — nested bands
    //contaminate (band3 inherits band2's churn layout)
    let run_band = |name: &str, band: &[u64], warm: bool| {
        let mut ns = 0f64;
        let mut moved = 0usize;
        let mut set = BlockSet::<u64, u16>::new();
        if warm {
            for &k in &base {
                set.insert(k).unwrap();
                set.deltas().clear();
            }
        }
        let t = Instant::now();
        for &k in band {
            set.insert(k).unwrap();
            moved += set.deltas().iter().count();
        }
        ns = t.elapsed().as_nanos() as f64 / band.len() as f64;
        let keys: Vec<u64> = set.iter().copied().collect();
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "order broken");
        println!("  {name:>16}: {:>8.1} ns/ins | moved/ins {:>6.2}", ns, moved as f64 / band.len() as f64);
    };
    for warm in [false, true] {
        run_band("desc band fresh", &band2, warm);
        run_band("asc band fresh", &band3, warm);
    }
    let mut phase_ns = [[0f64; 3]; RUNS];
    let mut phase_moved = [[0usize; 3]; RUNS];
    let mut flips = 0usize;
    let mut prev_rev = false;
    for run in 0..RUNS {
        let mut set = BlockSet::<u64, u16>::new();
        let mut moved = 0usize;
        let t = Instant::now();
        for &k in &base {
            set.insert(k).unwrap();
            moved += set.deltas().iter().count();
        }
        phase_ns[run][0] = t.elapsed().as_nanos() as f64 / base.len() as f64;
        phase_moved[run][0] = moved;
        for (phase, keys) in [(1, &band2), (2, &band3)] {
            let mut moved = 0usize;
            let t = Instant::now();
            for &k in keys {
                set.insert(k).unwrap();
                moved += set.deltas().iter().count();
                if set.reversed() != prev_rev {
                    prev_rev = set.reversed();
                    flips += 1;
                }
            }
            phase_ns[run][phase] = t.elapsed().as_nanos() as f64 / keys.len() as f64;
            phase_moved[run][phase] = moved;
        }
        //validity: all 384 present, ordered
        assert_eq!(set.len(), 384);
        let keys: Vec<u64> = set.iter().copied().collect();
        assert_eq!(keys.first(), Some(&0));
        assert_eq!(keys.last(), Some(&1016));
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
    }
    let med = |a: &[[f64; 3]; RUNS], p: usize| {
        let mut v: Vec<f64> = a.iter().map(|r| r[p]).collect();
        v.sort_by(f64::total_cmp);
        v[RUNS / 2]
    };
    let med_moved = |a: &[[usize; 3]; RUNS], p: usize| {
        let mut v: Vec<usize> = a.iter().map(|r| r[p]).collect();
        v.sort_unstable();
        v[RUNS / 2]
    };
    println!("block: 128-append prefill (dense, cap 128) + 128 desc band + 128 asc band");
    println!("  append prefill : {:>8.1} ns/ins | moved/ins {:>6.2}", med(&phase_ns, 0), med_moved(&phase_moved, 0) as f64 / 128.0);
    println!("  desc mid band  : {:>8.1} ns/ins | moved/ins {:>6.2}", med(&phase_ns, 1), med_moved(&phase_moved, 1) as f64 / 128.0);
    println!("  asc mid band   : {:>8.1} ns/ins | moved/ins {:>6.2}", med(&phase_ns, 2), med_moved(&phase_moved, 2) as f64 / 128.0);
    println!("  flips across the fill: {flips}");
}