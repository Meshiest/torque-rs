//! Per-vertex tangents for normal mapping: MikkTSpace (`bevy_mikktspace`, a safe Rust translation of the reference implementation),
//! the frame normal-map bakers use, so a map baked against it reads back as it was baked. A tangent is `[x, y, z, w]` with the
//! bitangent's handedness in `w` (`bitangent = w * cross(normal, tangent)`, `w` is 1 or -1, whichever way the triangles
//! wind); maps follow OpenGL's convention (+Y along the bitangent, `dP/dv`).

use bevy_mikktspace::{Geometry, TangentSpace, generate_tangents};

use crate::math::Vec3;

/// An indexed triangle list as MikkTSpace reads it, and the tangents it writes (one a vertex: a vertex shared by faces takes
/// the last face's, which MikkTSpace makes equal unless the faces' UV frames split it).
struct Triangles<'a> {
    positions: &'a [Vec3],
    normals: &'a [Vec3],
    uvs: &'a [[f32; 2]],
    /// The triangles whose three indices are in range.
    faces: Vec<[usize; 3]>,
    out: Vec<[f32; 4]>,
}

impl Geometry for Triangles<'_> {
    fn num_faces(&self) -> usize {
        self.faces.len()
    }

    fn num_vertices_of_face(&self, _face: usize) -> usize {
        3
    }

    fn position(&self, face: usize, vert: usize) -> [f32; 3] {
        self.positions[self.faces[face][vert]]
    }

    fn normal(&self, face: usize, vert: usize) -> [f32; 3] {
        self.normals
            .get(self.faces[face][vert])
            .copied()
            .unwrap_or([0.0, 0.0, 1.0])
    }

    fn tex_coord(&self, face: usize, vert: usize) -> [f32; 2] {
        self.uvs
            .get(self.faces[face][vert])
            .copied()
            .unwrap_or([0.0; 2])
    }

    fn set_tangent(&mut self, tangent_space: Option<TangentSpace>, face: usize, vert: usize) {
        // The handedness from the bitangent itself: MikkTSpace's own sign assumes counterclockwise fronts, and this crate's
        // are clockwise (a sign from the winding would flip every map's green channel).
        if let Some(ts) = tangent_space {
            let [x, y, z] = ts.tangent();
            let n = self.normal(face, vert);
            let b = ts.bi_tangent();
            let nxt = [
                n[1] * z - n[2] * y,
                n[2] * x - n[0] * z,
                n[0] * y - n[1] * x,
            ];
            let w = if nxt[0] * b[0] + nxt[1] * b[1] + nxt[2] * b[2] < 0.0 {
                -1.0
            } else {
                1.0
            };
            self.out[self.faces[face][vert]] = [x, y, z, w];
        }
    }
}

/// Any unit tangent perpendicular to `n` (a vertex no face gives a frame).
fn perpendicular(n: Vec3) -> [f32; 4] {
    let axis: Vec3 = if n[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let d = axis[0] * n[0] + axis[1] * n[1] + axis[2] * n[2];
    let t = [axis[0] - n[0] * d, axis[1] - n[1] * d, axis[2] - n[2] * d];
    let l = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
    if l > 1e-6 && l.is_finite() {
        [t[0] / l, t[1] / l, t[2] / l, 1.0]
    } else {
        [1.0, 0.0, 0.0, 1.0]
    }
}

/// The most triangles that may meet at one vertex (vertices of equal position, normal and UV counted as one, as MikkTSpace
/// welds them) for MikkTSpace: its grouping recurses once a triangle around a vertex (about 200 bytes a level unoptimized), so
/// a hostile fan of many thousands overflows a thread's stack. A mesh past it takes [`face_frames`] instead.
pub const MAX_VALENCE: u32 = 512;
/// The largest coordinate taken (larger ones are clamped: their squares stay finite in `f32`).
const MAX_COORD: f32 = 1.0e9;

/// `v` finite and within [`MAX_COORD`] (a NaN is 0).
fn tame(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(-MAX_COORD, MAX_COORD)
    }
}

