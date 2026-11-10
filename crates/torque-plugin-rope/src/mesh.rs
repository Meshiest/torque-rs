//! The ropes as triangles: each rope a tube of [`SIDES`] sides around its points, closed at both ends.

use glam::Vec3;

/// The sides of a tube.
pub const SIDES: usize = 8;
/// Bytes of one [`TubeVertex`] as the shaders read it: position, normal, color, texture place and scroll.
pub const VERTEX_BYTES: usize = 40;

/// One corner of a tube.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TubeVertex {
    /// Where it is in the world.
    pub position: [f32; 3],
    /// The way the tube's surface faces there (a unit vector).
    pub normal: [f32; 3],
    /// The rope's paint; the alpha is 255 where shadows fall on the rope, else 0.
    pub color: [u8; 4],
    /// Where the corner is on the texture: `u` in repeats along the rope (by arc length, so it runs on across the joints), `v`
    /// around the tube from 0 to 1 (the seam has a corner at each end of the range).
    pub uv: [f32; 2],
    /// How fast the texture runs along the rope, repeats a second.
    pub scroll: f32,
}

/// A rope to make a tube of.
#[derive(Clone, Copy, Debug)]
pub struct TubeSource<'a> {
    /// The points, from the start to the end.
    pub points: &'a [Vec3],
    /// The tube's width.
    pub thickness: f32,
    /// The paint.
    pub color: [u8; 4],
    /// Whether it casts a shadow.
    pub cast_shadow: bool,
    /// Whether shadows fall on it.
    pub receive_shadow: bool,
    /// The texture's number in the world's table (0: none).
    pub texture: u32,
    /// The world length of one repeat of the texture.
    pub texture_length: f32,
    /// How fast the texture runs, repeats a second.
    pub scroll: f32,
    /// The width at the far end (the tube tapers to it from `thickness`).
    pub thickness_end: f32,
    /// The color at the far end (the paint blends to it along the rope).
    pub color_end: Option<[u8; 4]>,
}

/// A run of a mesh's indices drawn with one texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draw {
    /// The texture's number in the world's table (0: none).
    pub texture: u32,
    /// Whether the triangles are those of ropes that cast a shadow.
    pub casting: bool,
    /// Whether the strips of the run add their light (never for a tube).
    pub additive: bool,
    /// The first index, and how many.
    pub first: u32,
    /// How many indices.
    pub count: u32,
}

/// The tubes of a set of ropes: the vertices, and the triangles as indices into them. The triangles of the ropes that cast a
/// shadow come first, so a shadow pass draws a prefix of them ([`TubeMesh::shadow_indices`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TubeMesh {
    /// The [`RopeWorld::revision`](crate::RopeWorld::revision) it was made for.
    pub revision: u64,
    /// The corners.
    pub vertices: Vec<TubeVertex>,
    /// Three indices a triangle.
    pub indices: Vec<u32>,
    /// How many of the first indices are the triangles of ropes that cast a shadow.
    pub shadow_indices: usize,
    /// The index runs by texture, the ropes that cast first (a run is a stretch of ropes in order with one texture).
    pub draws: Vec<Draw>,
}

