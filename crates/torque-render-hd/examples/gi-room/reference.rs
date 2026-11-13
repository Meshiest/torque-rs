//! The reference the probes are judged against: the diffuse ambient's radiance (the irradiance from every surface over pi, in
//! the forward pass's units: what multiplies a surface's albedo) at a region's points, path traced on the CPU against the scene's
//! own triangles (the static cells' and the planes'; the moving shapes cast nothing, as in the voxels). Each point takes
//! `PATHS` cosine-weighted paths of up to `BOUNCES` surfaces; at each surface the area lights' direct light (the forward
//! pass's: the radiance over the shape windowed to its radius, sampled over the shape), shadowed as the forward pass shadows
//! it (whole by its center's visibility; the light's own emitter lets rays through). Unlit surfaces (the lights' own boxes) are black. Every random number comes from a hash of the point,
//! the path and the bounce: the tables are the same every run.

use torque_render_hd::ext::PlainScene;
use torque_render_scene::hd::{AreaShape, HdAreaLight, HdVertex, material, srgb_to_linear};

use crate::scenes::Region;

/// Paths a point.
pub const PATHS: u32 = 4096;
/// The most surfaces a path meets after the point (the ambient's bounces).
pub const BOUNCES: u32 = 8;
/// Light samples on an area light's shape at each surface.
const LIGHT_SAMPLES: u32 = 4;

#[derive(Clone, Copy)]
struct Tri {
    a: [f32; 3],
    e1: [f32; 3],
    e2: [f32; 3],
    n: [f32; 3],
    albedo: [f32; 3],
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f32; 3], k: f32) -> [f32; 3] {
    a.map(|c| c * k)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    if l > 0.0 { scale(a, 1.0 / l) } else { a }
}

/// The scene the paths are traced in: its triangles and its area lights.
pub struct Traced {
    tris: Vec<Tri>,
    /// Runs of `GROUP` triangles in their order (a box's faces) and each run's box: a ray tests a run's triangles only when it
    /// meets the box.
    groups: Vec<([f32; 3], [f32; 3])>,
    lights: Vec<HdAreaLight>,
}

/// Triangles a run of [`Traced::groups`].
const GROUP: usize = 12;

/// A triangle of three vertices, its normal the vertices' (the winding turned to agree), its albedo the vertices' color in
/// linear light (black when unlit).
fn tri_of(v: [&HdVertex; 3]) -> Option<Tri> {
    let (a, b, c) = (v[0].position, v[1].position, v[2].position);
    let mut e1 = sub(b, a);
    let mut e2 = sub(c, a);
    let mut n = cross(e1, e2);
    if dot(n, n) < 1e-20 {
        return None;
    }
    let shading = torque_render_scene::hd::oct_decode(v[0].normal);
    if dot(n, shading) < 0.0 {
        std::mem::swap(&mut e1, &mut e2);
        n = scale(n, -1.0);
    }
    let unlit = v[0].material & (material::UNLIT | material::GLOW) != 0;
    let albedo = if unlit {
        [0.0; 3]
    } else {
        std::array::from_fn(|k| {
            (0..3)
                .map(|i| srgb_to_linear(f32::from(v[i].color[k]) / 255.0))
                .sum::<f32>()
                / 3.0
        })
    };
    Some(Tri {
        a,
        e1,
        e2,
        n: normalize(n),
        albedo,
    })
}

impl Traced {
    /// The scene's static cells' opaque batches and its opaque planes.
    pub fn of(scene: &PlainScene) -> Traced {
        let mut tris = Vec::new();
        let mut push = |vertices: &[HdVertex], indices: &[u32]| {
            for t in indices.as_chunks::<3>().0 {
                let v = t.map(|i| &vertices[i as usize]);
                tris.extend(tri_of(v));
            }
        };
        for cell in scene.frame.cells.cells.values() {
            for b in cell.batches.iter().filter(|b| !b.translucent) {
                push(
                    &cell.vertices,
                    &cell.indices[b.first as usize..(b.first + b.count) as usize],
                );
            }
        }
        for d in &scene.frame.planes {
            if d.blend == torque_render_scene::Blend::Opaque {
                push(&d.vertices, &d.indices);
            }
        }
        let groups = tris
            .chunks(GROUP)
            .map(|g| {
                let mut lo = [f32::INFINITY; 3];
                let mut hi = [f32::NEG_INFINITY; 3];
                for t in g {
                    for p in [t.a, add(t.a, t.e1), add(t.a, t.e2)] {
                        for k in 0..3 {
                            lo[k] = lo[k].min(p[k] - 1e-3);
                            hi[k] = hi[k].max(p[k] + 1e-3);
                        }
                    }
                }
                (lo, hi)
            })
            .collect();
        Traced {
            tris,
            groups,
            lights: scene.frame.area_lights.clone(),
        }
    }

