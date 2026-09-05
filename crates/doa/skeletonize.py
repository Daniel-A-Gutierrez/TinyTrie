#!/usr/bin/env python3
"""Skeletonize src/*.rs into doc/*.skeleton.md: drop blank lines, `use` items,
and fn/macro/impl bodies (signatures + doc comments kept), fenced in ```rust.
Each root item gets a `///L####` tag naming its source line."""
import re
from pathlib import Path


def clean(line):
    # crude literal/comment removal: enough for brace counting
    line = re.sub(r"//.*$", "", line)
    line = re.sub(r'"(?:[^"\\]|\\.)*"', '""', line)
    line = re.sub(r"'(?:[^'\\]|\\.)'", "''", line)
    return line


def braces(line):
    c = clean(line)
    return c.count("{") - c.count("}")


def skeleton(src):
    lines = src.splitlines()
    out = []
    pending = []  # depth-0 doc comments + attributes, flushed before their item
    i = 0
    depth = 0
    in_header = False  # one L-tag covers the whole root-item header (incl. where clauses)
    while i < len(lines):
        line = lines[i]
        s = line.strip()
        if not s or (depth == 0 and s.startswith("use ")):
            if depth == 0 and s.startswith("use "):
                while i < len(lines) and not clean(lines[i]).rstrip().endswith(";"):
                    i += 1
            i += 1
            continue
        if depth == 0 and not in_header:
            if s.startswith("///") or s.startswith("#["):
                pending.append(line)
                i += 1
                continue
            if s.startswith("//"):
                out.append(line)
                i += 1
                continue
            # item decl: source-line tag goes before its own doc comments
            out.append(f"///L{i + 1:04d}")
            out.extend(pending)
            pending.clear()
            in_header = True
        # fn/macro_rules signature: gather until body opens, then skip the body
        opener = re.search(r"\b(fn|macro_rules)\b", clean(line))
        # impl blocks: keep the header (with where clauses), drop the whole body
        if depth == 0 and re.match(r"(unsafe\s+)?impl\b", s):
            base = depth
            while i < len(lines):
                b = lines[i].find("{")
                if b >= 0:
                    header = lines[i][:b].rstrip()
                    if header:
                        out.append(header + " {}")
                    else:  # bare `{` line: fold into the last where-clause line
                        out[-1] = out[-1].rstrip().rstrip(",") + " {}"
                    depth += 1
                    i += 1
                    break
                out.append(lines[i])
                i += 1
            while i < len(lines) and depth > base:
                depth += braces(lines[i])
                i += 1
            in_header = False
            continue
        if opener and depth <= 1:
            base = depth
            # emit signature lines up to the opening brace
            while i < len(lines):
                c = clean(lines[i])
                if depth == base and "{" in c:
                    out.append(lines[i].rstrip()[:-1].rstrip() + ";")
                    depth += 1
                    i += 1
                    break
                if depth == base and ";" in c:  # no-body fn (trait decl)
                    out.append(lines[i])
                    i += 1
                    break
                out.append(lines[i])
                depth += braces(lines[i])
                i += 1
                if depth != base and depth != base + 1:
                    break
            # skip body until depth returns to base
            while i < len(lines) and depth > base:
                depth += braces(lines[i])
                i += 1
            in_header = False
            continue
        out.append(line)
        depth += braces(line)
        if in_header and (depth != 0 or clean(line).rstrip().endswith(";")):
            in_header = False
        i += 1
    out.extend(pending)
    return "\n".join(out)


if __name__ == "__main__":
    root = Path(__file__).resolve().parent
    doc = root / "doc"
    doc.mkdir(exist_ok=True)
    for f in sorted((root / "src").glob("*.rs")):
        out = doc / (f.stem + ".md")
        out.write_text("```rust\n" + skeleton(f.read_text()) + "\n```\n")
        print(f"{f.name} -> doc/{out.name}")