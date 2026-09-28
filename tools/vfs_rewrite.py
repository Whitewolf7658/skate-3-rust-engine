"""One-shot mechanical rewrite: std::fs / std::time / Path fs queries -> skate_vfs.

Usage: python tools/vfs_rewrite.py crates/<crate>/src ...
Native behaviour is unchanged (skate_vfs re-exports std::fs and web-time == std::time).
"""
import re
import sys
from pathlib import Path

PATH_METHODS = ["exists", "is_file", "is_dir", "canonicalize", "read_dir"]


def split_top(s):
    """Split a use-tree body on top-level commas."""
    parts, depth, cur = [], 0, ""
    for ch in s:
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
        if ch == "," and depth == 0:
            parts.append(cur)
            cur = ""
        else:
            cur += ch
    if cur.strip():
        parts.append(cur)
    return [p.strip() for p in parts if p.strip()]


def rewrite_std_groups(text):
    out, i = [], 0
    pat = re.compile(r"(pub(?:\([a-z]+\))?\s+)?use std::\{")
    while True:
        m = pat.search(text, i)
        if not m:
            out.append(text[i:])
            break
        start = m.end()
        depth, j = 1, start
        while depth:
            depth += {"{": 1, "}": -1}.get(text[j], 0)
            j += 1
        body = text[start:j - 1]
        end = text.index(";", j) + 1
        vis = m.group(1) or ""
        items = split_top(body)
        keep, extra = [], []
        for it in items:
            if it == "fs":
                extra.append(f"{vis}use skate_vfs as fs;")
            elif it.startswith("fs::"):
                extra.append(f"{vis}use skate_vfs::{it[4:]};")
            elif it.startswith("time::"):
                extra.append(f"{vis}use skate_vfs::time::{it[6:]};")
            elif it == "time":
                extra.append(f"{vis}use skate_vfs::time;")
            else:
                keep.append(it)
        if not extra:
            out.append(text[i:end])
        else:
            out.append(text[i:m.start()])
            stmt = []
            if keep:
                multiline = "\n" in body
                if multiline:
                    stmt.append(f"{vis}use std::{{\n" + "".join(f"    {k},\n" for k in keep) + "};")
                else:
                    stmt.append(f"{vis}use std::{{{', '.join(keep)}}};")
            stmt += extra
            out.append("\n".join(stmt))
        i = end
    return "".join(out)


def rewrite(text):
    text = rewrite_std_groups(text)
    text = re.sub(r"\buse std::fs;", "use skate_vfs as fs;", text)
    text = re.sub(r"\bstd::fs::", "skate_vfs::", text)
    text = re.sub(r"\bstd::fs\b", "skate_vfs", text)
    text = re.sub(r"\bstd::time::", "skate_vfs::time::", text)
    text = re.sub(r"\buse std::time;", "use skate_vfs::time;", text)
    used = False
    for m in PATH_METHODS:
        new, n = re.subn(rf"\.{m}\(\)", f".vfs_{m}()", text)
        if n:
            used = True
            text = new
    if used and "PathExt as _" not in text:
        lines = text.split("\n")
        at = 0
        while at < len(lines) and (lines[at].startswith("//!") or lines[at].startswith("#![") or not lines[at].strip()):
            at += 1
        lines.insert(at, "use skate_vfs::PathExt as _;")
        text = "\n".join(lines)
    return text


def main():
    changed = 0
    for root in sys.argv[1:]:
        for path in Path(root).rglob("*.rs"):
            old = path.read_text(encoding="utf-8")
            new = rewrite(old)
            if new != old:
                path.write_text(new, encoding="utf-8", newline="\n")
                changed += 1
    print(f"rewrote {changed} files")


if __name__ == "__main__":
    main()
