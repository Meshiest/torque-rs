//! A box query against a box: the second box's faces are fed into a clipped polygon list whose clip planes are the
//! query box, so what remains is the part of the faces inside the query. This is how a mover finds what it touches.
//!
//! `cargo run -p torque --example collide`

use torque::collide::math::{Box3, IDENTITY, Matrix};
use torque::collide::poly::{ClippedPolyList, PolyList, Polyhedron};

fn translation(x: f32, y: f32, z: f32) -> Matrix {
    let mut m = IDENTITY;
    m[3] = x;
    m[7] = y;
    m[11] = z;
    m
}

fn main() {
    // The query region: a unit box around the origin, as six planes.
    let mut region = Polyhedron::default();
    region.build_box(&IDENTITY, &Box3::new([-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]));

    for offset in [0.25_f32, 0.75, 1.5] {
        let mut list = ClippedPolyList::new();
        list.planes.clone_from(&region.planes);
        // The other box, moved along +x; the list takes its faces in world space.
        list.set_transform(&translation(offset, 0.0, 0.0), [1.0, 1.0, 1.0]);
        list.add_box(&Box3::new([-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]));

        // The clipped polygons' vertices (the list also keeps the unclipped points it was fed).
        let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
        for poly in &list.polys {
            for &i in list.poly_indices(poly) {
                let x = list.verts[i as usize].point[0];
                min_x = min_x.min(x);
                max_x = max_x.max(x);
            }
        }
        if list.polys.is_empty() {
            println!("box at x = {offset}: no contact");
        } else {
            println!(
                "box at x = {offset}: {} faces inside the region, spanning x {min_x:.2}..{max_x:.2}",
                list.polys.len()
            );
        }
    }
}
