//! The semantic check: bounds, centres and radii recomputed from the parsed vertices (the structural round trip proves the
//! reader skips no field; this proves it read the right ones).
//!
//! **What a writer stores, and what is checked.**
//! - *Each mesh*: `bounds` is the box of every vertex of the mesh (all frames), `center` its middle, `radius` the largest distance
//!   from the centre to a vertex, truncated to the integer the file holds: exactly `TSMesh::computeBounds` over the identity
//!   transform, and a skin mesh uses its initial vertices (`TSSkinMesh::computeBounds`). The check is
//!   bit equality of the f32 box, the centre within the tolerance below and the truncated radius.
//! - *The shape*: `bounds` is **not** the geometry's box. The Torque exporters take it from a box the artist builds and leaves out
//!   of the file (`ShapeMimic::generateBounds`, `max2dtsExporter`: the mesh of the node called `Bounds`), so it cannot be
//!   recomputed from the parsed vertices. What the exporter derives from that box can: `center` is the middle of the box, `radius`
//!   the distance from the centre to the farthest of its eight corners (the "smallest radius that includes the bounds") and
//!   `tubeRadius` the same in the horizontal plane. The check recomputes the three from the stored box, and
//!   reports separately whether the geometry of detail level 0, placed by the default node transforms
//!   (`TSShape::computeBounds`), lies inside the stored box (informative: an artist's box need not contain
//!   everything).
//!
//! **The rule for "equal".** The box of a mesh is exact f32 (bit equality, `-0` equal to `0`). The centre, the radii and the
//! shape's figures are derived values that a writer may compute in another precision (a float halved or a square root taken in
//! double), so they are compared with a tolerance of [`MAX_ULPS`] units in the last place, measured against the size of the box
//! (a centre near zero is the difference of two numbers near one): see [`close`]. A mesh's radius word is the integer
//! `(S32)mRadius` of Torque's writer; a writer may store the float's own bits in that word instead, which is accepted within
//! [`MAX_ULPS`]. [`Report`] counts how many met the exact value and how many only the tolerance, and the largest distance seen.
//! A mesh whose centre and radius words are both zero while the recomputed ones are not is **unset** (a writer may leave them
//! zero; its box is still exact): it is counted on its own and accepted, so that a field read from the wrong place still shows
//! in a mesh that does write them.

use crate::shape::{Mesh, MeshExtra, MeshRender, Model, Shape};
use crate::{Charge, ShapeError, pay};

/// The most units in the last place a centre or a radius may differ from the recomputed value.
pub const MAX_ULPS: u32 = 8;

/// `a` and `b` agree to [`MAX_ULPS`] units in the last place of `scale` (the size of the numbers they were made from): for a
/// value near zero the unit is the one of the box that produced it, not the value's own.
pub fn close(a: f32, b: f32, scale: f32) -> bool {
    a == b || (a - b).abs() <= MAX_ULPS as f32 * f32::EPSILON * scale.abs().max(f32::MIN_POSITIVE)
}

/// The distance between two floats in units in the last place (`u32::MAX` when they differ in sign or one is not finite).
pub fn ulps(a: f32, b: f32) -> u32 {
    if a == b {
        return 0;
    }
    if !a.is_finite() || !b.is_finite() || (a < 0.0) != (b < 0.0) {
        return u32::MAX;
    }
    a.to_bits().abs_diff(b.to_bits())
}

/// A 4x4 matrix as `MatrixF` holds it: row-major, the translation in the last column.
type Mat = [f32; 16];

/// A mesh's box, centre and radius as `TSMesh::computeBounds` finds them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Computed {
    pub bounds: [f32; 6],
    pub center: [f32; 3],
    pub radius: f32,
}

/// `TSMesh::computeBounds(v, n, identity)`: no vertices give the zero box, centre and radius.
pub fn compute(verts: &[[f32; 3]]) -> Computed {
    let Some(first) = verts.first() else {
        return Computed {
            bounds: [0.0; 6],
            center: [0.0; 3],
            radius: 0.0,
        };
    };
    let (mut min, mut max) = (*first, *first);
    for v in verts {
        for k in 0..3 {
            // `setMax` / `setMin` as Torque has them: a compare and a store, so a NaN never replaces a number.
            if v[k] > max[k] {
                max[k] = v[k];
            }
            if v[k] < min[k] {
                min[k] = v[k];
            }
        }
    }
    let center = [
        0.5 * (min[0] + max[0]),
        0.5 * (min[1] + max[1]),
        0.5 * (min[2] + max[2]),
    ];
    let mut r2 = 0.0f32;
    for v in verts {
        let p = [v[0] - center[0], v[1] - center[1], v[2] - center[2]];
        let d = p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
        if d > r2 {
            r2 = d;
        }
    }
    Computed {
        bounds: [min[0], min[1], min[2], max[0], max[1], max[2]],
        center,
        radius: r2.sqrt(),
    }
}

