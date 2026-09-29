"""Pack an installed asset directory for the web build.

    python tools/web_pack.py <installed pack dir> <site dir> [--default NAME] [--maps A,B] [--part-mb 95]

<installed pack dir> contains assets/ and maps/. Emits into <site dir>:
  core.pack[.partN]        SKPK1 archive: everything except maps/*.skate
  maps/<Name>.skate[.partN] one file per map, served as-is
  pack.json                {"core": {...}, "maps": [...], "default": NAME}
Files larger than --part-mb are split into .partN chunks (GitHub Pages caps
files at 100 MB); web/loader.js concatenates them.

SKPK1: magic b"SKPK1\\0\\0\\0", u32 count, then per entry u32 path_len,
utf8 path (relative, forward slashes), u64 data_len, bytes. Little endian.
"""
import argparse
import hashlib
import json
import shutil
import struct
from pathlib import Path

# Installer bookkeeping that the game never reads.
SKIP_ROOT = {"setup.log", "setup-report.json", "maps.json"}


def core_files(root: Path):
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(root).as_posix()
        top = rel.split("/")[0]
        if "/" not in rel and (rel in SKIP_ROOT or rel.endswith(".log") or rel.endswith(".json") and rel != "release.json"):
            continue
        if top == "maps" and rel.lower().endswith(".skate"):
            continue
        if top not in ("assets", "maps", "settings", "mods") and "/" in rel:
            continue
        yield rel, path


def write_parts(data_iter, out: Path, name: str, part_bytes: int):
    """Stream chunks into out/name, splitting into name.partN if needed."""
    parts, size, index = [], 0, 0
    current = None
    written = 0

    def open_part():
        nonlocal current, written, index
        part = f"{name}.part{index}"
        index += 1
        parts.append(part)
        current = open(out / part, "wb")
        written = 0

    open_part()
    for chunk in data_iter:
        view = memoryview(chunk)
        while view:
            room = part_bytes - written
            if room == 0:
                current.close()
                open_part()
                room = part_bytes
            take = view[:room]
            current.write(take)
            written += len(take)
            size += len(take)
            view = view[len(take):]
    current.close()
    if len(parts) == 1:
        (out / parts[0]).replace(out / name)
        parts = [name]
    return {"size": size, "parts": parts}


def pack_chunks(entries):
    yield b"SKPK1\0\0\0" + struct.pack("<I", len(entries))
    for rel, path in entries:
        encoded = rel.encode("utf-8")
        yield struct.pack("<I", len(encoded)) + encoded + struct.pack("<Q", path.stat().st_size)
        with open(path, "rb") as f:
            while block := f.read(8 << 20):
                yield block


def file_chunks(path: Path):
    with open(path, "rb") as f:
        while block := f.read(8 << 20):
            yield block


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("pack")
    ap.add_argument("site")
    ap.add_argument("--default", help="default map name (else settings/default-map.json)")
    ap.add_argument("--maps", help="comma-separated subset of maps to publish")
    ap.add_argument("--part-mb", type=int, default=95)
    a = ap.parse_args()
    root, out = Path(a.pack), Path(a.site)
    if not (root / "assets").is_dir():
        raise SystemExit(f"{root} has no assets/ directory")
    out.mkdir(parents=True, exist_ok=True)
    part = a.part_mb * 1000 * 1000
    for old in list(out.glob("core.pack*")):
        old.unlink()
    if (out / "maps").is_dir():
        shutil.rmtree(out / "maps")
    (out / "maps").mkdir()

    entries = list(core_files(root))
    core = write_parts(pack_chunks(entries), out, "core.pack", part)
    core["files"] = len(entries)

    wanted = set(a.maps.split(",")) if a.maps else None
    maps = []
    for path in sorted((root / "maps").glob("*.skate"), key=lambda p: p.stem.lower()):
        if wanted is not None and path.stem not in wanted:
            continue
        info = write_parts(file_chunks(path), out / "maps", path.name, part)
        maps.append({"name": path.stem, "size": info["size"], "parts": [f"maps/{p}" for p in info["parts"]]})

    default = a.default
    if not default:
        try:
            rel = json.loads((root / "settings/default-map.json").read_text(encoding="utf-8-sig"))
            default = Path(rel).stem if rel else None
        except (OSError, ValueError):
            default = None
    names = [m["name"] for m in maps]
    if default not in names:
        default = names[0] if names else None
    # Content hashes let loader.js keep downloads in Cache Storage across visits
    # and map switches, and drop them when a new build changes the bytes.
    def digest(parts):
        h = hashlib.sha256()
        for rel in parts:
            with open(out / rel, "rb") as f:
                while chunk := f.read(1 << 20):
                    h.update(chunk)
        return h.hexdigest()[:16]
    core["hash"] = digest(core["parts"])
    for m in maps:
        m["hash"] = digest(m["parts"])
    wasm = out / "skate3rust_bg.wasm"
    engine = {"size": wasm.stat().st_size, "hash": digest(["skate3rust_bg.wasm"])} if wasm.exists() else None
    (out / "pack.json").write_text(json.dumps({"version": 1, "engine": engine, "core": core, "maps": maps, "default": default}, indent=1))
    mb = lambda n: f"{n / 1e6:.1f} MB"
    print(f"core.pack: {core['files']} files, {mb(core['size'])} in {len(core['parts'])} part(s)")
    for m in maps:
        print(f"  map {m['name']}: {mb(m['size'])} in {len(m['parts'])} part(s)")
    print(f"default map: {default}")


if __name__ == "__main__":
    main()
