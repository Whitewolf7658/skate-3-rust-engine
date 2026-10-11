//! Grind edges derived from triangle collision, for custom maps only. Stock
//! maps carry their own splines and never reach this. An edge qualifies when
//! it is a convex crease on a rideable face with a real drop beside it.
use std::collections::HashMap;

type P = [f64; 3];

/// Same 1 mm weld as the collision world.
const WELD: f64 = 0.001;
/// Up component of a face the board can sit on.
const TOP_MIN_UP: f64 = 0.7;
/// Faces must turn by at least 40 degrees across the edge.
const MAX_FOLD_COS: f64 = -0.766;
/// Rise over length; steeper edges are posts, not rails.
const MAX_SLOPE: f64 = 0.7;
/// Where the ground beside the edge is sampled, and how far it must fall.
const PROBE_OUT: f64 = 0.12;
const PROBE_LIFT: f64 = 0.05;
const MIN_DROP: f64 = 0.25;
/// Clear air needed above the sample; less means the edge is buried in another solid.
const HEADROOM: f64 = 0.3;
const CHAIN_COS: f64 = 0.9;
const MIN_LENGTH: f64 = 0.6;
/// Both edges of a thin bar become one rail along its middle.
const MERGE_DISTANCE: f64 = 0.35;
const MERGE_HEIGHT: f64 = 0.05;
const EXISTING_DISTANCE: f64 = 0.3;
const MAX_RAILS: usize = 20_000;
/// The engine's cross-section test (S3 82C20728): short probes this far to each
/// side of the edge and this far above it. Surface on both sides, or anything
/// across the top, and the engine refuses the grind.
const SECTION_SIDE: f64 = 0.09;
const SECTION_HALF: f64 = 0.04;
/// Share of a line that may fail the cross-section test before it is dropped.
const MAX_BLOCKED: f64 = 0.5;
const CELL: f64 = 2.0;
const MAX_CELLS: i64 = 256;

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: P, b: P) -> P {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(a: P) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: P) -> Option<P> {
    let l = length(a);
    (l > 1e-9).then(|| [a[0] / l, a[1] / l, a[2] / l])
}
fn lerp(a: P, b: P, t: f64) -> P {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}
fn nearest_on_segment(p: P, a: P, b: P) -> P {
    let ab = sub(b, a);
    let l = dot(ab, ab);
    let t = if l > 0. {
        (dot(sub(p, a), ab) / l).clamp(0., 1.)
    } else {
        0.
    };
    lerp(a, b, t)
}
fn nearest_on_line(p: P, line: &[P]) -> P {
    let mut best = line[0];
    let mut best_d = f64::INFINITY;
    for pair in line.windows(2) {
        let q = nearest_on_segment(p, pair[0], pair[1]);
        let d = length(sub(p, q));
        if d < best_d {
            best_d = d;
            best = q;
        }
    }
    best
}
fn line_length(line: &[P]) -> f64 {
    line.windows(2).map(|p| length(sub(p[1], p[0]))).sum()
}
/// Vertices and segment midpoints: enough to say one line hugs another.
fn samples(line: &[P]) -> impl Iterator<Item = P> + '_ {
    line.iter()
        .copied()
        .chain(line.windows(2).map(|p| lerp(p[0], p[1], 0.5)))
}

