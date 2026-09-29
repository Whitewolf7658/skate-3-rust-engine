//! Print a map's materials (texture slots, colour, flags) and embedded textures.
//!
//!     cargo run -p skate-data --example map_materials -- <map.skate>
fn main() {
    let path = std::env::args().nth(1).expect("usage: map_materials <map.skate>");
    let map = skate_data::skate_map::SkateMap::load(std::path::Path::new(&path)).unwrap();
    for (i, t) in map.textures.iter().enumerate() {
        println!("tex {i:2} {:<40} {}x{} space={}", t.name, t.width, t.height, t.color_space);
    }
    let mut used = vec![0usize; map.materials.len()];
    for tri in map.geometry.indices.chunks(3) {
        let _ = tri;
    }
    let _ = &mut used;
    for (i, m) in map.materials.iter().enumerate() {
        println!("mat {i:2} {:<32} tex={:?} color={:?} flags={:#x} alpha={} rough={} retail_def={}", m.name, m.textures, m.color, m.flags, m.alpha_mode, m.roughness, m.retail_definition.is_some());
    }
}