/// The vertices a mesh's bounds are taken over: its own, or a skin mesh's initial vertices.
fn bound_verts<'a>(m: &'a Mesh, r: &'a MeshRender) -> &'a [[f32; 3]] {
    match &r.extra {
        MeshExtra::Skin(s) => s
            .initial_verts
            .get(..s.num_initial_verts)
            .unwrap_or(&s.initial_verts),
        _ => m.verts(),
    }
}

/// The centre, radius and tube radius the exporter derives from the shape's box: the middle, the farthest corner and the
/// farthest corner in the horizontal plane (`ShapeMimic::generateBounds`, over the eight corners of the box).
pub fn shape_derived(bounds: &[f32; 6]) -> ([f32; 3], f32, f32) {
    let center = [
        (bounds[0] + bounds[3]) * 0.5,
        (bounds[1] + bounds[4]) * 0.5,
        (bounds[2] + bounds[5]) * 0.5,
    ];
    let (mut r2, mut t2) = (-1.0f32, -1.0f32);
    for corner in 0..8 {
        let p = [
            bounds[if corner & 1 == 0 { 0 } else { 3 }] - center[0],
            bounds[if corner & 2 == 0 { 1 } else { 4 }] - center[1],
            bounds[if corner & 4 == 0 { 2 } else { 5 }] - center[2],
        ];
        let d = p[0] * p[0] + p[1] * p[1] + p[2] * p[2];
        if d > r2 {
            r2 = d;
        }
        let d = p[0] * p[0] + p[1] * p[1];
        if d > t2 {
            t2 = d;
        }
    }
    (center, r2.sqrt(), t2.sqrt())
}

fn identity() -> Mat {
    let mut m = [0.0; 16];
    m[0] = 1.0;
    m[5] = 1.0;
    m[10] = 1.0;
    m[15] = 1.0;
    m
}

/// `a * b` as `m_matF_x_matF` multiplies.
fn mul(a: &Mat, b: &Mat) -> Mat {
    let mut r = [0.0; 16];
    for i in 0..4 {
        for j in 0..4 {
            r[i * 4 + j] = a[i * 4] * b[j]
                + a[i * 4 + 1] * b[4 + j]
                + a[i * 4 + 2] * b[8 + j]
                + a[i * 4 + 3] * b[12 + j];
        }
    }
    r
}

/// `TSTransform::setMatrix(Quat16::getQuatF, translation)`.
fn node_matrix(q: [i16; 4], t: [f32; 3]) -> Mat {
    let f = |v: i16| f32::from(v) / 32767.0;
    let (x, y, z, w) = (f(q[0]), f(q[1]), f(q[2]), f(q[3]));
    let mut m = [0.0; 16];
    if x * x + y * y + z * z < 10e-20 {
        m = identity();
    } else {
        let (xs, ys, zs) = (x * 2.0, y * 2.0, z * 2.0);
        let (wx, wy, wz) = (w * xs, w * ys, w * zs);
        let (xx, xy, xz) = (x * xs, x * ys, x * zs);
        let (yy, yz, zz) = (y * ys, y * zs, z * zs);
        m[0] = 1.0 - (yy + zz);
        m[4] = xy - wz;
        m[8] = xz + wy;
        m[1] = xy + wz;
        m[5] = 1.0 - (xx + zz);
        m[9] = yz - wx;
        m[2] = xz - wy;
        m[6] = yz + wx;
        m[10] = 1.0 - (xx + yy);
        m[15] = 1.0;
    }
    m[3] = t[0];
    m[7] = t[1];
    m[11] = t[2];
    m
}

fn mul_point(m: &Mat, p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3],
        m[4] * p[0] + m[5] * p[1] + m[6] * p[2] + m[7],
        m[8] * p[0] + m[9] * p[1] + m[10] * p[2] + m[11],
    ]
}

/// `TSShape::computeBounds(dl, bounds)`: the box, in shape space, of the meshes of detail `dl`, each
/// placed by its node's default transform (the chain from the root of its sub-shape). `None` when the detail does not exist,
/// has no mesh, or the shape's tables are inconsistent (an index out of range: Torque would read past its arrays there).
/// Charged before the work: the nodes placed, and for every object the vertices of its mesh (an object is walked each time it is named, however
/// many objects share one mesh); a run that cannot pay ends with [`ShapeError::OutOfBudget`].
pub fn detail_bounds(
    shape: &Shape,
    dl: usize,
    charge: Charge,
) -> Result<Option<[f32; 6]>, ShapeError> {
    let mut refused = false;
    let found = detail_walk(shape, dl, &mut |units| {
        let paid = charge(units);
        refused |= !paid;
        paid
    });
    if refused {
        Err(ShapeError::OutOfBudget)
    } else {
        Ok(found)
    }
}