    /// The nearest triangle the ray from `o` along `d` meets within `max`, and how far.
    fn hit(&self, o: [f32; 3], d: [f32; 3], max: f32) -> Option<(usize, f32)> {
        self.hit_with(o, d, max, false)
    }

    /// [`Traced::hit`], with `unlit` the unlit triangles (the lights' own shapes, black) let through: a light's shadow rays
    /// pass its emitter, as the forward pass's light and the voxels' do.
    fn hit_with(&self, o: [f32; 3], d: [f32; 3], max: f32, unlit: bool) -> Option<(usize, f32)> {
        let mut best: Option<(usize, f32)> = None;
        let mut far = max;
        let inv_d = d.map(|c| 1.0 / c);
        for (g, (lo, hi)) in self.groups.iter().enumerate() {
            let (mut t0, mut t1) = (0.0f32, far);
            for k in 0..3 {
                let a = (lo[k] - o[k]) * inv_d[k];
                let b = (hi[k] - o[k]) * inv_d[k];
                t0 = t0.max(a.min(b));
                t1 = t1.min(a.max(b));
            }
            if t0 > t1 {
                continue;
            }
            let first = g * GROUP;
            let end = (first + GROUP).min(self.tris.len());
            for (i, t) in self.tris[first..end]
                .iter()
                .enumerate()
                .map(|(k, t)| (first + k, t))
            {
                if unlit && t.albedo == [0.0; 3] {
                    continue;
                }
                let p = cross(d, t.e2);
                let det = dot(t.e1, p);
                if det.abs() < 1e-12 {
                    continue;
                }
                let inv = 1.0 / det;
                let s = sub(o, t.a);
                let u = dot(s, p) * inv;
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let q = cross(s, t.e1);
                let v = dot(d, q) * inv;
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let dist = dot(t.e2, q) * inv;
                if dist > 1e-4 && dist < far {
                    far = dist;
                    best = Some((i, dist));
                }
            }
        }
        best
    }

    /// The area lights' direct light at `p` facing `n` (the forward pass's units) from `LIGHT_SAMPLES` stratified points of each
    /// rectangle, shadowed as the forward pass shadows an area light: whole or not at all, by whether its center is in sight
    /// (the comparison judges the bounce, not the shadow's shape).
    fn direct(&self, p: [f32; 3], n: [f32; 3], rng: &mut Rng) -> [f32; 3] {
        let mut out = [0.0; 3];
        for l in &self.lights {
            let AreaShape::Rect { half_u, half_v } = l.shape else {
                continue;
            };
            let c = l.light.position;
            let radius = l.gpu_light().radius;
            let x = dot(sub(p, c), sub(p, c)).sqrt() / radius.max(1e-4);
            let w = (1.0 - x.powi(4)).clamp(0.0, 1.0);
            if w <= 0.0 {
                continue;
            }
            let ln = cross(half_u, half_v);
            let quarter = dot(ln, ln).sqrt();
            let ln = scale(ln, 1.0 / quarter);
            // The light's radiance per unit of color (area.wgsl's: 1 over |u x v|), over its whole area 4 |u x v|.
            let area = 4.0 * quarter;
            let to_c = sub(c, p);
            let dc = dot(to_c, to_c).sqrt();
            if self
                .hit_with(
                    add(p, scale(n, 1e-3)),
                    scale(to_c, 1.0 / dc),
                    dc - 0.05,
                    true,
                )
                .is_some()
            {
                continue;
            }
            let mut sum = 0.0;
            let side = (LIGHT_SAMPLES as f32).sqrt() as u32;
            for j in 0..side {
                for i in 0..side {
                    let a = ((i as f32 + rng.next()) / side as f32) * 2.0 - 1.0;
                    let b = ((j as f32 + rng.next()) / side as f32) * 2.0 - 1.0;
                    let q = add(c, add(scale(half_u, a), scale(half_v, b)));
                    let to = sub(q, p);
                    let d2 = dot(to, to);
                    let d = d2.sqrt();
                    let dir = scale(to, 1.0 / d);
                    let cp = dot(n, dir);
                    let cl = -dot(ln, dir);
                    if cp <= 0.0 || cl <= 0.0 {
                        continue;
                    }
                    sum += cp * cl / d2;
                }
            }
            let k = sum / (side * side) as f32 * area / quarter * w * w;
            for (o, col) in out.iter_mut().zip(l.light.color) {
                *o += col * k;
            }
        }
        out
    }

