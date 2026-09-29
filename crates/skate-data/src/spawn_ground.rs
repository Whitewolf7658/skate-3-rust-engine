//! Some retail spawns (and teleport-less custom maps) sit over no collision, so
//! the skater falls out of the world. This finds ground under a start point, or
//! the nearest walkable collision to move it onto.
use crate::skate_map::SkateMap;

/// Every collision triangle of `map`: the embedded RWCM archive for retail
/// maps, otherwise the map's own collision list.
fn triangles(map: &SkateMap) -> Result<Vec<[[f32; 3]; 3]>, String> {
    let mut out: Vec<[[f32; 3]; 3]> = map.geometry.collision.iter().map(|c| c.points).collect();
    if let Some(archive) = map.extensions.iter().find(|e| e.tag == *b"RWCM") {
        crate::retail_collision::visit_clusters(&archive.payload, |_, cluster| {
            out.extend(cluster.iter().map(|t| t.points));
            Ok(())
        })?;
    }
    Ok(out)
}

/// Height of `t` at (x, z) if the vertical line through it crosses the triangle.
fn height_at(t: &[[f32; 3]; 3], x: f32, z: f32) -> Option<f32> {
    let [a, b, c] = *t;
    let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
    if d.abs() < 1e-9 {
        return None;
    }
    let u = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / d;
    let v = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / d;
    let w = 1.0 - u - v;
    (u >= -1e-4 && v >= -1e-4 && w >= -1e-4).then(|| u * a[1] + v * b[1] + w * c[1])
}

/// Upward-facing enough to stand on (normal within ~45 degrees of +Y).
fn walkable(t: &[[f32; 3]; 3]) -> bool {
    let [a, b, c] = *t;
    let (e1, e2) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
    let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    len > 1e-9 && (n[1] / len).abs() > 0.7
}

/// `None` when there is walkable ground within 200 m below `point`; otherwise
/// the point 1 m above the centre of the nearest walkable triangle.
pub fn grounded_spawn(map: &SkateMap, point: [f32; 3]) -> Result<Option<[f32; 3]>, String> {
    let tris = triangles(map)?;
    let below = tris.iter().filter(|t| walkable(t)).filter_map(|t| height_at(t, point[0], point[2]))
        .any(|y| y <= point[1] + 0.5 && y >= point[1] - 200.0);
    if below || tris.is_empty() {
        return Ok(None);
    }
    let centre = |t: &[[f32; 3]; 3]| [0, 1, 2].map(|i| (t[0][i] + t[1][i] + t[2][i]) / 3.0);
    let best = tris.iter().filter(|t| walkable(t)).map(centre).min_by(|a, b| {
        let d = |c: &[f32; 3]| (c[0] - point[0]).powi(2) + (c[2] - point[2]).powi(2) + 0.25 * (c[1] - point[1]).powi(2);
        d(a).total_cmp(&d(b))
    });
    Ok(best.map(|c| [c[0], c[1] + 1.0, c[2]]))
}
