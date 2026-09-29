//! Phone mode for the web build (`?lite=1`, on by default on touch screens):
//! halves every large texture so a mobile browser tab stays inside its memory
//! limit. iOS kills a page that goes much past ~1 GB, and the full-resolution
//! build peaks near 1.9 GB (page + GPU) on desktop Edge.
use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureFormat};

/// Textures at or below this edge are left alone.
const MIN_EDGE: u32 = 256;

/// 2x2 box filter over RGBA8 rows. Odd trailing rows/columns are dropped.
pub(crate) fn halve_rgba(rgba: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let (w, h) = ((width / 2).max(1), (height / 2).max(1));
    let row = width as usize * 4;
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as usize {
        let (a, b) = (2 * y * row, (2 * y + 1).min(height as usize - 1) * row);
        for x in 0..w as usize {
            let (p, q) = (8 * x, (8 * x + 4).min(row - 4));
            for c in 0..4 {
                let sum = rgba[a + p + c] as u32 + rgba[a + q + c] as u32 + rgba[b + p + c] as u32 + rgba[b + q + c] as u32;
                out.push(((sum + 2) / 4) as u8);
            }
        }
    }
    (out, w, h)
}

/// Halve the map's embedded textures in place. Cube maps are stored as six
/// faces stacked vertically, so halving both axes keeps them six square faces.
pub(crate) fn shrink_map(map: &mut skate_data::skate_map::SkateMap) -> usize {
    let mut saved = 0;
    for t in &mut map.textures {
        if t.width <= MIN_EDGE || t.rgba.len() != t.width as usize * t.height as usize * 4 {
            continue;
        }
        let (rgba, w, h) = halve_rgba(&t.rgba, t.width, t.height);
        saved += t.rgba.len() - rgba.len();
        (t.rgba, t.width, t.height) = (rgba, w, h);
    }
    saved
}

/// Loaded images (glTF characters, props, HUD): halve large single-level RGBA8
/// ones and let Bevy drop the CPU copy after upload.
pub(crate) fn shrink_images(
    mut events: MessageReader<AssetEvent<Image>>,
    mut images: ResMut<Assets<Image>>,
    server: Res<AssetServer>,
) {
    let added: Vec<_> = events
        .read()
        .filter_map(|e| if let AssetEvent::Added { id } = e { Some(*id) } else { None })
        .collect();
    for id in added {
        // Only file-loaded images: code-built ones may read their pixels back.
        if server.get_path(id).is_none() {
            continue;
        }
        let Some(image) = images.get_mut(id) else { continue };
        let d = &image.texture_descriptor;
        let rgba8 = matches!(d.format, TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb);
        let (w, h) = (d.size.width, d.size.height);
        if !rgba8 || d.mip_level_count != 1 || d.size.depth_or_array_layers != 1 || w <= MIN_EDGE || h <= MIN_EDGE
            || !image.asset_usage.contains(RenderAssetUsages::MAIN_WORLD)
        {
            continue;
        }
        let Some(data) = image.data.as_ref().filter(|d| d.len() == w as usize * h as usize * 4) else { continue };
        let (rgba, w, h) = halve_rgba(data, w, h);
        image.data = Some(rgba);
        image.texture_descriptor.size = Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        image.asset_usage = RenderAssetUsages::RENDER_WORLD;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn halves_by_averaging_quads() {
        let px = |v: u8| [v, v, v, 255];
        let rgba: Vec<u8> = [px(0), px(4), px(8), px(12)].concat();
        let (out, w, h) = super::halve_rgba(&rgba, 2, 2);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, vec![6, 6, 6, 255]);
    }
}
