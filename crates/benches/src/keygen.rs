use clap::ValueEnum;
pub use tiny_trie::NonZeroBytes;

pub const SIZES: &[usize] = &[100, 10_000, 1_000_000];

pub fn string_keys(n: usize) -> Vec<Vec<u8>> {
    let w = format!("{}", n - 1).len();
    (0..n).map(|i| format!("key_{i:0>w$}").into_bytes()).collect()
}

pub fn random_keys(n: usize) -> Vec<Vec<u8>> {
    use rand::Rng;
    let mut rng = rand::rng();
    let mut keys = std::collections::BTreeSet::new();
    while keys.len() < n {
        let len = rng.random_range(4..=16);
        let key: Vec<u8> = (0..len).map(|_| rng.random()).collect();
        keys.insert(key);
    }
    keys.into_iter().collect()
}

/// Random unique `u64` keys, shuffled — the shared core of the fixed-width
/// `RandomU64` mode, fed natively to `Benchable<u64>` contestants (the
/// `u64` std structures + `CTree`'s u64 variant). No byte-string projection: the
/// byte/trie contestants skip fixed-width modes entirely.
pub fn random_u64_keys_core(n: usize) -> Vec<u64> {
    use rand::Rng;
    let mut rng = rand::rng();
    let mut keys = std::collections::BTreeSet::new();
    while keys.len() < n {
        let v: u64 = rng.random();
        keys.insert(v);
    }
    let mut keys: Vec<u64> = keys.into_iter().collect();
    rand::seq::SliceRandom::shuffle(&mut keys[..], &mut rng);
    keys
}

/// Descending `0..n` as `u64` — the core of the fixed-width `SeqU64` mode.
pub fn seq_u64_keys_core(n: usize) -> Vec<u64> {
    (0..n as u64).rev().collect()
}

/// Typed `u64` keys for fixed-width modes. Only called when
/// `KeyMode::is_fixed_width()` is true (the harness guards it); other modes
/// are byte-string domains that the byte-keyed contestants handle directly.
pub fn generate_keys_u64(mode: &KeyMode, n: usize) -> Vec<u64> {
    match mode {
        KeyMode::RandomU64 => random_u64_keys_core(n),
        KeyMode::SeqU64 => seq_u64_keys_core(n),
        _ => Vec::new(),
    }
}

/// Non-zero bytestring keys for `Benchable<NonZeroBytes>` contestants
/// (null-terminator tries: BitTrie, PolyTrie). Returns empty for null-byte
/// modes (`Random`/`RandomU64`/`SeqU64`), where no `0x00`-free key set exists —
/// those contestants skip such modes by construction (no keys to build on).
/// For text modes (`Sequential`/`Lines`/`Words`) every key is `0x00`-free, so
/// the `filter_map` keeps them all.
pub fn generate_keys_nonzero(
    mode: &KeyMode,
    n: usize,
    corpus: Option<&[Vec<u8>]>,
) -> Vec<NonZeroBytes> {
    if mode.may_contain_null_bytes() {
        return Vec::new();
    }
    generate_keys(mode, n, corpus)
        .into_iter()
        .filter_map(NonZeroBytes::new)
        .collect()
}

pub fn load_corpus_lines(path: &str) -> Vec<Vec<u8>> {
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("Failed to read corpus '{}': {e}", path);
        std::process::exit(1);
    });
    let mut keys: Vec<Vec<u8>> = data.split(|&b| b == b'\n')
        .map(|line| {
            let mut v = line.to_vec();
            v.truncate(v.len().saturating_sub(1));
            v
        })
        .filter(|line| !line.is_empty())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

pub fn load_corpus_words(path: &str) -> Vec<Vec<u8>> {
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("Failed to read corpus '{}': {e}", path);
        std::process::exit(1);
    });
    let mut keys: Vec<Vec<u8>> = data.split(|&b| b.is_ascii_whitespace())
        .map(|w| w.to_vec())
        .filter(|w| !w.is_empty())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum KeyMode {
    Sequential,
    Random,
    Lines,
    Words,
    RandomU64,
    SeqU64,
}

impl KeyMode {
    /// Returns true for key modes that can produce keys with embedded 0x00 bytes.
    pub fn may_contain_null_bytes(&self) -> bool {
        matches!(self, KeyMode::Random | KeyMode::RandomU64 | KeyMode::SeqU64)
    }

    /// Returns true for fixed-width key modes (8-byte big-endian u64). These
    /// are the natural domain of `CTree<u64>` (SIMD comparison path); the
    /// variable-length `CTree<Box<[u8]>>` path is used for all other modes.
    pub fn is_fixed_width(&self) -> bool {
        matches!(self, KeyMode::RandomU64 | KeyMode::SeqU64)
    }
}

pub fn resolve_sizes(arg: Option<&str>) -> Vec<usize> {
    let all_sizes: Vec<usize> = SIZES.to_vec();
    let Some(arg) = arg else { return all_sizes };
    let arg = arg.trim();

    if let Some((lo_s, hi_s)) = arg.split_once("..") {
        let lo: usize = lo_s.trim().parse().unwrap_or_else(|_| {
            eprintln!("Error: invalid range lower bound '{}'", lo_s.trim());
            std::process::exit(1);
        });
        let hi: usize = hi_s.trim().parse().unwrap_or_else(|_| {
            eprintln!("Error: invalid range upper bound '{}'", hi_s.trim());
            std::process::exit(1);
        });
        if lo > hi {
            eprintln!("Error: range lower bound {lo} > upper bound {hi}");
            std::process::exit(1);
        }
        let filtered: Vec<usize> = all_sizes.iter().filter(|&&s| s >= lo && s <= hi).copied().collect();
        if filtered.is_empty() {
            eprintln!("Error: no canonical sizes in range {lo}..{hi}. Available: {:?}", all_sizes);
            std::process::exit(1);
        }
        return filtered;
    }

    let requested: Vec<usize> = arg.split(',')
        .filter_map(|s| s.trim().parse::<usize>().ok())
        .collect();
    let filtered: Vec<usize> = requested.iter().filter(|s| all_sizes.contains(s)).copied().collect();
    let rejected: Vec<usize> = requested.iter().filter(|s| !all_sizes.contains(s)).copied().collect();
    if !rejected.is_empty() {
        eprintln!("Warning: sizes {rejected:?} not in canonical sizes {:?}, ignoring", all_sizes);
    }
    if filtered.is_empty() {
        eprintln!("Error: no valid sizes remaining. Available: {:?}", all_sizes);
        std::process::exit(1);
    }
    filtered
}

/// Byte-string keys for the variable-length modes (`Sequential`/`Random`/
/// `Lines`/`Words`). The harness only calls this for non-u64 modes — the byte
/// and trie contestants (bytes/nonzero variants) skip fixed-width
/// (`RandomU64`/`SeqU64`) modes entirely, so no byte-string projection of `u64`
/// keys is produced. (The `_ => Vec::new()` arms keep it callable from
/// `generate_keys_nonzero` without panicking if it ever sees a u64 mode.)
pub fn generate_keys(mode: &KeyMode, n: usize, corpus: Option<&[Vec<u8>]>) -> Vec<Vec<u8>> {
    match mode {
        KeyMode::Sequential => string_keys(n),
        KeyMode::Random => random_keys(n),
        KeyMode::Lines | KeyMode::Words => {
            let all = corpus.expect("corpus keys required for words/lines mode");
            all[..n].to_vec()
        }
        _ => Vec::new(),
    }
}
// ── Formatting constants (shared with results) ────────────────────────

pub const COL: usize = 16;
pub const NAME_COL: usize = 22;