/// The MikkTSpace tangents of an indexed triangle list (module docs): one a vertex. `normals` and `uvs` are one a vertex (a
/// missing one reads +Z and 0, 0); NaN components read 0 and infinite or huge ones are clamped; triangles with an index out
/// of range are left out, and a vertex no face gives a frame gets one perpendicular to its normal. A mesh with more than
/// [`MAX_VALENCE`] triangles at a vertex gets [`face_frames`].
pub fn generate(
    positions: &[Vec3],
    normals: &[Vec3],
    uvs: &[[f32; 2]],
    indices: &[u32],
) -> Vec<[f32; 4]> {
    let n = positions.len();
    let positions: Vec<Vec3> = positions.iter().map(|p| p.map(tame)).collect();
    let normals: Vec<Vec3> = (0..n)
        .map(|i| normals.get(i).map_or([0.0, 0.0, 1.0], |v| v.map(tame)))
        .collect();
    let uvs: Vec<[f32; 2]> = (0..n)
        .map(|i| uvs.get(i).map_or([0.0; 2], |v| v.map(tame)))
        .collect();
    let faces: Vec<[usize; 3]> = indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|i| i as usize))
        .filter(|t| t.iter().all(|&i| i < n))
        .collect();
    let fallback: Vec<[f32; 4]> = normals.iter().map(|&nm| perpendicular(nm)).collect();
    if max_valence(&positions, &normals, &uvs, &faces) > MAX_VALENCE {
        return face_frames(&positions, &normals, &uvs, &faces, fallback);
    }
    let mut tris = Triangles {
        positions: &positions,
        normals: &normals,
        uvs: &uvs,
        faces,
        out: fallback,
    };
    // The crate's error type has no variants: it cannot fail.
    let _ = generate_tangents(&mut tris);
    tris.out
}

/// The most triangle corners at one welded vertex (equal position, normal and UV bits).
fn max_valence(
    positions: &[Vec3],
    normals: &[Vec3],
    uvs: &[[f32; 2]],
    faces: &[[usize; 3]],
) -> u32 {
    let mut count: std::collections::HashMap<[u32; 8], u32> = std::collections::HashMap::new();
    let mut most = 0;
    for &i in faces.iter().flatten() {
        let (p, nm, uv) = (positions[i], normals[i], uvs[i]);
        let key = [p[0], p[1], p[2], nm[0], nm[1], nm[2], uv[0], uv[1]].map(f32::to_bits);
        let c = count.entry(key).or_insert(0);
        *c += 1;
        most = most.max(*c);
    }
    most
}

/// Tangents without MikkTSpace's grouping (no recursion): each triangle's UV frame summed at its indexed vertices, made
/// orthogonal to the normal, the handedness from the summed bitangent; `out` holds the frames of vertices no face reaches.
pub fn face_frames(
    positions: &[Vec3],
    normals: &[Vec3],
    uvs: &[[f32; 2]],
    faces: &[[usize; 3]],
    mut out: Vec<[f32; 4]>,
) -> Vec<[f32; 4]> {
    let mut sums = vec![([0.0f32; 3], [0.0f32; 3]); positions.len()];
    for f in faces {
        let [a, b, c] = f.map(|i| positions[i]);
        let [ta, tb, tc] = f.map(|i| uvs[i]);
        let (e1, e2) = (
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]],
            [c[0] - a[0], c[1] - a[1], c[2] - a[2]],
        );
        let (du1, dv1, du2, dv2) = (tb[0] - ta[0], tb[1] - ta[1], tc[0] - ta[0], tc[1] - ta[1]);
        let det = du1 * dv2 - du2 * dv1;
        if !det.is_finite() || det.abs() < 1e-20 {
            continue;
        }
        let r = 1.0 / det;
        let s: Vec3 = std::array::from_fn(|k| (e1[k] * dv2 - e2[k] * dv1) * r);
        let t: Vec3 = std::array::from_fn(|k| (e2[k] * du1 - e1[k] * du2) * r);
        if !s.iter().chain(&t).all(|v| v.is_finite()) {
            continue;
        }
        for &i in f {
            for k in 0..3 {
                sums[i].0[k] += s[k];
                sums[i].1[k] += t[k];
            }
        }
    }
    for (i, (s, t)) in sums.into_iter().enumerate() {
        let nm = normals[i];
        let d = nm[0] * s[0] + nm[1] * s[1] + nm[2] * s[2];
        let o = [s[0] - nm[0] * d, s[1] - nm[1] * d, s[2] - nm[2] * d];
        let l = (o[0] * o[0] + o[1] * o[1] + o[2] * o[2]).sqrt();
        if !(l > 1e-12 && l.is_finite()) {
            continue;
        }
        let u = o.map(|v| v / l);
        let c = [
            nm[1] * u[2] - nm[2] * u[1],
            nm[2] * u[0] - nm[0] * u[2],
            nm[0] * u[1] - nm[1] * u[0],
        ];
        let w = if c[0] * t[0] + c[1] * t[1] + c[2] * t[2] < 0.0 {
            -1.0
        } else {
            1.0
        };
        out[i] = [u[0], u[1], u[2], w];
    }
    out
}

#[cfg(test)]
#[path = "tangents_tests.rs"]
mod tests;
