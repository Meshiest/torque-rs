//! The exact triangle-box overlap test (the separating axes of Akenine-Moller 2001), the CPU's side of `gi_voxel.wgsl`'s: the
//! voxels it fills are the reference the GPU's (tested with a box a hair larger) must hold.

use glam::Vec3;

/// Whether the triangle (corners less the box's middle) lies wholly on one side of the box of half sides `h` along `axis`.
fn separated(axis: Vec3, v: [Vec3; 3], h: Vec3) -> bool {
    let p = v.map(|c| c.dot(axis));
    let r = h.dot(axis.abs());
    p[0].min(p[1]).min(p[2]) > r || p[0].max(p[1]).max(p[2]) < -r
}

/// Whether the triangle `t` overlaps the box about `center` of half sides `h`.
pub(crate) fn overlaps(center: Vec3, h: Vec3, t: [Vec3; 3]) -> bool {
    let v = t.map(|c| c - center);
    let edges = [v[1] - v[0], v[2] - v[1], v[0] - v[2]];
    for e in edges {
        for axis in [
            Vec3::new(0.0, -e.z, e.y),
            Vec3::new(e.z, 0.0, -e.x),
            Vec3::new(-e.y, e.x, 0.0),
        ] {
            if separated(axis, v, h) {
                return false;
            }
        }
    }
    if [Vec3::X, Vec3::Y, Vec3::Z]
        .into_iter()
        .any(|axis| separated(axis, v, h))
    {
        return false;
    }
    !separated(edges[0].cross(edges[1]), v, h)
}

/// The voxels of side `size` the triangles fill, as world voxel coordinates within `lo .. lo + n` on each axis, sorted.
pub(crate) fn voxelize(tris: &[[Vec3; 3]], size: f32, lo: [i32; 3], n: i32) -> Vec<[i32; 3]> {
    let mut out = std::collections::BTreeSet::new();
    let h = Vec3::splat(0.5 * size);
    for t in tris {
        let tmin = t[0].min(t[1]).min(t[2]) / size;
        let tmax = t[0].max(t[1]).max(t[2]) / size;
        let a: [i32; 3] = std::array::from_fn(|k| (tmin[k].floor() as i32 - 1).max(lo[k]));
        let b: [i32; 3] = std::array::from_fn(|k| (tmax[k].floor() as i32 + 1).min(lo[k] + n - 1));
        for z in a[2]..=b[2] {
            for y in a[1]..=b[1] {
                for x in a[0]..=b[0] {
                    let center = (Vec3::new(x as f32, y as f32, z as f32) + 0.5) * size;
                    if overlaps(center, h, *t) {
                        out.insert([x, y, z]);
                    }
                }
            }
        }
    }
    out.into_iter().collect()
}