// The vertices [`detail_walk`] has walked on this thread: the tests measure the work a budget allows by it, not by the clock.
#[cfg(test)]
thread_local! {
    static WALKED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn detail_walk(shape: &Shape, dl: usize, pay: &mut dyn FnMut(u64) -> bool) -> Option<[f32; 6]> {
    let detail = shape.details.get(dl)?;
    let ss = shape
        .sub_shapes
        .get(usize::try_from(detail.sub_shape).ok()?)?;
    let od = detail.object_detail;
    let first_node = usize::try_from(ss.first_node).ok()?;
    let num_nodes = usize::try_from(ss.num_nodes).ok()?;
    let first_object = usize::try_from(ss.first_object).ok()?;
    let num_objects = usize::try_from(ss.num_objects).ok()?;
    if !pay(8 + num_nodes.min(shape.nodes.len()) as u64) {
        return None;
    }
    let mut mats: Vec<Mat> = Vec::new();
    for i in first_node..first_node.checked_add(num_nodes)? {
        let local = node_matrix(
            *shape.default_rotations.get(i)?,
            *shape.default_translations.get(i)?,
        );
        let parent = shape.nodes.get(i)?.parent;
        let m = if parent >= 0 {
            let p = usize::try_from(parent).ok()?.checked_sub(first_node)?;
            mul(mats.get(p)?, &local)
        } else {
            local
        };
        mats.push(m);
    }
    let mut min = [1e31f32; 3];
    let mut max = [-1e31f32; 3];
    let mut any = false;
    for i in first_object..first_object.checked_add(num_objects)? {
        let object = shape.objects.get(i)?;
        if od < 0 || od >= object.num_meshes {
            continue;
        }
        let at = usize::try_from(object.start_mesh_index.checked_add(od)?).ok()?;
        let Some(Some(mesh)) = shape.meshes.get(at) else {
            continue;
        };
        let mat = if object.node < 0 {
            identity()
        } else {
            let n = usize::try_from(object.node).ok()?.checked_sub(first_node)?;
            *mats.get(n)?
        };
        if !pay(4 + mesh.verts().len() as u64) {
            return None;
        }
        #[cfg(test)]
        WALKED.with(|w| w.set(w.get() + mesh.verts().len() as u64));
        for v in mesh.verts() {
            let p = mul_point(&mat, *v);
            for k in 0..3 {
                if p[k] < min[k] {
                    min[k] = p[k];
                }
                if p[k] > max[k] {
                    max[k] = p[k];
                }
            }
            any = true;
        }
    }
    any.then_some([min[0], min[1], min[2], max[0], max[1], max[2]])
}

/// What [`check`] found in one shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Report {
    /// Meshes checked (the slots that hold a mesh).
    pub meshes: usize,
    /// Of those, the ones whose stored box equals the recomputed one bit for bit.
    pub mesh_box_equal: usize,
    /// ... whose stored centre equals the recomputed one bit for bit, and within the tolerance (the first are in the second).
    pub mesh_center_exact: usize,
    pub mesh_center_close: usize,
    /// ... whose stored radius is the recomputed one truncated, and whose stored word is the float's bits within the tolerance.
    pub mesh_radius_int: usize,
    pub mesh_radius_float: usize,
    /// The largest distance of a mesh's float-form radius from the recomputed one, in units in the last place.
    pub mesh_radius_ulps: u32,
    /// Meshes whose centre and radius words are both zero though the recomputed ones are not: a writer may leave them
    /// unset (the box is still exact). Counted in neither of the groups above.
    pub mesh_unset: usize,
    /// The shape's centre is the one derived from its stored box, exactly and within the tolerance.
    pub shape_center_exact: bool,
    pub shape_center_close: bool,
    /// The shape's radius and tube radius against the ones derived from the box.
    pub shape_radius: Figure,
    pub shape_tube: Figure,
    /// Detail 0's geometry lies inside the stored box (`None`: it has none to place).
    pub detail0_inside: Option<bool>,
}

impl Report {
    /// Every mesh and the shape's derived figures equal under the rule of the module doc: what the exit line counts as "bounds
    /// recomputed equal".
    pub fn all_equal(&self) -> bool {
        self.mesh_box_equal == self.meshes
            && self.mesh_center_close + self.mesh_unset == self.meshes
            && self.mesh_radius_int + self.mesh_radius_float + self.mesh_unset == self.meshes
            && self.shape_center_close
            && self.shape_radius != Figure::Outside
            && self.shape_tube != Figure::Outside
    }

