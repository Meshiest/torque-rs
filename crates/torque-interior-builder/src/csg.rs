//! Hidden faces removed: each brush face cut by every other brush it meets, the parts inside one left out.
//!
//! A part on another brush's face counts as inside it when the two faces look at each other (two brushes touching) and, for
//! two faces of one plane looking the same way (two brushes with a common top), inside the later brush only: the earlier keeps
//! it, so the surface is drawn once.

use crate::brush::Brush;
use crate::geom::{self, Plane, V3};

/// A visible part of a face.
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    pub brush: usize,
    pub face: usize,
    /// Clockwise seen from the front.
    pub poly: Vec<V3>,
}

/// The parts of `poly` (on `plane`, a face of brush `i`) outside brush `j`.
fn outside(poly: Vec<V3>, plane: &Plane, i: usize, j: usize, other: &Brush) -> Vec<Vec<V3>> {
    let mut out = Vec::new();
    let mut rest = poly;
    for f in &other.faces {
        let q = &f.plane;
        if q.same(plane) {
            if i < j {
                out.push(rest);
                return out;
            }
            continue;
        }
        if q.same(&plane.flipped()) {
            continue;
        }
        match geom::side(&rest, q) {
            geom::Side::Front => {
                out.push(rest);
                return out;
            }
            geom::Side::Back | geom::Side::On => {}
            geom::Side::Both => {
                let (front, back) = geom::split(&rest, q);
                if !front.is_empty() {
                    out.push(front);
                }
                rest = back;
                if rest.is_empty() {
                    return out;
                }
            }
        }
    }
    out
}

/// The visible parts of every brush's faces (`polys[b][f]`: brush `b`'s face `f`'s polygon), in brush and face order; `Err`
/// with the count reached as soon as the parts (those kept and those still being cut) would pass `max`.
pub fn visible(
    brushes: &[Brush],
    polys: &[Vec<Vec<V3>>],
    boxes: &[(V3, V3)],
    max: usize,
) -> Result<Vec<Fragment>, usize> {
    let mut out = Vec::new();
    for (i, b) in brushes.iter().enumerate() {
        for (fi, face) in b.faces.iter().enumerate() {
            let poly = &polys[i][fi];
            if poly.is_empty() {
                continue;
            }
            let pbox = geom::bounds(poly.iter().copied());
            let mut parts = vec![poly.clone()];
            for (j, other) in brushes.iter().enumerate() {
                if j == i || !geom::boxes_meet(&pbox, &boxes[j], geom::EPS) {
                    continue;
                }
                parts = parts
                    .into_iter()
                    .flat_map(|p| outside(p, &face.plane, i, j, other))
                    .collect();
                if out.len() + parts.len() > max {
                    return Err(out.len() + parts.len());
                }
                if parts.is_empty() {
                    break;
                }
            }
            for mut p in parts {
                geom::dedup(&mut p);
                if p.len() >= 3 && geom::area(&p) > geom::EPS * geom::EPS {
                    out.push(Fragment {
                        brush: i,
                        face: fi,
                        poly: p,
                    });
                }
            }
        }
    }
    Ok(out)
}
