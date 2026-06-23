use clap::ValueEnum;

pub(crate) const SIZES: &[usize] = &[100, 10_000, 1_000_000];

pub(crate) fn string_keys(n: usize) -> Vec<Vec<u8>> {
    let w = format!("{}", n - 1).len();
    (0..n).map(|i| format!("key_{i:0>w$}").into_bytes()).collect()
}

pub(crate) fn random_keys(n: usize) -> Vec<Vec<u8>> {
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

fn u64_to_key(v: u64) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}

pub(crate) fn random_u64_keys(n: usize) -> Vec<Vec<u8>> {
    use rand::Rng;
    let mut rng = rand::rng();
    let mut keys = std::collections::BTreeSet::new();
    while keys.len() < n {
        let v: u64 = rng.random();
        keys.insert(v);
    }
    let mut keys: Vec<Vec<u8>> = keys.into_iter().map(u64_to_key).collect();
    rand::seq::SliceRandom::shuffle(&mut keys[..], &mut rng);
    keys
}

pub(crate) fn seq_u64_keys(n: usize) -> Vec<Vec<u8>> {
    (0..n as u64).rev().map(u64_to_key).collect()
}

pub(crate) fn load_corpus_lines(path: &str) -> Vec<Vec<u8>> {
    tiny_trie::load_corpus_lines(path)
}

pub(crate) fn load_corpus_words(path: &str) -> Vec<Vec<u8>> {
    tiny_trie::load_corpus_words(path)
}

/// Key domain: describes what kind of keys a structure can accept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyDomain {
    Any,      // Compatible with all key modes
    Strings,  // No embedded null bytes (null terminator only at end)
    Variable, // Variable-length keys only — skip fixed-width u64 modes
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum KeyMode {
    Sequential,
    Random,
    Lines,
    Words,
    RandomU64,
    SeqU64,
}

impl KeyMode {
    /// Returns true for key modes that can produce keys with embedded 0x00 bytes.
    pub(crate) fn may_contain_null_bytes(&self) -> bool {
        matches!(self, KeyMode::Random | KeyMode::RandomU64 | KeyMode::SeqU64)
    }

    /// Returns true for fixed-width key modes (8-byte big-endian u64). These
    /// are the natural domain of `CTree<u64>` (SIMD comparison path); the
    /// variable-length `CTree<Box<[u8]>>` path is used for all other modes.
    pub(crate) fn is_fixed_width(&self) -> bool {
        matches!(self, KeyMode::RandomU64 | KeyMode::SeqU64)
    }
}

pub(crate) fn resolve_sizes(arg: Option<&str>) -> Vec<usize> {
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

pub(crate) fn generate_keys(mode: &KeyMode, n: usize, corpus: Option<&[Vec<u8>]>) -> Vec<Vec<u8>> {
    match mode {
        KeyMode::Sequential => string_keys(n),
        KeyMode::Random => random_keys(n),
        KeyMode::Lines | KeyMode::Words => {
            let all = corpus.expect("corpus keys required for words/lines mode");
            all[..n].to_vec()
        }
        KeyMode::RandomU64 => random_u64_keys(n),
        KeyMode::SeqU64 => seq_u64_keys(n),
    }
}