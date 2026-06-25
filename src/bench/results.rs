use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use super::{COL, NAME_COL, KeyMode};

// ── Formatting ──────────────────────────────────────────────────────

pub(crate) fn fmt_rate(rate: f64) -> String {
    if rate >= 1e9 {
        format!("{:.2}G", rate / 1e9)
    } else if rate >= 1e6 {
        format!("{:.2}M", rate / 1e6)
    } else if rate >= 1e3 {
        format!("{:.1}K", rate / 1e3)
    } else {
        format!("{:.1}", rate)
    }
}

pub(crate) fn fmt_bytes_per(bytes: f64) -> String {
    if bytes >= 1e3 { format!("{:.0}", bytes) } else { format!("{:.1}", bytes) }
}

// ── Result storage ──────────────────────────────────────────────────

pub(crate) type ResultMap = HashMap<String, Vec<f64>>;

fn fmt_table(title: &str, unit: &str, data: &ResultMap, sizes: &[usize], names: &[&str], fmt_val: fn(f64) -> String, higher_is_better: bool) -> String {
    let active_names: Vec<&str> = names.iter().filter(|n| data.contains_key(*n as &str)).copied().collect();
    if active_names.is_empty() { return String::new(); }
    let mut sorted: Vec<&&str> = active_names.iter().collect();
    sorted.sort_by(|a, b| {
        let va = data.get(**a as &str).and_then(|v| v.first()).unwrap_or(&0.0);
        let vb = data.get(**b as &str).and_then(|v| v.first()).unwrap_or(&0.0);
        if higher_is_better { vb.partial_cmp(va) } else { va.partial_cmp(vb) }
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut s = format!("\n─── {title} ({unit}) ───\n");
    s.push_str(&format!("{:<NAME_COL$}", ""));
    for &sz in sizes { s.push_str(&format!("{:>COL$}", sz)); }
    s.push('\n');
    for name in &sorted {
        s.push_str(&format!("{:<NAME_COL$}", name));
        for &val in data.get(**name as &str).unwrap() { s.push_str(&format!("{:>COL$}", fmt_val(val))); }
        s.push('\n');
    }
    s
}

pub(crate) fn print_table(title: &str, unit: &str, data: &ResultMap, sizes: &[usize], names: &[&str]) {
    let s = fmt_table(title, unit, data, sizes, names, fmt_rate, true);
    if !s.is_empty() { print!("{s}"); }
}

pub(crate) fn print_mem_table(data: &ResultMap, sizes: &[usize], names: &[&str]) {
    let s = fmt_table("Memory", "bytes/key", data, sizes, names, fmt_bytes_per, false);
    if !s.is_empty() { print!("{s}"); }
}

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct ResultsFile {
    pub sizes: Vec<usize>,
    #[serde(default)]
    pub sections: BTreeMap<String, BTreeMap<String, Vec<f64>>>,
}

pub(crate) fn results_paths(key_mode: &KeyMode) -> (String, String) {
    let base = concat!(env!("CARGO_MANIFEST_DIR"), "/benches/");
    let suffix = match key_mode {
        KeyMode::Sequential => "_seq_txt",
        KeyMode::Random => "_random_txt",
        KeyMode::Lines => "_lines",
        KeyMode::Words => "_words",
        KeyMode::RandomU64 => "_random_u64",
        KeyMode::SeqU64 => "_seq_u64",
    };
    (format!("{base}bench_results{suffix}.json"), format!("{base}bench_results{suffix}.md"))
}

pub(crate) fn load_results(json_path: &str) -> ResultsFile {
    match std::fs::read_to_string(json_path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => ResultsFile::default(),
    }
}

pub(crate) fn save_results(data: &ResultsFile, json_path: &str, md_path: &str) {
    let json = serde_json::to_string_pretty(data).unwrap();
    std::fs::write(json_path, &json).unwrap();
    eprintln!("  wrote {json_path}");

    let mut all_sizes: Vec<usize> = Vec::new();
    for (_, rows) in &data.sections {
        for vals in rows.values() {
            for (i, &sz) in data.sizes.iter().enumerate() {
                if i < vals.len() && !all_sizes.contains(&sz) {
                    all_sizes.push(sz);
                }
            }
        }
    }
    all_sizes.sort();

    let mut md = String::new();
    for (section, rows) in &data.sections {
        let is_mem = section.contains("Memory");
        let fmt: fn(f64) -> String = if is_mem { fmt_bytes_per } else { fmt_rate };
        let mut entries: Vec<_> = rows.iter().collect();
        if is_mem {
            entries.sort_by(|a, b| a.1.first().partial_cmp(&b.1.first()).unwrap_or(std::cmp::Ordering::Equal));
        } else {
            entries.sort_by(|a, b| b.1.first().partial_cmp(&a.1.first()).unwrap_or(std::cmp::Ordering::Equal));
        }
        md.push_str(&format!("\n─── {section} ───\n"));
        md.push_str(&format!("{:<NAME_COL$}", ""));
        for &sz in &all_sizes { md.push_str(&format!("{:>COL$}", sz)); }
        md.push('\n');
        for (name, vals) in entries {
            md.push_str(&format!("{:<NAME_COL$}", name));
            for (_i, &sz) in all_sizes.iter().enumerate() {
                if let Some(pos) = data.sizes.iter().position(|&s| s == sz) {
                    if pos < vals.len() {
                        md.push_str(&format!("{:>COL$}", fmt(vals[pos])));
                    } else {
                        md.push_str(&format!("{:>COL$}", ""));
                    }
                } else {
                    md.push_str(&format!("{:>COL$}", ""));
                }
            }
            md.push('\n');
        }
    }
    if !md.is_empty() { md.push('\n'); }
    std::fs::write(md_path, &md).unwrap();
    eprintln!("  wrote {md_path}");
}

pub(crate) fn merge_results(data: &mut ResultsFile, section: &str, new: &ResultMap, run_sizes: &[usize]) {
    let sec = data.sections.entry(section.to_string()).or_default();
    for (name, values) in new {
        if let Some(existing) = sec.get_mut(name) {
            for (i, &sz) in run_sizes.iter().enumerate() {
                if let Some(pos) = data.sizes.iter().position(|&s| s == sz) {
                    if i < values.len() {
                        while existing.len() <= pos { existing.push(0.0); }
                        existing[pos] = values[i];
                    }
                }
            }
        } else {
            let mut row = vec![0.0; data.sizes.len()];
            for (i, &sz) in run_sizes.iter().enumerate() {
                if let Some(pos) = data.sizes.iter().position(|&s| s == sz) {
                    if i < values.len() && pos < row.len() {
                        row[pos] = values[i];
                    }
                }
            }
            sec.insert(name.clone(), row);
        }
    }
}