    /// The ambient's radiance at `p` facing `n`: the mean of `PATHS` paths' light.
    pub fn ambient(&self, p: [f32; 3], n: [f32; 3], seed: u32) -> [f32; 3] {
        let mut sum = [0.0f64; 3];
        for path in 0..PATHS {
            let mut rng = Rng::new(seed, path);
            let mut at = p;
            let mut normal = n;
            let mut throughput = [1.0f32; 3];
            for bounce in 0..BOUNCES {
                let d = rng.cosine(normal);
                let Some((i, t)) = self.hit(add(at, scale(normal, 1e-3)), d, 1e4) else {
                    break;
                };
                let tri = self.tris[i];
                // A back face (the inside of a wall) gives nothing.
                if dot(tri.n, d) >= 0.0 {
                    break;
                }
                at = add(add(at, scale(normal, 1e-3)), scale(d, t));
                normal = tri.n;
                throughput = std::array::from_fn(|k| throughput[k] * tri.albedo[k]);
                if throughput.iter().all(|&c| c <= 0.0) {
                    break;
                }
                let e = self.direct(at, normal, &mut rng);
                for k in 0..3 {
                    sum[k] += f64::from(throughput[k] * e[k]);
                }
                let _ = bounce;
            }
        }
        sum.map(|s| (s / f64::from(PATHS)) as f32)
    }

    /// Each region's mean ambient radiance over its `n` x `n` points (the points traced on all the machine's threads).
    pub fn regions(&self, regions: &[Region], n: usize) -> Vec<[f32; 3]> {
        regions
            .iter()
            .enumerate()
            .map(|(r, region)| {
                let points = region.points(n);
                let traced = on_threads(&points, |k, &p| {
                    self.ambient(p, region.normal, seed(r * 1000 + k))
                });
                let total = traced.into_iter().fold([0.0; 3], add);
                scale(total, 1.0 / points.len() as f32)
            })
            .collect()
    }

    /// The ambient radiance at each point facing its normal (on all the machine's threads; seeded apart from the regions').
    pub fn points(&self, points: &[([f32; 3], [f32; 3])]) -> Vec<[f32; 3]> {
        on_threads(points, |k, &(p, n)| {
            self.ambient(p, n, seed(POINT_SEEDS + k))
        })
    }
}

/// The first seed index of `Traced::points` (past every region's).
const POINT_SEEDS: usize = 1 << 20;

/// The paths' seed of the `index`th point traced.
fn seed(index: usize) -> u32 {
    (index as u32).wrapping_mul(0x9e37_79b9)
}

/// `f` of each item and its index, the items split over all the machine's threads, in the items' order.
fn on_threads<T: Sync, R: Send>(items: &[T], f: impl Fn(usize, &T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism().map_or(4, |t| t.get());
    let chunk = items.len().div_ceil(threads).max(1);
    let f = &f;
    std::thread::scope(|s| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .enumerate()
            .map(|(c, part)| {
                s.spawn(move || {
                    part.iter()
                        .enumerate()
                        .map(|(k, item)| f(c * chunk + k, item))
                        .collect::<Vec<R>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a reference thread"))
            .collect()
    })
}

/// A hashed stream of uniform numbers in [0, 1) (PCG's output of a counter).
struct Rng {
    state: u32,
}

impl Rng {
    fn new(seed: u32, path: u32) -> Rng {
        Rng {
            state: seed ^ path.wrapping_mul(0x85eb_ca6b).wrapping_add(0x27d4_eb2f),
        }
    }

    fn next(&mut self) -> f32 {
        self.state = self
            .state
            .wrapping_mul(747_796_405)
            .wrapping_add(2_891_336_453);
        let s = self.state;
        let w = ((s >> ((s >> 28) + 4)) ^ s).wrapping_mul(277_803_737);
        let x = (w >> 22) ^ w;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// A direction about `n`, cosine-weighted.
    fn cosine(&mut self, n: [f32; 3]) -> [f32; 3] {
        let (u1, u2) = (self.next(), self.next());
        let r = u1.sqrt();
        let phi = std::f32::consts::TAU * u2;
        let (x, y, z) = (r * phi.cos(), r * phi.sin(), (1.0 - u1).max(0.0).sqrt());
        let a = if n[0].abs() > 0.9 {
            [0.0, 1.0, 0.0]
        } else {
            [1.0, 0.0, 0.0]
        };
        let t = normalize(cross(a, n));
        let b = cross(n, t);
        normalize(add(add(scale(t, x), scale(b, y)), scale(n, z)))
    }
}

/// Rec. 709 luminance (`common.wgsl`'s).
pub fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}
