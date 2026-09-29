//! Dump a .skate map's decoded embedded textures for external tools.
//!
//!     cargo run -p skate-data --example dump_textures -- <map.skate> <out.bin>
//!
//! Output: b"SKTX", u32 count, then per texture: u32 name length, name UTF-8,
//! u32 width, u32 height, u32 color space, width*height*4 RGBA bytes exactly as
//! the map stores them.
use std::io::Write;

fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [map, out] = args.as_slice() else {
        return Err("usage: dump_textures <map.skate> <out.bin>".into());
    };
    let map = skate_data::skate_map::SkateMap::load(std::path::Path::new(map))?;
    let mut w = std::io::BufWriter::new(std::fs::File::create(out).map_err(|e| e.to_string())?);
    let mut put = |b: &[u8]| w.write_all(b).map_err(|e| e.to_string());
    put(b"SKTX")?;
    put(&(map.textures.len() as u32).to_le_bytes())?;
    for t in &map.textures {
        put(&(t.name.len() as u32).to_le_bytes())?;
        put(t.name.as_bytes())?;
        for v in [t.width, t.height, t.color_space] {
            put(&v.to_le_bytes())?;
        }
        put(&t.rgba)?;
    }
    eprintln!("version {} textures {}", map.version, map.textures.len());
    Ok(())
}