/// Downward height lookups over the triangle soup.
struct Ground<'a> {
    triangles: &'a [[P; 3]],
    cells: HashMap<(i64, i64), Vec<u32>>,
    large: Vec<u32>,
}
impl<'a> Ground<'a> {
    fn new(triangles: &'a [[P; 3]]) -> Self {
        let mut cells = HashMap::<(i64, i64), Vec<u32>>::new();
        let mut large = Vec::new();
        for (i, t) in triangles.iter().enumerate() {
            let range = |axis: usize| {
                let lo = t.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min);
                let hi = t.iter().map(|p| p[axis]).fold(f64::NEG_INFINITY, f64::max);
                ((lo / CELL).floor() as i64, (hi / CELL).floor() as i64)
            };
            let (x, z) = (range(0), range(2));
            if (x.1 - x.0 + 1).saturating_mul(z.1 - z.0 + 1) > MAX_CELLS {
                large.push(i as u32);
                continue;
            }
            for cx in x.0..=x.1 {
                for cz in z.0..=z.1 {
                    cells.entry((cx, cz)).or_default().push(i as u32);
                }
            }
        }
        Self {
            triangles,
            cells,
            large,
        }
    }
    /// Highest surface at or below `p` and lowest one above it, on its vertical.
    fn around(&self, p: P) -> (Option<f64>, Option<f64>) {
        let cell = ((p[0] / CELL).floor() as i64, (p[2] / CELL).floor() as i64);
        let mut below: Option<f64> = None;
        let mut above: Option<f64> = None;
        let near = self.cells.get(&cell).map(Vec::as_slice).unwrap_or(&[]);
        for &i in near.iter().chain(&self.large) {
            let [a, b, c] = self.triangles[i as usize];
            let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
            if d.abs() < 1e-12 {
                continue;
            }
            let u = ((b[2] - c[2]) * (p[0] - c[0]) + (c[0] - b[0]) * (p[2] - c[2])) / d;
            let v = ((c[2] - a[2]) * (p[0] - c[0]) + (a[0] - c[0]) * (p[2] - c[2])) / d;
            let w = 1. - u - v;
            if u < -1e-9 || v < -1e-9 || w < -1e-9 {
                continue;
            }
            let y = u * a[1] + v * b[1] + w * c[1];
            if y <= p[1] {
                if below.is_none_or(|h| y > h) {
                    below = Some(y);
                }
            } else if above.is_none_or(|h| y < h) {
                above = Some(y);
            }
        }
        (below, above)
    }
    fn hits(&self, a: P, b: P) -> bool {
        let cell = |p: P| ((p[0] / CELL).floor() as i64, (p[2] / CELL).floor() as i64);
        let (first, second) = (cell(a), cell(b));
        let get = |c| self.cells.get(&c).map(Vec::as_slice).unwrap_or(&[]);
        let other: &[u32] = if second == first { &[] } else { get(second) };
        let delta = sub(b, a);
        get(first).iter().chain(other).chain(&self.large).any(|&i| {
            let [p, q, r] = self.triangles[i as usize];
            let (e1, e2) = (sub(q, p), sub(r, p));
            let h = cross(delta, e2);
            let det = dot(e1, h);
            if det.abs() < 1e-12 {
                return false;
            }
            let s = sub(a, p);
            let u = dot(s, h) / det;
            let k = cross(s, e1);
            let v = dot(delta, k) / det;
            let t = dot(e2, k) / det;
            u >= 0. && v >= 0. && u + v <= 1. && (0. ..=1.).contains(&t)
        })
    }
    /// Share of the line where the engine would see a blocked cross-section.
    fn blocked(&self, line: &[P]) -> f64 {
        let (mut blocked, mut total) = (0, 0);
        for pair in line.windows(2) {
            let delta = sub(pair[1], pair[0]);
            let (Some(direction), Some(up)) =
                (unit(delta), unit(cross(delta, cross([0., 1., 0.], delta))))
            else {
                continue;
            };
            let side = cross(up, direction);
            let at = |p: P, v: P, d: f64| [p[0] + v[0] * d, p[1] + v[1] * d, p[2] + v[2] * d];
            for t in [0.25, 0.5, 0.75] {
                let centre = lerp(pair[0], pair[1], t);
                let beside = |d: f64| {
                    let p = at(centre, side, d);
                    self.hits(at(p, up, SECTION_HALF), at(p, up, -SECTION_HALF))
                };
                let over = at(centre, up, SECTION_SIDE);
                total += 1;
                if (beside(SECTION_SIDE) && beside(-SECTION_SIDE))
                    || self.hits(at(over, side, SECTION_SIDE), at(over, side, -SECTION_SIDE))
                {
                    blocked += 1;
                }
            }
        }
        if total == 0 {
            1.
        } else {
            blocked as f64 / total as f64
        }
    }
}

pub(super) struct Derived {
    /// Parallel to the map's own rails: true where the engine could never
    /// grind it, because the surface under it is wider than a rail.
    pub own_blocked: Vec<bool>,
    pub rails: Vec<Vec<[f32; 3]>>,
}

