//! NibbleTrie structure analyzer.
//!
//! Builds tries at multiple sizes with random keys, then walks every arena
//! node to collect metrics: fanout distribution, parent-child nibble overlap
//! (for node-stacking analysis), STAK coverage, terminal/leaf-only counts,
//! depth distribution.

use clap::Parser;
use tiny_trie::{NibbleTrie, Node, TrieIndex};
use tiny_trie_bench::keygen::{load_corpus_lines, load_corpus_words};
use rand::Rng;
use std::collections::BTreeSet;

#[derive(Parser)]
#[command(name = "trie-stats", about = "NibbleTrie structure analyzer")]
struct Cli {
    /// Key source mode
    #[arg(short, long, default_value = "random")]
    mode: String,

    /// Path to corpus file (required for --mode words|lines)
    #[arg(short, long)]
    corpus: Option<String>,

    /// Number of keys (default: all corpus keys, or 1000 for random)
    #[arg(short, long)]
    number: Option<usize>,

    /// Show detailed memory breakdown
    #[arg(long)]
    memory: bool,
}

// ---------------------------------------------------------------------------
// Key generation
// ---------------------------------------------------------------------------

fn generate_random_keys(n: usize, min_len: usize, max_len: usize) -> Vec<Vec<u8>> {
    let mut rng = rand::rng();
    let mut keys = BTreeSet::new();
    while keys.len() < n {
        let len = rng.random_range(min_len..=max_len);
        let key: Vec<u8> = (0..len).map(|_| rng.random()).collect();
        keys.insert(key);
    }
    keys.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Stats computation
// ---------------------------------------------------------------------------

struct StatsReport {
    total_nodes: usize,
    key_count: usize,
    avg_depth: f64,
    max_depth: usize,
    depth_histogram: Vec<usize>,
    fanout_histogram: [usize; 17],
    internal_fanout_histogram: [usize; 17],
    leaf_fanout_histogram: [usize; 17],
    overlap_histogram: [usize; 17], // overlap count -> number of (parent, child) pairs
    sibling_overlap_histogram: [usize; 17], // overlap count -> number of sibling pairs
    nodes_with_siblings: usize, // nodes that have ≥2 internal children
    terminal_count: usize,
    leaf_only_count: usize,
    // STAK: how many chain levels absorbed before conflict
    stak_depth_histogram: [usize; 33],
    stak_total_absorbed: usize,
}

fn compute_stats<PTR: TrieIndex, LEN: TrieIndex>(trie: &NibbleTrie<Vec<u8>, usize, PTR, LEN>) -> StatsReport {
    let arena = &trie.arena;
    let total_nodes = arena.len();
    let key_count = trie.len();

    // --- Depth computation via BFS ---
    let mut depth = vec![0usize; total_nodes];
    let mut max_depth = 0usize;
    let mut queue = vec![0usize];
    let mut visited = vec![false; total_nodes];
    visited[0] = true;
    while !queue.is_empty() {
        let mut next_queue = Vec::new();
        for &idx in &queue {
            let node = &arena[idx];
            for nib in 0..16 {
                let child = node.children[nib].as_usize();
                if !node.is_occupied(nib, 0) || node.is_leaf(nib, 0) {
                    continue;
                }
                if !visited[child] {
                    visited[child] = true;
                    depth[child] = depth[idx] + 1;
                    max_depth = max_depth.max(depth[child]);
                    next_queue.push(child);
                }
            }
        }
        queue = next_queue;
    }

    let avg_depth = if total_nodes > 0 {
        depth.iter().sum::<usize>() as f64 / total_nodes as f64
    } else {
        0.0
    };

    let mut depth_histogram = vec![0usize; max_depth + 1];
    for &d in &depth {
        depth_histogram[d] += 1;
    }

    // --- Fanout histograms ---
    let mut fanout_histogram = [0usize; 17];
    let mut internal_fanout_histogram = [0usize; 17];
    let mut leaf_fanout_histogram = [0usize; 17];
    let mut terminal_count = 0usize;
    let mut leaf_only_count = 0usize;

    for node in arena.iter() {
        let cmask = node.children_mask();
        let total_fanout = cmask.count_ones() as usize;
        let leaf_fanout = node.leaf_mask[0].count_ones() as usize;
        let internal_fanout = total_fanout - leaf_fanout;

        fanout_histogram[total_fanout] += 1;
        internal_fanout_histogram[internal_fanout] += 1;
        leaf_fanout_histogram[leaf_fanout] += 1;

        if node.is_terminal(0) {
            terminal_count += 1;
        }
        if cmask == 0 {
            leaf_only_count += 1;
        }
    }

    // --- Parent-child nibble overlap ---
    let mut overlap_histogram = [0usize; 17];
    let mut parent_of = vec![0usize; total_nodes];
    for idx in 0..total_nodes {
        let node = &arena[idx];
        for nib in 0..16 {
            let child = node.children[nib].as_usize();
            if !node.is_occupied(nib, 0) || node.is_leaf(nib, 0) {
                continue;
            }
            parent_of[child] = idx;
        }
    }

    for idx in 1..total_nodes {
        let parent_idx = parent_of[idx];
        let parent_mask = arena[parent_idx].children_mask();
        let child_mask = arena[idx].children_mask();
        let overlap = (parent_mask & child_mask).count_ones() as usize;
        overlap_histogram[overlap] += 1;
    }

    // --- Sibling overlap ---
    let mut sibling_overlap_histogram = [0usize; 17];
    let mut nodes_with_siblings = 0usize;
    for idx in 0..total_nodes {
        let node = &arena[idx];
        let internal_children: Vec<usize> = (0..16)
            .filter(|&nib| node.is_occupied(nib, 0) && !node.is_leaf(nib, 0))
            .map(|nib| node.children[nib].as_usize())
            .collect();
        if internal_children.len() < 2 {
            continue;
        }
        nodes_with_siblings += 1;
        for i in 0..internal_children.len() {
            for j in (i + 1)..internal_children.len() {
                let mask_a = arena[internal_children[i]].children_mask();
                let mask_b = arena[internal_children[j]].children_mask();
                let overlap = (mask_a & mask_b).count_ones() as usize;
                sibling_overlap_histogram[overlap] += 1;
            }
        }
    }

    // --- STAK coverage ---
    let mut stak_depth_histogram = [0usize; 33];
    let mut total_absorbed = 0usize;

    for idx in 0..total_nodes {
        let mut merged_mask = arena[idx].children_mask();
        let mut depth = 0usize;
        let mut cur = idx;
        loop {
            let node = &arena[cur];
            let mut internal_children: Vec<usize> = Vec::new();
            for nib in 0..16 {
                let c = node.children[nib].as_usize();
                if c != 0 && !node.is_leaf(nib, 0) {
                    internal_children.push(c);
                }
            }
            if internal_children.len() != 1 {
                break;
            }
            let child_idx = internal_children[0];
            let child_mask = arena[child_idx].children_mask();
            if merged_mask & child_mask != 0 {
                break;
            }
            merged_mask |= child_mask;
            depth += 1;
            cur = child_idx;
        }
        stak_depth_histogram[depth] += 1;
        total_absorbed += depth;
    }

    StatsReport {
        total_nodes, key_count, avg_depth, max_depth, depth_histogram,
        fanout_histogram, internal_fanout_histogram, leaf_fanout_histogram,
        overlap_histogram, sibling_overlap_histogram, nodes_with_siblings,
        terminal_count, leaf_only_count, stak_depth_histogram,
        stak_total_absorbed: total_absorbed,
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

fn print_report(size: usize, mode: &str, report: &StatsReport) {
    let StatsReport {
        total_nodes, key_count, avg_depth, max_depth, depth_histogram,
        fanout_histogram, internal_fanout_histogram, leaf_fanout_histogram,
        overlap_histogram, sibling_overlap_histogram, nodes_with_siblings,
        terminal_count, leaf_only_count, stak_depth_histogram, stak_total_absorbed,
    } = report;

    println!("=== NibbleTrie Stats: {size} keys [{mode}] ===");
    println!("Arena nodes:    {total_nodes}");
    println!("Key count:      {key_count}");
    println!("Nodes/key:      {:.2}", *total_nodes as f64 / *key_count as f64);
    println!("Avg depth:      {avg_depth:.2}");
    println!("Max depth:      {max_depth}");
    println!();

    println!("Depth distribution:");
    for (d, &count) in depth_histogram.iter().enumerate() {
        if count > 0 {
            let pct = count as f64 / *total_nodes as f64 * 100.0;
            println!("  depth {d:3}: {count:6} ({pct:5.1}%)");
        }
    }
    println!();

    println!("Fanout histogram (total occupied slots per node):");
    for (k, &count) in fanout_histogram.iter().enumerate() {
        if count > 0 {
            let pct = count as f64 / *total_nodes as f64 * 100.0;
            println!("  {k:2} slots: {count:6} ({pct:5.1}%)");
        }
    }
    println!();

    println!("Fanout split:");
    let total_internal: f64 = internal_fanout_histogram.iter().enumerate()
        .map(|(k, &c)| k as f64 * c as f64).sum();
    let total_leaf: f64 = leaf_fanout_histogram.iter().enumerate()
        .map(|(k, &c)| k as f64 * c as f64).sum();
    let avg_internal = total_internal / *total_nodes as f64;
    let avg_leaf = total_leaf / *total_nodes as f64;
    println!("  Avg internal children: {avg_internal:.2}");
    println!("  Avg leaf children:      {avg_leaf:.2}");
    println!();

    let terminal_pct = *terminal_count as f64 / *total_nodes as f64 * 100.0;
    let leaf_only_pct = *leaf_only_count as f64 / *total_nodes as f64 * 100.0;
    println!("Terminal nodes:  {terminal_count:6} ({terminal_pct:.1}%)");
    println!("Leaf-only nodes: {leaf_only_count:6} ({leaf_only_pct:.1}%)");
    println!();

    let internal_edges: usize = overlap_histogram.iter().sum();
    println!("Parent-child nibble overlap ({internal_edges} internal edges):");
    for (k, &count) in overlap_histogram.iter().enumerate() {
        if count > 0 || k == 0 {
            let pct = count as f64 / internal_edges as f64 * 100.0;
            let tag = if k == 0 { " <-- stackable" } else { "" };
            println!("  {k:2} overlap: {count:6} ({pct:5.1}%){tag}");
        }
    }
    println!();

    let sibling_pairs: usize = sibling_overlap_histogram.iter().sum();
    println!("Sibling nibble overlap ({nodes_with_siblings} nodes with ≥2 internal children, {sibling_pairs} pairs):");
    for (k, &count) in sibling_overlap_histogram.iter().enumerate() {
        if count > 0 || k == 0 {
            let pct = if sibling_pairs > 0 { count as f64 / sibling_pairs as f64 * 100.0 } else { 0.0 };
            let tag = if k == 0 { " <-- co-stackable" } else { "" };
            println!("  {k:2} overlap: {count:6} ({pct:5.1}%){tag}");
        }
    }
    println!();

    let absorbable = *total_nodes - stak_depth_histogram[0];
    println!("STAK chain depth (nodes that can absorb their child chain):");
    for (depth, &count) in stak_depth_histogram.iter().enumerate() {
        if count > 0 {
            let pct = count as f64 / *total_nodes as f64 * 100.0;
            let tag = if depth == 0 { " (can't absorb)" } else { "" };
            println!("  depth {depth:2}: {count:6} ({pct:5.1}%){tag}");
        }
    }
    println!("  Total absorbable nodes: {absorbable} ({:.1}%)", absorbable as f64 / *total_nodes as f64 * 100.0);
    println!("  Total nodes absorbed:   {stak_total_absorbed}");
    println!();
}

fn print_memory_breakdown<PTR: TrieIndex, LEN: TrieIndex>(trie: &NibbleTrie<Vec<u8>, usize, PTR, LEN>, n: usize) {
    let node_size = std::mem::size_of::<Node<PTR, LEN>>();
    let arena_nodes = trie.arena.len();
    let arena_cap = trie.arena.capacity();
    let arena_bytes = arena_cap * node_size;
    let buf_bytes = trie.buf.capacity();
    let idx_entry = std::mem::size_of::<(usize, LEN)>();
    let idx_bytes = trie.index.capacity() * idx_entry;
    let val_bytes = trie.values.capacity() * std::mem::size_of::<usize>();
    let total = arena_bytes + buf_bytes + idx_bytes + val_bytes;

    let mut total_slots = 0u64;
    let mut used_slots = 0u64;
    for node in &trie.arena {
        let mask = node.children_mask();
        total_slots += 16;
        used_slots += mask.count_ones() as u64;
    }
    let empty_slots = total_slots - used_slots;
    let waste_bytes = empty_slots * std::mem::size_of::<PTR>() as u64;

    println!("Memory breakdown:");
    println!("  Node size:      {node_size} bytes");
    println!("  Arena:         {arena_bytes:8} bytes ({:>5}/key) — {} nodes (cap {}), {:.0}% of total",
        arena_bytes as f64 / n as f64, arena_nodes, arena_cap, arena_bytes as f64 / total as f64 * 100.0);
    println!("    empty slots: {waste_bytes:8} bytes ({:>5}/key) — {empty_slots}/{total_slots} slots empty ({:.1}%), {:.0}% of arena",
        waste_bytes as f64 / n as f64, empty_slots as f64 / total_slots as f64 * 100.0, waste_bytes as f64 / arena_bytes as f64 * 100.0);
    println!("  Buf:           {buf_bytes:8} bytes ({:>5}/key) — {:.0}% of total",
        buf_bytes as f64 / n as f64, buf_bytes as f64 / total as f64 * 100.0);
    println!("  Index:         {idx_bytes:8} bytes ({:>5}/key) — {} entries × {idx_entry}B, {:.0}% of total",
        idx_bytes as f64 / n as f64, trie.index.len(), idx_bytes as f64 / total as f64 * 100.0);
    println!("  Values:        {val_bytes:8} bytes ({:>5}/key) — {:.0}% of total",
        val_bytes as f64 / n as f64, val_bytes as f64 / total as f64 * 100.0);
    println!("  Total:        {total:8} bytes ({:.1}/key)", total as f64 / n as f64);
    println!();
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();

    let (keys, mode_label) = match cli.mode.as_str() {
        "random" => {
            let n = cli.number.unwrap_or(1000);
            (generate_random_keys(n, 4, 16), "random")
        }
        "lines" => {
            let path = cli.corpus.as_deref().unwrap_or_else(|| {
                eprintln!("--corpus <file> required for --mode lines");
                std::process::exit(1);
            });
            let all_keys = load_corpus_lines(path);
            let n = cli.number.unwrap_or(all_keys.len()).min(all_keys.len());
            eprintln!("Loaded {} unique lines from {}, using {}", all_keys.len(), path, n);
            (all_keys[..n].to_vec(), "lines")
        }
        "words" => {
            let path = cli.corpus.as_deref().unwrap_or_else(|| {
                eprintln!("--corpus <file> required for --mode words");
                std::process::exit(1);
            });
            let all_keys = load_corpus_words(path);
            let n = cli.number.unwrap_or(all_keys.len()).min(all_keys.len());
            eprintln!("Loaded {} unique words from {}, using {}", all_keys.len(), path, n);
            (all_keys[..n].to_vec(), "words")
        }
        other => {
            eprintln!("Unknown mode '{other}'. Use: random, lines, words");
            std::process::exit(1);
        }
    };

    let n = keys.len();
    let mut trie: NibbleTrie<Vec<u8>, usize, u32, u32> = NibbleTrie::new();
    for (i, key) in keys.into_iter().enumerate() {
        trie.insert(key, i).unwrap();
    }
    let report = compute_stats(&trie);
    print_report(n, mode_label, &report);
    if cli.memory {
        print_memory_breakdown(&trie, n);
    }
}