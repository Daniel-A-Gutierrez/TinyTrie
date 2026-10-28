#!/usr/bin/env python3
"""Analyze CTree parameter sweep CSV and produce a markdown summary."""

import csv
import sys
import os

def parse_rate(s):
    """Parse rate strings like '1.04G', '538.22M', '2.31M' into floats (M units)"""
    s = s.strip()
    if s == 'BUILD_FAIL':
        return 0
    if s.endswith('G'):
        return float(s[:-1]) * 1000
    elif s.endswith('M'):
        return float(s[:-1])
    elif s.endswith('K'):
        return float(s[:-1]) / 1000
    else:
        try:
            return float(s)
        except ValueError:
            return 0

def format_rate(val):
    """Format a float in M units back to a human-readable string"""
    if val >= 1000:
        return f"{val/1000:.2f}G"
    elif val >= 1:
        return f"{val:.2f}M"
    else:
        return f"{val*1000:.1f}K"

def main():
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <summary.csv> <output.md> [--is-u64]")
        sys.exit(1)

    csv_path = sys.argv[1]
    md_path = sys.argv[2]
    is_u64 = '--is-u64' in sys.argv

    rows = []
    with open(csv_path) as f:
        reader = csv.DictReader(f)
        for r in reader:
            rows.append(r)

    # Parse values
    for r in rows:
        r['insert_val'] = parse_rate(r.get('insert_1M', '0'))
        r['lookup_val'] = parse_rate(r.get('lookup_1M', '0'))
        r['fwd_val'] = parse_rate(r.get('fwd_1M', '0'))
        r['rev_val'] = parse_rate(r.get('rev_1M', '0'))
        r['mem_val'] = float(r.get('mem_1M', '0')) if r.get('mem_1M', '0') != 'BUILD_FAIL' else 0
        r['insert_opt_val'] = parse_rate(r.get('insert_opt_1M', '0'))
        r['lookup_opt_val'] = parse_rate(r.get('lookup_opt_1M', '0'))
        r['fwd_opt_val'] = parse_rate(r.get('fwd_opt_1M', '0'))
        r['rev_opt_val'] = parse_rate(r.get('rev_opt_1M', '0'))
        if is_u64:
            r['label'] = f"N={r['N']}, NoPreview"
        else:
            r['label'] = f"N={r['N']}, P={r['P']}"

    # Filter out build failures
    valid = [r for r in rows if r['insert_val'] > 0]
    if not valid:
        print("No valid rows to analyze!")
        sys.exit(1)

    # Find baseline (N=4)
    if is_u64:
        baseline = [r for r in valid if r['N'] == '4']
    else:
        baseline = [r for r in valid if r['N'] == '4' and r['P'] == 'u64']
    baseline = baseline[0] if baseline else valid[0]

    # Compute composite score (geometric mean)
    for r in valid:
        r['insert_norm'] = r['insert_val'] / baseline['insert_val'] if baseline['insert_val'] else 0
        r['lookup_norm'] = r['lookup_val'] / baseline['lookup_val'] if baseline['lookup_val'] else 0
        r['fwd_norm'] = r['fwd_val'] / baseline['fwd_val'] if baseline['fwd_val'] else 0
        r['rev_norm'] = r['rev_val'] / baseline['rev_val'] if baseline['rev_val'] else 0
        r['mem_norm'] = baseline['mem_val'] / r['mem_val'] if r['mem_val'] else 0
        norms = [r['insert_norm'], r['lookup_norm'], r['fwd_norm'], r['rev_norm'], r['mem_norm']]
        norms = [n for n in norms if n > 0]
        r['geomean'] = len(norms) and (norms[0]**(1/len(norms)) * norms[-1]**(1/len(norms)))**0.5 if len(norms) >= 2 else 0
        # Proper geometric mean
        import math
        if all(n > 0 for n in norms):
            r['geomean'] = math.exp(sum(math.log(n) for n in norms) / len(norms))
        else:
            r['geomean'] = 0

    sorted_geo = sorted(valid, key=lambda r: r['geomean'], reverse=True)

    key_type = "u64 (fixed-width, NoPreview)" if is_u64 else "Vec<u8> (variable-length, preview)"
    baseline_label = f"N={baseline['N']}, P={baseline['P']}" if not is_u64 else f"N={baseline['N']}, NoPreview"

    lines = []
    lines.append(f"# CTree Parameter Sweep: {key_type}")
    lines.append(f"")
    lines.append(f"**Baseline**: `{baseline_label}` (current default)")
    lines.append(f"")
    lines.append(f"## Raw Results")
    lines.append(f"")
    lines.append(f"| Config | Insert | Lookup | Fwd Iter | Rev Iter | Memory | Insert* | Lookup* | Fwd* | Rev* |")
    lines.append(f"|--------|--------|--------|----------|----------|--------|---------|---------|------|------|")
    for r in sorted_geo:
        lines.append(f"| {r['label']:16s} | {r.get('insert_1M',''):>8s} | {r.get('lookup_1M',''):>8s} | {r.get('fwd_1M',''):>8s} | {r.get('rev_1M',''):>8s} | {r.get('mem_1M',''):>6s} | {r.get('insert_opt_1M',''):>8s} | {r.get('lookup_opt_1M',''):>8s} | {r.get('fwd_opt_1M',''):>8s} | {r.get('rev_opt_1M',''):>8s} |")

    lines.append(f"")
    lines.append(f"## Rankings by Metric (1M keys)")
    lines.append(f"")

    # Forward iteration
    lines.append(f"### Forward Iteration (keys/sec)")
    lines.append(f"")
    lines.append(f"| Rank | Config | Rate | vs Baseline |")
    lines.append(f"|------|--------|------|-------------|")
    sorted_fwd = sorted(valid, key=lambda r: r['fwd_val'], reverse=True)
    for i, r in enumerate(sorted_fwd[:10]):
        delta = (r['fwd_val'] / baseline['fwd_val'] - 1) * 100
        lines.append(f"| {i+1} | {r['label']:16s} | {r['fwd_1M']:>8s} | {delta:+.1f}% |")

    # Lookup
    lines.append(f"")
    lines.append(f"### Lookup (keys/sec)")
    lines.append(f"")
    lines.append(f"| Rank | Config | Rate | vs Baseline |")
    lines.append(f"|------|--------|------|-------------|")
    sorted_lookup = sorted(valid, key=lambda r: r['lookup_val'], reverse=True)
    for i, r in enumerate(sorted_lookup[:10]):
        delta = (r['lookup_val'] / baseline['lookup_val'] - 1) * 100
        lines.append(f"| {i+1} | {r['label']:16s} | {r['lookup_1M']:>8s} | {delta:+.1f}% |")

    # Reverse iteration
    lines.append(f"")
    lines.append(f"### Reverse Iteration (keys/sec)")
    lines.append(f"")
    lines.append(f"| Rank | Config | Rate | vs Baseline |")
    lines.append(f"|------|--------|------|-------------|")
    sorted_rev = sorted(valid, key=lambda r: r['rev_val'], reverse=True)
    for i, r in enumerate(sorted_rev[:10]):
        delta = (r['rev_val'] / baseline['rev_val'] - 1) * 100
        lines.append(f"| {i+1} | {r['label']:16s} | {r['rev_1M']:>8s} | {delta:+.1f}% |")

    # Insertion
    lines.append(f"")
    lines.append(f"### Insertion (keys/sec)")
    lines.append(f"")
    lines.append(f"| Rank | Config | Rate | vs Baseline |")
    lines.append(f"|------|--------|------|-------------|")
    sorted_insert = sorted(valid, key=lambda r: r['insert_val'], reverse=True)
    for i, r in enumerate(sorted_insert[:10]):
        delta = (r['insert_val'] / baseline['insert_val'] - 1) * 100
        lines.append(f"| {i+1} | {r['label']:16s} | {r['insert_1M']:>8s} | {delta:+.1f}% |")

    # Memory
    lines.append(f"")
    lines.append(f"### Memory (bytes/key, lower is better)")
    lines.append(f"")
    lines.append(f"| Rank | Config | Bytes/key | vs Baseline |")
    lines.append(f"|------|--------|-----------|-------------|")
    sorted_mem = sorted(valid, key=lambda r: r['mem_val'])
    for i, r in enumerate(sorted_mem[:10]):
        delta = (r['mem_val'] / baseline['mem_val'] - 1) * 100
        lines.append(f"| {i+1} | {r['label']:16s} | {r['mem_1M']:>6s} | {delta:+.1f}% |")

    # Composite ranking
    lines.append(f"")
    lines.append(f"## Composite Ranking (geometric mean of normalized metrics)")
    lines.append(f"")
    lines.append(f"Normalized to baseline `{baseline_label}` = 1.00x across insert, lookup, fwd, rev, memory.")
    lines.append(f"")
    lines.append(f"| Rank | Config | Geomean | Insert | Lookup | Fwd | Rev | Memory |")
    lines.append(f"|------|--------|---------|--------|--------|-----|-----|--------|")
    for i, r in enumerate(sorted_geo):
        lines.append(f"| {i+1} | {r['label']:16s} | {r['geomean']:.3f} | {r['insert_norm']:.2f}x | {r['lookup_norm']:.2f}x | {r['fwd_norm']:.2f}x | {r['rev_norm']:.2f}x | {r['mem_norm']:.2f}x |")

    lines.append(f"")
    lines.append(f"**Baseline** (`{baseline_label}`): insert={baseline.get('insert_1M','?')}, lookup={baseline.get('lookup_1M','?')}, fwd={baseline.get('fwd_1M','?')}, rev={baseline.get('rev_1M','?')}, mem={baseline.get('mem_1M','?')}")

    md_content = "\n".join(lines)
    with open(md_path, 'w') as f:
        f.write(md_content)

    print(md_content)

if __name__ == '__main__':
    main()