    /// Something needed the tolerance: a centre, a radius or the shape's figures were not the recomputed value to the bit.
    pub fn needed_tolerance(&self) -> bool {
        self.mesh_center_close != self.mesh_center_exact
            || self.mesh_radius_float != 0
            || !self.shape_center_exact
            || matches!(self.shape_radius, Figure::Within(_))
            || matches!(self.shape_tube, Figure::Within(_))
    }

    /// The shape's radius or tube radius is smaller than the box's corners give: the artist's bounds object was not a box.
    pub fn sphere_bounded(&self) -> bool {
        self.shape_radius == Figure::Inside || self.shape_tube == Figure::Inside
    }
}

/// A shape's radius or tube radius against the value derived from its stored box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Figure {
    /// Bit for bit the value the box's corners give.
    Exact,
    /// Within the tolerance of it (the distance in units in the last place).
    Within(u32),
    /// Smaller than the corners give, but not smaller than half the box's longest side: a writer may take the vertices of
    /// the artist's bounds object, which need not be a box (a sphere reaches each face but not the corners).
    Inside,
    /// Larger than the corners give, or too small to reach the box's faces: the field was not read right.
    #[default]
    Outside,
}

/// Classifies `stored` against `derived` (the corner value) and `lower` (half the longest side the figure covers).
fn figure(stored: f32, derived: f32, lower: f32) -> Figure {
    let d = ulps(stored, derived);
    if d == 0 {
        Figure::Exact
    } else if d <= MAX_ULPS {
        Figure::Within(d)
    } else if stored < derived && stored >= lower * (1.0 - MAX_ULPS as f32 * f32::EPSILON) {
        Figure::Inside
    } else {
        Figure::Outside
    }
}

fn same(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.to_bits() == y.to_bits() || x == y)
}

/// Recomputes the bounds of every mesh and the figures of the shape from the parsed vertices and compares them with the stored
/// ones (the module doc states what is compared and why). Charged per vertex before it is looked at.
pub fn check(model: &Model, charge: Charge) -> Result<Report, ShapeError> {
    let shape = &model.shape;
    let mut report = Report::default();
    for (m, r) in shape.meshes.iter().zip(&model.render) {
        let (Some(m), Some(r)) = (m, r) else {
            continue;
        };
        let verts = bound_verts(m, r);
        pay(charge, 2 * verts.len() as u64 + 4)?;
        let c = compute(verts);
        report.meshes += 1;
        report.mesh_box_equal += usize::from(same(&m.bounds, &c.bounds));
        let scale = |k: usize| c.bounds[k].abs().max(c.bounds[3 + k].abs());
        let exact = same(&m.center, &c.center);
        let close_center = exact || (0..3).all(|k| close(m.center[k], c.center[k], scale(k)));
        // Torque turns the float into the integer the file holds by truncation (`(S32)mRadius`).
        let radius_int = m.radius == c.radius as i32;
        let float_ulps = ulps(f32::from_bits(m.radius as u32), c.radius);
        let radius_float = !radius_int && float_ulps <= MAX_ULPS;
        if !(close_center && (radius_int || radius_float)) && m.center == [0.0; 3] && m.radius == 0
        {
            report.mesh_unset += 1;
            continue;
        }
        report.mesh_center_exact += usize::from(exact);
        report.mesh_center_close += usize::from(close_center);
        report.mesh_radius_int += usize::from(radius_int);
        if radius_float {
            report.mesh_radius_float += 1;
            report.mesh_radius_ulps = report.mesh_radius_ulps.max(float_ulps);
        }
    }
    let (center, radius, tube) = shape_derived(&shape.bounds);
    report.shape_center_exact = same(&shape.center, &center);
    report.shape_center_close = report.shape_center_exact
        || (0..3).all(|k| {
            close(
                shape.center[k],
                center[k],
                shape.bounds[k].abs().max(shape.bounds[3 + k].abs()),
            )
        });
    let half = |k: usize| (shape.bounds[3 + k] - shape.bounds[k]) * 0.5;
    report.shape_radius = figure(shape.radius, radius, half(0).max(half(1)).max(half(2)));
    report.shape_tube = figure(shape.tube_radius, tube, half(0).max(half(1)));
    report.detail0_inside = detail_bounds(shape, 0, charge)?.map(|g| {
        let b = &shape.bounds;
        (0..3).all(|k| g[k] >= b[k] && g[3 + k] <= b[3 + k])
    });
    Ok(report)
}

#[cfg(test)]
#[path = "bounds_tests.rs"]
mod tests;