impl TubeMesh {
    /// The tubes of ropes given as their points, thickness, color, whether they cast a shadow and whether shadows fall on them (a
    /// vertex's alpha is 255 for the ropes that take a shadow, else 0). A rope with fewer than two points, or with a point that is
    /// not finite, is left out.
    pub fn build<'a>(
        revision: u64,
        ropes: impl Iterator<Item = TubeSource<'a>> + Clone,
    ) -> TubeMesh {
        let mut mesh = TubeMesh {
            revision,
            ..TubeMesh::default()
        };
        for casting in [true, false] {
            for rope in ropes.clone().filter(|r| r.cast_shadow == casting) {
                let before = mesh.indices.len() as u32;
                mesh.add_tube(&rope);
                let count = mesh.indices.len() as u32 - before;
                if count == 0 {
                    continue;
                }
                match mesh.draws.last_mut() {
                    Some(d) if d.texture == rope.texture && d.casting == casting => {
                        d.count += count
                    }
                    _ => mesh.draws.push(Draw {
                        texture: rope.texture,
                        casting,
                        additive: false,
                        first: before,
                        count,
                    }),
                }
            }
            if casting {
                mesh.shadow_indices = mesh.indices.len();
            }
        }
        mesh
    }

    /// The vertices as the shaders read them (little endian).
    pub fn vertex_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.vertices.len() * VERTEX_BYTES);
        for v in &self.vertices {
            for c in v.position.into_iter().chain(v.normal) {
                out.extend_from_slice(&c.to_le_bytes());
            }
            out.extend_from_slice(&v.color);
            for c in [v.uv[0], v.uv[1], v.scroll] {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        out
    }

    /// The indices as the shaders read them (little endian).
    pub fn index_bytes(&self) -> Vec<u8> {
        self.indices.iter().flat_map(|i| i.to_le_bytes()).collect()
    }

    fn add_tube(&mut self, rope: &TubeSource<'_>) {
        let points = rope.points;
        if points.len() < 2 || points.iter().any(|p| !p.is_finite()) {
            return;
        }
        let shadow_flag = if rope.receive_shadow { 255 } else { 0 };
        // The paint and the radius `share` (0 to 1) of the way along the rope: the datablock's taper and gradient.
        let paint = |share: f32| {
            let end = rope.color_end.unwrap_or(rope.color);
            let mix =
                |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * share).round() as u8;
            [
                mix(rope.color[0], end[0]),
                mix(rope.color[1], end[1]),
                mix(rope.color[2], end[2]),
                shadow_flag,
            ]
        };
        let radius_at =
            |share: f32| (rope.thickness + (rope.thickness_end - rope.thickness) * share) * 0.5;
        // How far along the rope each point is, in repeats of the texture.
        let repeat = if rope.texture_length > 0.0 {
            rope.texture_length
        } else {
            rope.thickness.max(1.0e-3)
        };
        let mut along = vec![0.0f32; points.len()];
        for i in 1..points.len() {
            along[i] = along[i - 1] + points[i].distance(points[i - 1]) / repeat;
        }
        let last = points.len() - 1;
        let share = |i: usize| {
            if along[last] > 0.0 {
                (along[i] / along[last]).clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        let tangent = |i: usize| -> Vec3 {
            let (a, b) = (points[i.saturating_sub(1)], points[(i + 1).min(last)]);
            (b - a).try_normalize().unwrap_or(Vec3::X)
        };
        // The rings: a closing disc at each end (a ring of no radius, then a ring facing along the rope), and the rope's own rings
        // between, each turned from the one before by the least turn that follows the rope (so the tube does not twist).
        let first = self.vertices.len() as u32;
        let mut rings = 0u32;
        let mut normal = tangent(0).any_orthonormal_vector();
        // A ring has a corner more than it has sides: the first corner again, at the far end of the texture's v.
        let mut ring = |mesh: &mut TubeMesh,
                        at: Vec3,
                        t: Vec3,
                        normal: Vec3,
                        r: f32,
                        facing: Option<Vec3>,
                        u: f32,
                        color: [u8; 4]| {
            let side = t.cross(normal);
            for s in 0..=SIDES {
                let a = std::f32::consts::TAU * (s % SIDES) as f32 / SIDES as f32;
                let out = normal * a.cos() + side * a.sin();
                mesh.vertices.push(TubeVertex {
                    position: (at + out * r).to_array(),
                    normal: facing.unwrap_or(out).to_array(),
                    color,
                    uv: [u, s as f32 / SIDES as f32],
                    scroll: rope.scroll,
                });
            }
            rings += 1;
        };
        let t0 = tangent(0);
        ring(
            self,
            points[0],
            t0,
            normal,
            0.0,
            Some(-t0),
            along[0],
            paint(0.0),
        );
        ring(
            self,
            points[0],
            t0,
            normal,
            radius_at(0.0),
            Some(-t0),
            along[0],
            paint(0.0),
        );
        for (i, &p) in points.iter().enumerate() {
            let t = tangent(i);
            let kept = normal - t * normal.dot(t);
            normal = kept
                .try_normalize()
                .unwrap_or_else(|| t.any_orthonormal_vector());
            ring(
                self,
                p,
                t,
                normal,
                radius_at(share(i)),
                None,
                along[i],
                paint(share(i)),
            );
        }
        let t1 = tangent(last);
        ring(
            self,
            points[last],
            t1,
            normal,
            radius_at(1.0),
            Some(t1),
            along[last],
            paint(1.0),
        );
        ring(
            self,
            points[last],
            t1,
            normal,
            0.0,
            Some(t1),
            along[last],
            paint(1.0),
        );
        let stride = SIDES as u32 + 1;
        for r in 0..rings - 1 {
            for s in 0..SIDES as u32 {
                let a = first + r * stride;
                let b = a + stride;
                self.indices
                    .extend([a + s, b + s, b + s + 1, a + s, b + s + 1, a + s + 1]);
            }
        }
    }
}
