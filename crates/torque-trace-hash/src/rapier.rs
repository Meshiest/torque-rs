//! The Rapier world's scenarios: 64 boxes and 8 hulls ([`drop`]), or 24 compounds of hulls ([`compound`]), dropped onto the example
//! hills and the keep interior standing on them, stepped [`TICKS`] ticks, every body's state hashed each tick.
//!
//! No server or client runs: the world is the deterministic core that a server's props and a client's prediction both step,
//! so its state alone must match on every target. The map is the example kit's own: the hills' `.ter` read as the engine
//! reads a terrain block, and the keep's `.dif` as it reads an interior, placed at the field's height.

use std::rc::Rc;
use std::sync::Arc;

use torque_collide::ObjectId;
use torque_collide::map::{HeightField, InteriorGeom, InteriorShape, MapShape, TerrainShape};
use torque_physics_rapier::{BodyDesc, BodyId, Config, Shape, World};

use crate::hash::TraceHasher;
use crate::scenarios::{Outcome, TICKS};

/// The squares of the hills (TGE's default `squareSize`).
const SQUARE: i32 = 8;

fn rng(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// A float in `lo..hi` from 24 bits of the generator.
fn range(state: &mut u64, lo: f32, hi: f32) -> f32 {
    let t = (rng(state) >> 40) as f32 / (1u64 << 24) as f32;
    lo + (hi - lo) * t
}

/// The hills and the keep, as the engine's collision holds them.
fn map() -> Result<(Rc<MapShape>, Rc<MapShape>), String> {
    let ter = torque_maps::ter::parse(&torque_example_assets::terrain::hills())
        .map_err(|e| format!("the hills: {e}"))?;
    let field = Arc::new(HeightField::new(&torque_maps::legacy::heightfield(&ter)));
    let place = torque_maps::legacy::terrain_placement(SQUARE, false, &[]);
    let hills = MapShape::Terrain(TerrainShape::new(field, &place));
    let dif = torque_example_assets::interiors::keep_dif().map_err(|e| format!("the keep: {e}"))?;
    let dif = torque_maps::dif::parse(&dif).map_err(|e| format!("the keep: {e:?}"))?;
    let detail = dif.details.first().ok_or("the keep has no detail")?;
    let geom = Arc::new(InteriorGeom::new(&torque_maps::legacy::convexes(detail)));
    let mut at = torque_collide::math::IDENTITY;
    at[11] = torque_example_assets::terrain::BASE;
    let keep = InteriorShape::new(geom, at, [1.0; 3]).ok_or("the keep does not place")?;
    Ok((Rc::new(hills), Rc::new(MapShape::Interior(keep))))
}

/// The bodies: an 8 x 8 grid of boxes over the keep, the field and the rolling ground north of it, and 8 hulls among them.
fn bodies() -> Vec<(BodyId, BodyDesc)> {
    let mut s = 0x2545_f491_4f6c_dd1d_u64;
    let mut out = Vec::new();
    let base = torque_example_assets::terrain::BASE;
    for k in 0..72u32 {
        let (gx, gy) = ((k % 8) as f32, ((k / 8) % 8) as f32);
        let position = if k < 64 {
            [
                -21.0 + gx * 6.0 + range(&mut s, -1.0, 1.0),
                -24.0 + gy * 12.0 + range(&mut s, -1.0, 1.0),
                base + 12.0 + range(&mut s, 0.0, 10.0),
            ]
        } else {
            [
                range(&mut s, -10.0, 10.0),
                range(&mut s, -10.0, 10.0),
                base + 26.0,
            ]
        };
        let shape = if k < 64 {
            Shape::Box {
                half: [
                    range(&mut s, 0.25, 0.9),
                    range(&mut s, 0.25, 0.9),
                    range(&mut s, 0.25, 0.9),
                ],
            }
        } else {
            Shape::Hull {
                points: (0..12)
                    .map(|_| {
                        [
                            range(&mut s, -0.8, 0.8),
                            range(&mut s, -0.8, 0.8),
                            range(&mut s, -0.8, 0.8),
                        ]
                    })
                    .collect(),
            }
        };
        let rotation = [
            range(&mut s, -1.0, 1.0),
            range(&mut s, -1.0, 1.0),
            range(&mut s, -1.0, 1.0),
            range(&mut s, 0.2, 1.0),
        ];
        let desc = BodyDesc {
            shape,
            position,
            rotation,
            linvel: [range(&mut s, -2.0, 2.0), range(&mut s, -2.0, 2.0), 0.0],
            angvel: [range(&mut s, -1.0, 1.0); 3],
            density: range(&mut s, 0.5, 3.0),
            friction: range(&mut s, 0.3, 1.0),
            restitution: range(&mut s, 0.0, 0.3),
            ..BodyDesc::default()
        };
        out.push((BodyId(100 + k), desc));
    }
    out
}

/// The eight corners of the box `lo..hi`.
fn corners(lo: [f32; 3], hi: [f32; 3]) -> Vec<[f32; 3]> {
    (0..8)
        .map(|i| std::array::from_fn(|k| if i & (1 << k) == 0 { lo[k] } else { hi[k] }))
        .collect()
}

/// A random box of up to `half` about a centre within `around` of the origin.
fn random_box(s: &mut u64, around: f32, half: f32) -> Vec<[f32; 3]> {
    let c = [
        range(s, -around, around),
        range(s, -around, around),
        range(s, -around, around),
    ];
    let h = [
        range(s, 0.1, half),
        range(s, 0.1, half),
        range(s, 0.1, half),
    ];
    corners(
        std::array::from_fn(|k| c[k] - h[k]),
        std::array::from_fn(|k| c[k] + h[k]),
    )
}

/// Points on a plane tilted about both horizontal axes (no axis-aligned box of them is thin).
fn tilted_plane(s: &mut u64, n: usize) -> Vec<[f32; 3]> {
    (0..n)
        .map(|_| {
            let (x, y) = (range(s, -0.8, 0.8), range(s, -0.8, 0.8));
            [x, y, 0.7 * x - 0.4 * y]
        })
        .collect()
}

/// The bodies of [`compound`]: 24 of them over the field, the keep and the ground north of it. Sixteen are compounds of two to
/// four boxes (an L, a table, a chair: overlapping, off-centre, so the summed mass, the centre of mass and the inertia of the
/// compound are all unlike a box's), four a compound with a hull of points in a tilted plane among sound ones, and four a single
/// hull of points in a tilted plane (two) or on a line (two): the points that cannot make a hull are made as their boxes.
fn compound_bodies() -> Vec<(BodyId, BodyDesc)> {
    let mut s = 0x9e37_79b9_7f4a_7c15_u64;
    let base = torque_example_assets::terrain::BASE;
    let mut out = Vec::new();
    for k in 0..24u32 {
        let (gx, gy) = ((k % 6) as f32, (k / 6) as f32);
        let position = [
            -15.0 + gx * 6.0 + range(&mut s, -1.0, 1.0),
            -18.0 + gy * 12.0 + range(&mut s, -1.0, 1.0),
            base + 14.0 + range(&mut s, 0.0, 8.0),
        ];
        let shape = match k {
            0..16 => Shape::Compound {
                hulls: (0..2 + k % 3)
                    .map(|_| random_box(&mut s, 0.6, 0.5))
                    .collect::<Vec<_>>()
                    .into(),
            },
            16..20 => Shape::Compound {
                hulls: vec![
                    random_box(&mut s, 0.5, 0.4),
                    tilted_plane(&mut s, 30),
                    random_box(&mut s, 0.5, 0.4),
                ]
                .into(),
            },
            20..22 => Shape::Hull {
                points: tilted_plane(&mut s, 24),
            },
            _ => Shape::Hull {
                points: (0..12)
                    .map(|_| {
                        let t = range(&mut s, -0.8, 0.8);
                        [0.5 * t, -0.8 * t, 0.3 * t]
                    })
                    .collect(),
            },
        };
        let rotation = [
            range(&mut s, -1.0, 1.0),
            range(&mut s, -1.0, 1.0),
            range(&mut s, -1.0, 1.0),
            range(&mut s, 0.2, 1.0),
        ];
        out.push((
            BodyId(100 + k),
            BodyDesc {
                shape,
                position,
                rotation,
                linvel: [range(&mut s, -2.0, 2.0), range(&mut s, -2.0, 2.0), 0.0],
                angvel: [range(&mut s, -1.0, 1.0); 3],
                density: range(&mut s, 0.5, 3.0),
                friction: range(&mut s, 0.3, 1.0),
                restitution: range(&mut s, 0.0, 0.3),
                ..BodyDesc::default()
            },
        ));
    }
    out
}

/// The first scenario's run (module docs). `distance` is the bodies' mean distance from where they started.
pub fn drop() -> Result<Outcome, String> {
    run(bodies())
}

/// The second scenario's run: `compound_bodies` dropped the same way.
pub fn compound() -> Result<Outcome, String> {
    run(compound_bodies())
}

/// Drops `bodies` onto the hills and the keep and hashes each tick.
fn run(bodies: Vec<(BodyId, BodyDesc)>) -> Result<Outcome, String> {
    let (hills, keep) = map()?;
    let mut world = World::new(Config::default());
    let mut free = |_: u64| true;
    world
        .add_map(ObjectId(1), hills)
        .map_err(|e| e.to_string())?;
    world
        .add_map(ObjectId(2), keep)
        .map_err(|e| e.to_string())?;
    let start: Vec<[f32; 3]> = bodies.iter().map(|(_, d)| d.position).collect();
    world
        .add_bodies(bodies, &mut free)
        .map_err(|e| e.to_string())?;
    let mut h = TraceHasher::new();
    let mut restored = 0u64;
    for _ in 0..TICKS {
        let report = world.step(&mut free).map_err(|e| e.to_string())?;
        restored += report.restored.len() as u64;
        h.u32(u32::from(report.substeps));
        h.u32(report.contacts as u32);
        for w in world.state_words() {
            h.u32(w);
        }
    }
    let ends: Vec<[f32; 3]> = world
        .ids()
        .filter_map(|id| world.body(id))
        .map(|s| s.position)
        .collect();
    let n = ends.len().max(1) as f32;
    let distance = start
        .iter()
        .zip(&ends)
        .map(|(a, b)| (0..3).map(|k| (b[k] - a[k]).powi(2)).sum::<f32>().sqrt())
        .sum::<f32>()
        / n;
    let end = [0, 1, 2].map(|k| ends.iter().map(|p| p[k]).sum::<f32>() / n);
    let stats = world.map_stats();
    h.u64(stats.cells as u64);
    h.u64(stats.triangles as u64);
    h.u64(stats.hulls as u64);
    Ok(Outcome {
        hash: h.finish(),
        values: h.count(),
        distance,
        end,
        corrections: restored,
        driven_ticks: 0,
    })
}
