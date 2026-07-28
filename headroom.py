#!/usr/bin/env python3
"""Ensure a blank line above each item header (fn/trait/impl/struct/enum),
including its preceding comment (`//`,`///`,`//!`) and attribute (`#[...]`)
block. The blank goes above the first such line directly above the header.
Goes off headers, not braces."""
import re, sys

HEADER = re.compile(
    r'^\s*(pub(\(|\s+|\s\(crate\))? )?'
    r'(fn|trait|impl|struct|enum)'
)
ATTACHED = re.compile(r'^\s*(//|#!?\[)')

def is_block_start(lines, i):
    """Line i is the first line of an item block (header or its leading
    doc/attr), i.e. header-or-attached with no attached line immediately above."""
    if not (HEADER.match(lines[i]) or ATTACHED.match(lines[i])):
        return False
    return i == 0 or not ATTACHED.match(lines[i - 1])

def process(text):
    lines = text.split("\n")
    out = []
    for i, line in enumerate(lines):
        if is_block_start(lines, i):
            while out and out[-1] == "":
                out.pop()
            if out:
                out.append("")
        out.append(line)
    return "\n".join(out)

def main():
    if len(sys.argv) > 1:
        for path in sys.argv[1:]:
            with open(path) as f:
                new = process(f.read())
            with open(path, "w") as f:
                f.write(new)
    else:
        sys.stdout.write(process(sys.stdin.read()))

if __name__ == "__main__":
    main()