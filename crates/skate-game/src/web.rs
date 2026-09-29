//! Browser entry. `web/loader.js` downloads the asset pack and one map, then
//! hands them over as globals before the module starts:
//! - `SKATE_PACK`: Uint8Array holding an `SKPK1` pack (paths relative to the install root)
//! - `SKATE_MAP_BYTES` + `SKATE_MAP_NAME`: the chosen `maps/<name>.skate`
//! - `SKATE_ARGS`: optional command-line arguments (strings)
//! Everything lands in the in-memory filesystem below [`ROOT`].
use skate_vfs::PathExt as _;
use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSourceBuilder, PathStream, Reader, VecReader,
};
use bevy::prelude::*;
use js_sys::{Array, Reflect, Uint8Array};
use std::{ffi::OsString, path::{Path, PathBuf}};
use wasm_bindgen::{JsCast, JsValue};

pub(crate) const ROOT: &str = "/pack";

fn global(name: &str) -> JsValue {
    Reflect::get(&js_sys::global(), &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
}
fn release(name: &str) {
    let _ = Reflect::set(&js_sys::global(), &JsValue::from_str(name), &JsValue::NULL);
}

/// Copy the pack straight from JS memory, one entry at a time, so wasm memory
/// never holds the whole pack twice.
fn install_pack(pack: &Uint8Array) -> Result<usize, String> {
    let len = pack.length() as usize;
    let bytes = |at: usize, n: usize| -> Result<Vec<u8>, String> {
        if at.checked_add(n).is_none_or(|end| end > len) { return Err("Truncated pack".into()); }
        Ok(pack.subarray(at as u32, (at + n) as u32).to_vec())
    };
    if bytes(0, 8)? != b"SKPK1\0\0\0" { return Err("SKATE_PACK is not an SKPK1 pack".into()); }
    let count = u32::from_le_bytes(bytes(8, 4)?.try_into().unwrap()) as usize;
    let mut at = 12;
    let root = Path::new(ROOT);
    for _ in 0..count {
        let n = u32::from_le_bytes(bytes(at, 4)?.try_into().unwrap()) as usize;
        let name = String::from_utf8(bytes(at + 4, n)?).map_err(|_| "Pack path is not UTF-8")?;
        at += 4 + n;
        let size = u64::from_le_bytes(bytes(at, 8)?.try_into().unwrap()) as usize;
        at += 8;
        if name.starts_with('/') || name.split('/').any(|c| c == "..") { return Err(format!("Unsafe pack path {name}")); }
        skate_vfs::insert(&root.join(&name), bytes(at, size)?);
        at += size;
    }
    Ok(count)
}

/// Install pack + map into the VFS and return the game's arguments.
pub(crate) fn install() -> Result<Vec<OsString>, String> {
    console_error_panic_hook::set_once();
    let pack = global("SKATE_PACK").dyn_into::<Uint8Array>().map_err(|_| "globalThis.SKATE_PACK must be a Uint8Array (load the page through web/index.html)")?;
    let count = install_pack(&pack)?;
    release("SKATE_PACK");
    log(&format!("skate3rust: installed {count} pack files under {ROOT}"));
    let map_name = global("SKATE_MAP_NAME").as_string();
    if let (Some(name), Ok(map)) = (&map_name, global("SKATE_MAP_BYTES").dyn_into::<Uint8Array>()) {
        if name.contains(['/', '\\']) || name.contains("..") { return Err("Invalid SKATE_MAP_NAME".into()); }
        skate_vfs::insert(&Path::new(ROOT).join(format!("maps/{name}.skate")), map.to_vec());
        release("SKATE_MAP_BYTES");
    }
    // Other maps appear in the in-game map menu as empty placeholders; choosing
    // one reloads the page (map_transition::start), so they are never parsed.
    let list = global("SKATE_MAP_LIST");
    if Array::is_array(&list) {
        for name in Array::from(&list).iter().filter_map(|v| v.as_string()) {
            let path = Path::new(ROOT).join(format!("maps/{name}.skate"));
            if !name.contains(['/', '\\']) &&skate_vfs::metadata(&path).is_err() {
                skate_vfs::insert(&path, Vec::new());
            }
        }
    }
    let args = global("SKATE_ARGS");
    if Array::is_array(&args) {
        return Ok(Array::from(&args).iter().filter_map(|v| v.as_string()).map(OsString::from).collect());
    }
    let mut args = vec![OsString::from("--assets"), format!("{ROOT}/assets").into()];
    match map_name {
        Some(name) => {
            args.extend([OsString::from("--map"), format!("{ROOT}/maps/{name}.skate").into()]);
            if let Some(id) = start_teleport(&name) {
                log(&format!("skate3rust: starting {name} at teleport {id:?} (?teleport=none uses the authored spawn)"));
                args.extend([OsString::from("--teleport"), id.into()]);
            }
        }
        None => args.push("--test-world".into()),
    }
    Ok(args)
}

/// Retail spawns can sit above no collision (see docs/web.md), so the web
/// build starts at the map's teleport destination: `SKATE_TELEPORT` (from
/// `?teleport=`) if set, else one ending in "progression", else the first.
fn start_teleport(map: &str) -> Option<String> {
    let wanted = global("SKATE_TELEPORT").as_string();
    if wanted.as_deref() == Some("none") { return None; }
    let destinations = crate::teleport_menu::load(&Path::new(ROOT).join("assets")).ok()?;
    let usable: Vec<_> = destinations.iter().filter(|d| d.map == map && d.matrix.is_some()).collect();
    if let Some(id) = wanted { return usable.iter().any(|d| d.id == id).then_some(id); }
    usable.iter().find(|d| d.id.ends_with("progression")).or(usable.first()).map(|d| d.id.clone())
}

pub(crate) fn log(message: &str) { web_sys::console::log_1(&message.into()); }

/// Call a page-provided `globalThis.<name>(arg)` if it exists.
pub(crate) fn call(name: &str, arg: &str) {
    if let Ok(f) = global(name).dyn_into::<js_sys::Function>() {
        let _ = f.call1(&JsValue::NULL, &arg.into());
    }
}

/// Phone mode, set by the page loader (`SKATE_LITE`): smaller textures, no MSAA, half-scale render.
pub(crate) fn lite() -> bool {
    global("SKATE_LITE").as_bool().unwrap_or(false)
}

/// `?debugmem=1`: log wasm linear memory every 5 s (the rest of the tab is
/// JS, compiled code and GPU memory, which the browser reports separately).
pub(crate) fn memory_log(time: bevy::prelude::Res<bevy::prelude::Time<bevy::prelude::Real>>, mut next: bevy::prelude::Local<f32>) {
    if !global("SKATE_DEBUGMEM").as_bool().unwrap_or(false) || time.elapsed_secs() < *next {
        return;
    }
    *next = time.elapsed_secs() + 5.0;
    let bytes = core::arch::wasm32::memory_size(0) * 65536;
    log(&format!("SKATE_MEM t={:.0}s wasm={} MB", time.elapsed_secs(), bytes >> 20));
}

/// Console error plus a visible message in the page (`skateFatal`).
pub(crate) fn report(message: &str) {
    web_sys::console::error_1(&message.into());
    call("skateFatal", message);
}

/// Default asset source served from the in-memory filesystem (no HTTP).
struct VfsAssetReader(PathBuf);
impl AssetReader for VfsAssetReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let full = self.0.join(path);
        skate_vfs::read(&full).map(VecReader::new).map_err(|_| AssetReaderError::NotFound(full))
    }
    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let mut meta = self.0.join(path).into_os_string();
        meta.push(".meta");
        let meta = PathBuf::from(meta);
        skate_vfs::read(&meta).map(VecReader::new).map_err(|_| AssetReaderError::NotFound(meta))
    }
    async fn read_directory<'a>(&'a self, path: &'a Path) -> Result<Box<PathStream>, AssetReaderError> {
        let full = self.0.join(path);
        let entries: Vec<PathBuf> = skate_vfs::read_dir(&full)
            .map_err(|_| AssetReaderError::NotFound(full.clone()))?
            .flatten()
            .map(|e| path.join(e.file_name()))
            .collect();
        Ok(Box::new(bevy::tasks::futures_lite::stream::iter(entries)))
    }
    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        Ok(skate_vfs::metadata(self.0.join(path)).is_ok_and(|m| m.is_dir()))
    }
}

/// Must run before `DefaultPlugins` so it replaces the HTTP default source.
pub(crate) fn register_asset_source(app: &mut App, root: PathBuf) {
    app.register_asset_source(
        bevy::asset::io::AssetSourceId::Default,
        AssetSourceBuilder::new(move || Box::new(VfsAssetReader(root.clone()))),
    );
    // Vehicle and other mod models load as `mods://<package>/...`.
    let mods = crate::modding::package_root();
    app.register_asset_source(
        "mods",
        AssetSourceBuilder::new(move || Box::new(VfsAssetReader(mods.clone()))),
    );
}
