# Web build (WebGPU)

`skate3rust` also builds for `wasm32-unknown-unknown` and runs in a WebGPU
browser (Chrome/Edge 113+). The native Windows build is unchanged.

## Build

```powershell
# one-time: rustup target add wasm32-unknown-unknown; cargo install wasm-bindgen-cli --version 0.2.127
powershell -File tools/build_web.ps1 -Pack <installed pack dir> -Out <site dir> [-DefaultMap StartPark] [-Maps A,B]
python -m http.server -d <site dir> 8000   # any static server; open http://localhost:8000/
```

`build_web.ps1` runs `cargo build -p skate-game --bin skate3rust --profile web
--target wasm32-unknown-unknown --no-default-features -j2` (target dir
`$CARGO_TARGET_DIR`, default `../target-web`), then `wasm-bindgen --target web`,
copies `web/` and runs `tools/web_pack.py`. `-SkipBuild` / `-SkipPack` redo one
half. The installed pack is private converted data: keep the site directory
out of the repository and never publish it.

## Site layout

| File | Contents |
| --- | --- |
| `index.html`, `loader.js` | page, progress bar, map picker |
| `skate3rust.js`, `skate3rust_bg.wasm` | wasm-bindgen output |
| `pack.json` | core parts + sizes, maps, default map |
| `core.pack[.partN]` | `SKPK1` archive of everything except `maps/*.skate` |
| `maps/<Name>.skate[.partN]` | one map per file |

Files over 95 MB are split into `.partN` chunks (GitHub Pages limit 100 MB);
the loader concatenates them. `SKPK1`: magic `SKPK1\0\0\0`, u32 count, then per
entry u32 path length, UTF-8 relative path, u64 size, bytes (little endian).

## Runtime

- `loader.js` downloads `core.pack` and the chosen map (`?map=Name`, else the
  pack default; `?map=__test` is the test world), sets `SKATE_PACK`,
  `SKATE_MAP_NAME`, `SKATE_MAP_BYTES`, `SKATE_MAP_LIST` (optional
  `?args=["--difficulty","easy"]` becomes `SKATE_ARGS`) and starts the module.
- `crates/skate-game/src/web.rs` copies the pack into the in-memory filesystem
  (`crates/skate-vfs`) under `/pack` and registers it as Bevy's default asset
  source; no asset is fetched over HTTP after that.
- Every crate reads files through `skate_vfs` (`std::fs` on native) and time
  through `skate_vfs::time` (`web-time`; `std::time` on native).
- Choosing another map in the in-game menu or the page's picker reloads the
  page with `?map=` (no loader threads on the web).
- Web-only differences: no Lua mods, multiplayer, updater, crash reporter,
  custom-model tools or trace capture; frame pacing is left to the browser.

## Controls

Keyboard and browser gamepads both produce the raw XInput packet the Windows
build reads, so every gameplay and menu binding matches the Xbox pad. Slot 0
merges the keyboard with the first gamepad; further gamepads take slots 1-3.
Click the page once so it has keyboard focus.

| Xbox | Keyboard |
| --- | --- |
| Left stick | W A S D |
| Right stick (flicks) | Arrow keys |
| A | Space |
| B | E |
| X | Left Shift |
| Y | F |
| LT / RT | Z / C |
| LB / RB | Q / R |
| Start | Enter or Escape |
| Back | Backspace |
| D-pad up/down/left/right | 1 / 2 / 3 / 4 |
| Left / right stick click | V / B |

Keys are digital, so a key held gives full stick deflection. Browsers expose a
gamepad only after a button press while the page is focused.
