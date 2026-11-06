//! A map's static shapes: its `TSStatic`s and its shape replicas, each shape at rest at its most detailed level
//! (the caller's [`super::RestShapes`]), every instance of a shape's draw merged into one mesh (a forest of 900 trees draws as a
//! handful of draws). A `TSStatic` is placed by its matrix and scale; drawn only on a map.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::{Fnv, Mesh, TexRef};
use glam::Vec3;

use crate::math::{Mat, unit_or_same};
use crate::textures::TextureSet;
use crate::{Assets, Draw, Vertex};

/// A merged draw: the shape's draw (its state), its texture, the vertices and their position sum.
type Group = (Draw, TexRef, Vec<Vertex>, [f64; 3]);

/// The most instances merged (statics and replicas together).
pub const MAX_INSTANCES: usize = 100_000;

/// The statics and replicas merged a draw per shape, at most `cap` vertices in all.
#[allow(clippy::too_many_arguments)]
pub(crate) fn meshes(
    rest: &dyn super::RestShapes,
    assets: &dyn Assets,
    statics: &[(String, [f32; 16], [f32; 3])],
    replicas: &[(String, Mat, [f32; 3])],
    textures: &mut TextureSet<'_>,
    key: u64,
    notes: &mut Vec<String>,
    cap: usize,
) -> Vec<Mesh> {
    let mut instances: Vec<(&str, Mat, [f32; 3])> = statics
        .iter()
        .map(|(p, m, s)| (p.as_str(), Mat { m: *m }, *s))
        .collect();
    instances.extend(replicas.iter().map(|(p, m, s)| (p.as_str(), *m, *s)));
    if instances.len() > MAX_INSTANCES {
        notes.push(format!(
            "map shapes: {} instances, the first {MAX_INSTANCES} merged",
            instances.len()
        ));
        instances.truncate(MAX_INSTANCES);
    }
    let mut shapes: BTreeMap<String, Option<Vec<Draw>>> = BTreeMap::new();
    // By (shape, draw): the draw's state, its texture and the merged vertices with their position sum.
    let mut groups: BTreeMap<(String, usize), Group> = BTreeMap::new();
    let mut merged = 0usize;
    'all: for (path, m, s) in instances {
        let folded = path.to_ascii_lowercase();
        let draws = shapes.entry(folded.clone()).or_insert_with(|| {
            let made = rest.rest_draws(assets, path, textures, notes);
            if made.is_none() {
                notes.push(format!("map shape {path}: not loaded"));
            }
            made
        });
        let Some(draws) = draws.as_ref() else {
            continue;
        };
        if !s.iter().all(|c| c.is_finite() && *c != 0.0) {
            continue;
        }
        let mirrored = s[0] * s[1] * s[2] < 0.0;
        for (k, d) in draws.iter().enumerate() {
            let entry = groups.entry((folded.clone(), k)).or_insert_with(|| {
                let texture = match d.texture.and_then(|(t, _)| textures.path(t)) {
                    Some(p) => TexRef::Name(p.to_string()),
                    None => TexRef::None,
                };
                (d.clone(), texture, Vec::new(), [0.0; 3])
            });
            for t in d.vertices.chunks_exact(3) {
                if merged >= cap {
                    notes.push(format!(
                        "map shapes: past {cap} vertices, the rest not drawn"
                    ));
                    break 'all;
                }
                merged += 3;
                let mut v = [0, 1, 2].map(|i| {
                    let x = t[i];
                    let p = x.position;
                    let n = x.normal;
                    Vertex {
                        position: m.mul_p((Vec3::from(p) * Vec3::from(s)).to_array()),
                        normal: unit_or_same(
                            m.mul_v((Vec3::from(n) / Vec3::from(s)).to_array()).into(),
                        )
                        .to_array(),
                        ..x
                    }
                });
                if mirrored {
                    v.swap(1, 2);
                }
                for x in v {
                    for c in 0..3 {
                        entry.3[c] += f64::from(x.position[c]);
                    }
                    entry.2.push(x);
                }
            }
        }
    }
    groups
        .into_iter()
        .enumerate()
        .map(|(n, ((path, k), (d, texture, vertices, sum)))| {
            let count = vertices.len().max(1) as f64;
            let mut h = Fnv::new("map static mesh");
            h.u64(key);
            h.u64(n as u64);
            Mesh {
                label: format!("shape {path} {k}"),
                texture,
                vertices: Arc::new(vertices),
                lighting: d.lighting,
                tex_env: d.tex_env,
                blend: d.blend,
                cull: d.cull,
                depth_write: d.depth_write,
                sampler: d.texture.map_or(super::SAMPLER, |t| t.1),
                alpha_from_texture: false,
                overlay: false,
                centre: sum.map(|c| (c / count) as f32),
                key: h.0,
            }
        })
        .collect()
}
