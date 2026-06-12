#!/usr/bin/env python3
"""Read every text file in the project (excluding .git, target, .claude) and
write all lines to corpus.txt."""

import os

SKIP = {".git", "target", ".claude"}

def should_read(path: str) -> bool:
    # Skip binary-ish extensions and known non-text dirs
    skip_ext = {".png", ".jpg", ".jpeg", ".gif", ".ico", ".woff", ".ttf",
                ".eot", ".svg", ".so", ".o", ".rlib", ".pdb", ".exe",
                ".wasm", ".gz", ".zip", ".tar", ".json", ".lock"}
    _, ext = os.path.splitext(path)
    return ext.lower() not in skip_ext

lines = []
root = os.path.dirname(os.path.abspath(__file__))

for dirpath, dirnames, filenames in os.walk(root):
    # Prune skipped dirs in-place so os.walk doesn't descend
    dirnames[:] = [d for d in dirnames if d not in SKIP]
    for fn in sorted(filenames):
        full = os.path.join(dirpath, fn)
        if not should_read(full):
            continue
        try:
            with open(full, "r", encoding="utf-8", errors="replace") as f:
                for line in f:
                    stripped = line.rstrip("\n\r")
                    if stripped:
                        lines.append(stripped)
        except Exception:
            pass

out = os.path.join(root, "corpus.txt")
with open(out, "w", encoding="utf-8") as f:
    for line in lines:
        f.write(line + "\n")

print(f"Wrote {len(lines)} lines to {out}")