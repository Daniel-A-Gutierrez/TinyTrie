//! NibbleTrie structure analyzer.
//!
//! Builds tries at multiple sizes with random keys, then walks every arena
//! node to collect metrics: fanout distribution, parent-child nibble overlap
//! (for node-stacking analysis), STAK coverage, terminal/leaf-only counts,
//! depth distribution.

use tiny_trie::{NibbleTrie, Node, TrieIndex, load_corpus_lines, load_corpus_words};
use rand::Rng;
use std::collections::BTreeSet;

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

fn compute_stats<PTR: TrieIndex, LEN: TrieIndex>(trie: &NibbleTrie<usize, PTR, LEN>) -> StatsReport {
    let arena = &trie.arena;
    let total_nodes = arena.len();
    let key_count = trie.len();

    // --- Depth computation via BFS ---
    let mut depth = vec![0usize; total_nodes];
    let mut max_depth = 0usize;
    // BFS from root
    let mut queue = vec![0usize];
    let mut visited = vec![false; total_nodes];
    visited[0] = true;
    while !queue.is_empty() {
        let mut next_queue = Vec::new();
        for &idx in &queue {
            let node = &arena[idx];
            for nib in 0..16 {
                let child = node.children[nib].as_usize();
                if child == 0 || node.is_leaf(nib) {
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
        let leaf_fanout = node.leaf_mask.count_ones() as usize;
        let internal_fanout = total_fanout - leaf_fanout;

        fanout_histogram[total_fanout] += 1;
        internal_fanout_histogram[internal_fanout] += 1;
        leaf_fanout_histogram[leaf_fanout] += 1;

        if node.is_terminal() {
            terminal_count += 1;
        }
        if cmask == 0 {
            leaf_only_count += 1;
        }
    }

    // --- Parent-child nibble overlap ---
    let mut overlap_histogram = [0usize; 17];
    // Build parent_of map: for each node, which arena index is its parent?
    let mut parent_of = vec![0usize; total_nodes]; // default root=0 (self)
    for idx in 0..total_nodes {
        let node = &arena[idx];
        for nib in 0..16 {
            let child = node.children[nib].as_usize();
            if child == 0 || node.is_leaf(nib) {
                continue;
            }
            parent_of[child] = idx;
        }
    }

    // For each non-root node, compute overlap with parent
    for idx in 1..total_nodes {
        let parent_idx = parent_of[idx];
        let parent_mask = arena[parent_idx].children_mask();
        let child_mask = arena[idx].children_mask();
        let overlap = (parent_mask & child_mask).count_ones() as usize;
        overlap_histogram[overlap] += 1;
    }

    // --- Sibling overlap ---
    // For each node with ≥2 internal children, compute pairwise overlap
    // between every pair of internal children.
    let mut sibling_overlap_histogram = [0usize; 17];
    let mut nodes_with_siblings = 0usize;
    for idx in 0..total_nodes {
        let node = &arena[idx];
        let internal_children: Vec<usize> = (0..16)
            .filter(|&nib| node.children[nib].as_usize() != 0 && !node.is_leaf(nib))
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
    // For each node, follow its chain of single-internal-child descendants
    // and absorb until we hit a child whose nibble slots conflict. Report
    // how many levels deep each chain goes before stopping.
    let mut stak_depth_histogram = [0usize; 33]; // depth 0..=32
    let mut total_absorbed = 0usize;

    for idx in 0..total_nodes {
        let mut merged_mask = arena[idx].children_mask();
        let mut depth = 0usize;
        let mut cur = idx;
        loop {
            // Find the single internal child (if any)
            let node = &arena[cur];
            let mut internal_children: Vec<usize> = Vec::new();
            for nib in 0..16 {
                let c = node.children[nib].as_usize();
                if c != 0 && !node.is_leaf(nib) {
                    internal_children.push(c);
                }
            }
            // Only follow if there's exactly one internal child
            if internal_children.len() != 1 {
                break;
            }
            let child_idx = internal_children[0];
            let child_mask = arena[child_idx].children_mask();
            if merged_mask & child_mask != 0 {
                break; // conflict — stop
            }
            merged_mask |= child_mask;
            depth += 1;
            cur = child_idx;
        }
        stak_depth_histogram[depth] += 1;
        total_absorbed += depth;
    }

    StatsReport {
        total_nodes,
        key_count,
        avg_depth,
        max_depth,
        depth_histogram,
        fanout_histogram,
        internal_fanout_histogram,
        leaf_fanout_histogram,
        overlap_histogram,
        sibling_overlap_histogram,
        nodes_with_siblings,
        terminal_count,
        leaf_only_count,
        stak_depth_histogram,
        stak_total_absorbed: total_absorbed,
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

fn print_report(size: usize, mode: &str, report: &StatsReport) {
    let StatsReport {
        total_nodes,
        key_count,
        avg_depth,
        max_depth,
        depth_histogram,
        fanout_histogram,
        internal_fanout_histogram,
        leaf_fanout_histogram,
        overlap_histogram,
        sibling_overlap_histogram,
        nodes_with_siblings,
        terminal_count,
        leaf_only_count,
        stak_depth_histogram,
        stak_total_absorbed,
    } = report;

    println!("=== NibbleTrie Stats: {size} keys [{mode}] ===");
    println!("Arena nodes:    {total_nodes}");
    println!("Key count:      {key_count}");
    println!("Nodes/key:      {:.2}", *total_nodes as f64 / *key_count as f64);
    println!("Avg depth:      {avg_depth:.2}");
    println!("Max depth:      {max_depth}");
    println!();

    // Depth distribution
    println!("Depth distribution:");
    for (d, &count) in depth_histogram.iter().enumerate() {
        if count > 0 {
            let pct = count as f64 / *total_nodes as f64 * 100.0;
            println!("  depth {d:3}: {count:6} ({pct:5.1}%)");
        }
    }
    println!();

    // Fanout histogram
    println!("Fanout histogram (total occupied slots per node):");
    for (k, &count) in fanout_histogram.iter().enumerate() {
        if count > 0 {
            let pct = count as f64 / *total_nodes as f64 * 100.0;
            println!("  {k:2} slots: {count:6} ({pct:5.1}%)");
        }
    }
    println!();

    // Fanout split
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

    // Terminal / leaf-only
    let terminal_pct = *terminal_count as f64 / *total_nodes as f64 * 100.0;
    let leaf_only_pct = *leaf_only_count as f64 / *total_nodes as f64 * 100.0;
    println!("Terminal nodes:  {terminal_count:6} ({terminal_pct:.1}%)");
    println!("Leaf-only nodes: {leaf_only_count:6} ({leaf_only_pct:.1}%)");
    println!();

    // Parent-child overlap
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

    // Sibling overlap
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

    // STAK chain depth
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

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn print_memory_breakdown<PTR: TrieIndex, LEN: TrieIndex>(trie: &NibbleTrie<usize, PTR, LEN>, n: usize) {
    let node_size = std::mem::size_of::<Node<PTR, LEN>>();
    let arena_nodes = trie.arena.len();
    let arena_cap = trie.arena.capacity();
    let arena_bytes = arena_cap * node_size;
    let buf_bytes = trie.buf.capacity();
    let idx_entry = std::mem::size_of::<(usize, LEN)>();
    let idx_bytes = trie.index.capacity() * idx_entry;
    let val_bytes = trie.values.capacity() * std::mem::size_of::<usize>();
    let total = arena_bytes + buf_bytes + idx_bytes + val_bytes;

    // Children waste
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

fn load_corpus_keys(path: &str) -> Vec<Vec<u8>> {
    load_corpus_lines(path)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() > 1 {
        // Usage: trie-stats <corpus_file> [max_keys] [--words] [--memory]
        let words_mode = args.iter().any(|a| a == "--words");
        let show_memory = args.iter().any(|a| a == "--memory");
        let positional: Vec<&String> = args[1..].iter().filter(|a| !a.starts_with('-')).collect();
        let path = positional[0];
        let max_keys: usize = positional.get(1).map(|s| s.parse().unwrap()).unwrap_or(usize::MAX);
        let all_keys = if words_mode { load_corpus_words(path) } else { load_corpus_keys(path) };
        let mode_label = if words_mode { "words" } else { "lines" };
        let n = max_keys.min(all_keys.len());
        eprintln!("Loaded {} unique {mode_label} from {}, using {}", all_keys.len(), path, n);

        let keys: Vec<Vec<u8>> = all_keys[..n].to_vec();
        let mut trie: NibbleTrie<usize, u32, u32> = NibbleTrie::new();
        for (i, key) in keys.into_iter().enumerate() {
            trie.insert(key, i).unwrap();
        }
        let report = compute_stats(&trie);
        print_report(n, mode_label, &report);
        if show_memory {
            print_memory_breakdown(&trie, n);
        }
    } else {
        // Default: random keys
        let sizes = [1_000, 10_000, 100_000, 1_000_000];
        for &size in &sizes {
            let keys = generate_random_keys(size, 4, 16);
            let mut trie: NibbleTrie<usize, u32, u32> = NibbleTrie::new();
            for (i, key) in keys.into_iter().enumerate() {
                trie.insert(key, i).unwrap();
            }
            let report = compute_stats(&trie);
            print_report(size, "random", &report);
        }
    }
}