/// Polylines for every derived grind edge. `own` are the map's own rails;
/// edges that already have a usable one are left out.
pub(super) fn rails(collision: &[[[f32; 3]; 3]], own: &[Vec<[f32; 3]>]) -> Derived {
    let triangles: Vec<[P; 3]> = collision
        .iter()
        .map(|t| t.map(|p| p.map(f64::from)))
        .collect();
    // Welded ids in first-seen order keep the result the same on every run.
    let mut welded = HashMap::<[i64; 3], u32>::new();
    let mut positions = Vec::<P>::new();
    let mut edge_ids = HashMap::<(u32, u32), usize>::new();
    // Per edge: the faces on it, each as (up-facing unit normal, third vertex).
    let mut edges = Vec::<((u32, u32), Vec<(P, P)>)>::new();
    for t in &triangles {
        let Some(mut normal) = unit(cross(sub(t[1], t[0]), sub(t[2], t[0]))) else {
            continue;
        };
        if normal[1] < 0. {
            normal = normal.map(|v| -v);
        }
        let ids = t.map(|p| {
            let key = p.map(|v| (v / WELD).round() as i64);
            *welded.entry(key).or_insert_with(|| {
                positions.push(p);
                positions.len() as u32 - 1
            })
        });
        for i in 0..3 {
            let (a, b) = (ids[i], ids[(i + 1) % 3]);
            if a == b {
                continue;
            }
            let key = (a.min(b), a.max(b));
            let at = *edge_ids.entry(key).or_insert_with(|| {
                edges.push((key, Vec::new()));
                edges.len() - 1
            });
            edges[at].1.push((normal, t[(i + 2) % 3]));
        }
    }
    let ground = Ground::new(&triangles);
    let mut segments = Vec::<(u32, u32)>::new();
    for ((a, b), faces) in &edges {
        if faces.len() > 2 {
            continue;
        }
        let (pa, pb) = (positions[*a as usize], positions[*b as usize]);
        let along = sub(pb, pa);
        let Some(axis) = unit(along) else { continue };
        if along[1].abs() > MAX_SLOPE * length(along) {
            continue;
        }
        let top = if faces.len() == 2 && faces[1].0[1] > faces[0].0[1] {
            1
        } else {
            0
        };
        let (normal, third) = faces[top];
        if normal[1] < TOP_MIN_UP {
            continue;
        }
        // Direction across the edge, back into the top face.
        let across = |p: P| {
            let v = sub(p, pa);
            let along = dot(v, axis);
            unit(sub(v, [axis[0] * along, axis[1] * along, axis[2] * along]))
        };
        let Some(inward) = across(third) else { continue };
        if let Some(&(_, other)) = faces.get(1 - top).filter(|_| faces.len() == 2) {
            let Some(fold) = across(other) else { continue };
            // The other face has to fall away from the top one, and sharply.
            if dot(normal, sub(other, pa)) > -WELD || dot(inward, fold) < MAX_FOLD_COS {
                continue;
            }
        }
        let Some(outward) = unit([-inward[0], 0., -inward[2]]) else {
            continue;
        };
        let mid = lerp(pa, pb, 0.5);
        let probe = [
            mid[0] + outward[0] * PROBE_OUT,
            mid[1] + PROBE_LIFT,
            mid[2] + outward[2] * PROBE_OUT,
        ];
        let (below, above) = ground.around(probe);
        // Nothing below means the edge of the map, not a ledge.
        if below.is_none_or(|y| mid[1] - y < MIN_DROP)
            || above.is_some_and(|y| y - probe[1] < HEADROOM)
        {
            continue;
        }
        segments.push((*a, *b));
    }

    // Join segments that continue each other into polylines.
    let mut touching = HashMap::<u32, Vec<usize>>::new();
    for (i, (a, b)) in segments.iter().enumerate() {
        touching.entry(*a).or_default().push(i);
        touching.entry(*b).or_default().push(i);
    }
    let mut used = vec![false; segments.len()];
    let mut lines = Vec::<Vec<P>>::new();
    for start in 0..segments.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut chain = vec![segments[start].0, segments[start].1];
        for _ in 0..2 {
            loop {
                let tip = chain[chain.len() - 1];
                if tip == chain[0] {
                    break;
                }
                let before = positions[chain[chain.len() - 2] as usize];
                let Some(heading) = unit(sub(positions[tip as usize], before)) else {
                    break;
                };
                let mut next = None;
                let mut count = 0;
                for &j in &touching[&tip] {
                    if used[j] {
                        continue;
                    }
                    let far = if segments[j].0 == tip {
                        segments[j].1
                    } else {
                        segments[j].0
                    };
                    let step = sub(positions[far as usize], positions[tip as usize]);
                    if unit(step).is_some_and(|s| dot(heading, s) >= CHAIN_COS) {
                        count += 1;
                        next = Some((j, far));
                    }
                }
                let Some((j, far)) = next.filter(|_| count == 1) else {
                    break;
                };
                used[j] = true;
                chain.push(far);
            }
            chain.reverse();
        }
        let mut line = Vec::<P>::new();
        for p in chain.iter().map(|&i| positions[i as usize]) {
            // Straight runs need one segment, not one per triangle.
            if line.len() >= 2 {
                let n = line.len();
                let (a, b) = (unit(sub(line[n - 1], line[n - 2])), unit(sub(p, line[n - 1])));
                if a.zip(b).is_some_and(|(a, b)| dot(a, b) > 0.9999) {
                    line[n - 1] = p;
                    continue;
                }
            }
            line.push(p);
        }
        if line_length(&line) >= MIN_LENGTH {
            lines.push(line);
        }
    }

    let own: Vec<Vec<P>> = own
        .iter()
        .map(|line| line.iter().map(|p| p.map(f64::from)).collect())
        .collect();
    let own_blocked: Vec<bool> = own
        .iter()
        .map(|line| ground.blocked(line) > MAX_BLOCKED)
        .collect();
    let existing: Vec<[P; 2]> = own
        .iter()
        .zip(&own_blocked)
        .filter(|(_, blocked)| !**blocked)
        .flat_map(|(line, _)| line.windows(2).map(|p| [p[0], p[1]]))
        .collect();
    lines.retain(|line| {
        !samples(line).all(|p| {
            existing
                .iter()
                .any(|s| length(sub(p, nearest_on_segment(p, s[0], s[1]))) <= EXISTING_DISTANCE)
        })
    });
    // Longest first, so a duplicate is always the shorter or equal line.
    lines.sort_by(|a, b| line_length(b).total_cmp(&line_length(a)));
    let mut kept = Vec::<Vec<P>>::new();
    for line in lines {
        let twin = kept.iter().position(|k| {
            samples(&line).all(|p| {
                let q = nearest_on_line(p, k);
                (p[1] - q[1]).abs() <= MERGE_HEIGHT && length(sub(p, q)) <= MERGE_DISTANCE
            })
        });
        match twin {
            Some(k) if line_length(&line) >= 0.8 * line_length(&kept[k]) => {
                let centre: Vec<P> = kept[k]
                    .iter()
                    .map(|p| lerp(*p, nearest_on_line(*p, &line), 0.5))
                    .collect();
                // A bar too wide to grind down its middle keeps both edges.
                if ground.blocked(&centre) > MAX_BLOCKED {
                    kept.push(line);
                } else {
                    kept[k] = centre;
                }
            }
            Some(_) => {}
            None => kept.push(line),
        }
    }
    kept.retain(|line| ground.blocked(line) <= MAX_BLOCKED);
    kept.truncate(MAX_RAILS);
    Derived {
        own_blocked,
        rails: kept
            .into_iter()
            .map(|line| line.into_iter().map(|p| p.map(|v| v as f32)).collect())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type T = [[f32; 3]; 3];
    fn quad(out: &mut Vec<T>, q: [[f32; 3]; 4]) {
        out.push([q[0], q[1], q[2]]);
        out.push([q[0], q[2], q[3]]);
    }
    fn floor(out: &mut Vec<T>, half: f32) {
        quad(
            out,
            [
                [-half, 0., -half],
                [-half, 0., half],
                [half, 0., half],
                [half, 0., -half],
            ],
        );
    }
    /// Closed box, faces wound outward.
    fn solid(out: &mut Vec<T>, min: [f32; 3], max: [f32; 3]) {
        let [x0, y0, z0] = min;
        let [x1, y1, z1] = max;
        quad(out, [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]]);
        quad(out, [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]]);
        quad(out, [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]]);
        quad(out, [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]]);
        quad(out, [[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]]);
        quad(out, [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]]);
    }
    fn found(t: &[T]) -> Vec<Vec<[f32; 3]>> {
        rails(t, &[]).rails
    }
    fn span(line: &[[f32; 3]]) -> f32 {
        line.windows(2)
            .map(|p| (0..3).map(|i| (p[1][i] - p[0][i]).powi(2)).sum::<f32>().sqrt())
            .sum()
    }

    #[test]
    fn flat_ground_and_gentle_banks_have_no_edges() {
        let mut t = Vec::new();
        floor(&mut t, 10.);
        // 30 degree bank up to a deck.
        quad(&mut t, [[0., 0., -2.], [0., 0., 2.], [1.732, 1., 2.], [1.732, 1., -2.]]);
        quad(&mut t, [[1.732, 1., -2.], [1.732, 1., 2.], [5., 1., 2.], [5., 1., -2.]]);
        // Open sides and the deck's back drop to the floor; the creases across the bank do not count.
        let found = found(&t);
        assert!(!found.is_empty());
        assert!(found.iter().all(|l| l.iter().all(|p| p[0] > 4.99 || p[2].abs() > 1.99)));
    }

    #[test]
    fn ledge_box_gives_its_four_top_edges() {
        let mut t = Vec::new();
        floor(&mut t, 10.);
        solid(&mut t, [-2., 0., -0.5], [2., 0.5, 0.5]);
        let found = found(&t);
        assert_eq!(found.len(), 4);
        assert!(found.iter().all(|l| l.len() == 2 && l.iter().all(|p| p[1] == 0.5)));
        assert_eq!(found.iter().filter(|l| (span(l) - 4.).abs() < 1e-4).count(), 2);
        assert_eq!(found.iter().filter(|l| (span(l) - 1.).abs() < 1e-4).count(), 2);
    }

    #[test]
    fn stair_noses_are_not_rails_but_the_drop_behind_is() {
        let mut t = Vec::new();
        floor(&mut t, 10.);
        for step in 0..3 {
            let s = step as f32;
            solid(&mut t, [-1., 0., s * 0.3], [1., 0.15 * (s + 1.), s * 0.3 + 0.3]);
        }
        let found = found(&t);
        assert_eq!(found.len(), 1);
        assert!(found[0].iter().all(|p| (p[1] - 0.45).abs() < 1e-5 && (p[2] - 0.9).abs() < 1e-5));
    }

    #[test]
    fn thin_bar_becomes_one_centred_rail() {
        let mut t = Vec::new();
        floor(&mut t, 10.);
        solid(&mut t, [-2., 0.35, -0.05], [2., 0.45, 0.05]);
        let found = found(&t);
        assert_eq!(found.len(), 1);
        assert!((span(&found[0]) - 4.).abs() < 1e-4);
        assert!(found[0].iter().all(|p| p[2].abs() < 1e-5 && (p[1] - 0.45).abs() < 1e-5));
    }

    #[test]
    fn edges_that_already_have_a_rail_are_left_out() {
        let mut t = Vec::new();
        floor(&mut t, 10.);
        solid(&mut t, [-2., 0.35, -0.05], [2., 0.45, 0.05]);
        let derived = rails(&t, &[vec![[-2., 0.45, 0.], [2., 0.45, 0.]]]);
        assert_eq!(derived.own_blocked, [false]);
        assert!(derived.rails.is_empty());
    }

    #[test]
    fn wide_bar_keeps_both_edges_and_rejects_a_centre_rail() {
        let mut t = Vec::new();
        floor(&mut t, 10.);
        solid(&mut t, [-2., 0.35, -0.15], [2., 0.45, 0.15]);
        let derived = rails(&t, &[vec![[-2., 0.45, 0.], [2., 0.45, 0.]]]);
        assert_eq!(derived.own_blocked, [true]);
        let long: Vec<_> = derived.rails.iter().filter(|l| span(l) > 3.9).collect();
        assert_eq!(long.len(), 2);
        assert!(long.iter().all(|l| l.iter().all(|p| (p[2].abs() - 0.15).abs() < 1e-5)));
    }

    #[test]
    fn winding_does_not_matter() {
        let mut t = Vec::new();
        floor(&mut t, 10.);
        solid(&mut t, [-2., 0., -0.5], [2., 0.5, 0.5]);
        for tri in &mut t {
            tri.swap(1, 2);
        }
        assert_eq!(found(&t).len(), 4);
    }

    #[test]
    #[ignore = "requires a supplied triangle dump (little-endian f32, 9 per triangle)"]
    fn supplied_triangle_dump() {
        let bytes = std::fs::read(std::env::var("SKATE_TEST_TRIANGLES").unwrap()).unwrap();
        let floats: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let t: Vec<T> = floats
            .chunks_exact(9)
            .map(|c| std::array::from_fn(|i| std::array::from_fn(|j| c[i * 3 + j])))
            .collect();
        let started = std::time::Instant::now();
        let found = found(&t);
        println!("triangles={} rails={} ms={}", t.len(), found.len(), started.elapsed().as_millis());
        for line in &found {
            println!("len={:.2} points={} {:?} -> {:?}", span(line), line.len(), line[0], line[line.len() - 1]);
        }
    }
}
