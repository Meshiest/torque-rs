//! A hull's emit strings and polygon lists, the two byte strings TGE's collision reads a convex hull by.
//!
//! - **Emit string** (`InteriorConvex::getFeatures`): one a hull point, the features round it: the points of every face that
//!   holds it (hull point numbers), their edges and the faces (each its point count, its hull plane number and its points as
//!   numbers into the string's own points). Every field is a byte. Equal strings are kept once.
//! - **Polygon list** (`Interior::processHullPolyLists`, followed step for step): the hull's distinct planes and points, the
//!   planes grouped into at most eight groups by how far apart their normals are (each group a mask bit), then each surface on
//!   the hull as its point count, its mask, its plane number and its points (two bytes each).
//!
//! Credit: follows Torque3D 2.0's MIT-licensed `Interior::processHullPolyLists` and `Interior::collisionFanFromSurface`
//! (`Engine/source/interior/interior.cpp` and `interiorCollision.cpp`) and the emit string `InteriorConvex::getFeatures` reads
//! (`interiorCollision.cpp`), each `Copyright (c) 2012 GarageGames, LLC`; the code here is this crate's own.

use std::collections::HashMap;

/// Why a hull cannot be encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HullError {
    /// More than 255 of something a byte counts (points, planes, edges, faces or surfaces of one hull).
    TooMany(&'static str, usize),
}

/// A face of a hull: its plane's number among the hull's planes and its points (hull point numbers, in order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HullFace {
    pub plane: u8,
    pub points: Vec<u8>,
}

fn byte(what: &'static str, n: usize) -> Result<u8, HullError> {
    u8::try_from(n).map_err(|_| HullError::TooMany(what, n))
}

/// The emit string of hull point `v` among `faces`.
pub fn emit_string(v: u8, faces: &[HullFace]) -> Result<Vec<u8>, HullError> {
    let near: Vec<&HullFace> = faces.iter().filter(|f| f.points.contains(&v)).collect();
    let mut points: Vec<u8> = Vec::new();
    for f in &near {
        for &p in &f.points {
            if !points.contains(&p) {
                points.push(p);
            }
        }
    }
    let local = |p: u8| points.iter().position(|&q| q == p).unwrap_or(0) as u8;
    let mut edges: Vec<(u8, u8)> = Vec::new();
    for f in &near {
        for k in 0..f.points.len() {
            let (a, b) = (
                local(f.points[k]),
                local(f.points[(k + 1) % f.points.len()]),
            );
            let e = (a.min(b), a.max(b));
            if !edges.contains(&e) {
                edges.push(e);
            }
        }
    }
    let mut s = vec![byte("emit points", points.len())?];
    s.extend_from_slice(&points);
    s.push(byte("emit edges", edges.len())?);
    for (a, b) in edges {
        s.push(a);
        s.push(b);
    }
    s.push(byte("emit faces", near.len())?);
    for f in near {
        s.push(byte("emit face points", f.points.len())?);
        s.push(f.plane);
        s.extend(f.points.iter().map(|&p| local(p)));
    }
    Ok(s)
}

/// Decodes an emit string into its faces (hull plane number and hull point numbers), as `getFeatures` reads it; None for a
/// string that runs out or names a point it does not list.
pub fn decode_emit_string(s: &[u8]) -> Option<Vec<HullFace>> {
    let mut at = 0usize;
    let mut next = || {
        let b = *s.get(at)?;
        at += 1;
        Some(b)
    };
    let n = next()? as usize;
    let points: Vec<u8> = (0..n).map(|_| next()).collect::<Option<_>>()?;
    let edges = next()? as usize;
    for _ in 0..edges * 2 {
        if next()? as usize >= n {
            return None;
        }
    }
    let faces = next()? as usize;
    let mut out = Vec::with_capacity(faces);
    for _ in 0..faces {
        let count = next()? as usize;
        let plane = next()?;
        let pts = (0..count)
            .map(|_| next().and_then(|k| points.get(k as usize).copied()))
            .collect::<Option<Vec<u8>>>()?;
        out.push(HullFace { plane, points: pts });
    }
    Some(out)
}

/// A surface on a hull as the polygon list takes it: its plane index (with the flip bit) and its collision points (interior
/// point indices, the fan order of `collisionFanFromSurface`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HullSurface {
    pub plane: u16,
    pub points: Vec<u32>,
}

/// One hull's polygon list: its planes, its points and its string.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PolyList {
    pub planes: Vec<u16>,
    pub points: Vec<u32>,
    pub string: Vec<u8>,
}

/// The polygon list of a hull's surfaces; `normal` gives a plane index's normal (the flip applied).
pub fn poly_list(
    surfaces: &[HullSurface],
    normal: impl Fn(u16) -> [f32; 3],
) -> Result<PolyList, HullError> {
    let mut planes: Vec<u16> = Vec::new();
    let mut points: Vec<u32> = Vec::new();
    for s in surfaces {
        if !planes.contains(&s.plane) {
            planes.push(s.plane);
        }
        for &p in &s.points {
            if !points.contains(&p) {
                points.push(p);
            }
        }
    }
    byte("hull planes", planes.len())?;
    byte("hull surfaces", surfaces.len())?;
    if points.len() > 0xffff {
        return Err(HullError::TooMany("hull points", points.len()));
    }
    let point_of: HashMap<u32, usize> = points.iter().enumerate().map(|(i, &p)| (p, i)).collect();
    // The plane groups: merged two at a time, the pair whose closest normals are farthest apart first, down to eight.
    let mut groups: Vec<Vec<u16>> = planes.iter().map(|&p| vec![p]).collect();
    while groups.len() > 8 {
        let mut best = (2.0f32, 0usize, 0usize);
        for j in 0..groups.len() {
            for k in j + 1..groups.len() {
                let mut max = -2.0f32;
                for &a in &groups[j] {
                    for &b in &groups[k] {
                        let (na, nb) = (normal(a), normal(b));
                        max = max.max(na[0] * nb[0] + na[1] * nb[1] + na[2] * nb[2]);
                    }
                }
                if max < best.0 {
                    best = (max, j, k);
                }
            }
        }
        let (_, j, k) = best;
        let mut from = groups.remove(k);
        while let Some(p) = from.pop() {
            groups[j].push(p);
        }
    }
    let mask_of = |plane: u16| -> u8 {
        groups
            .iter()
            .position(|g| g.contains(&plane))
            .map_or(0, |i| 1 << i)
    };
    let surface_masks: Vec<u8> = surfaces.iter().map(|s| mask_of(s.plane)).collect();
    let mut point_masks = vec![0u8; points.len()];
    for (s, &m) in surfaces.iter().zip(&surface_masks) {
        for &p in &s.points {
            point_masks[point_of[&p]] |= m;
        }
    }
    let mut string = vec![planes.len() as u8];
    string.extend(planes.iter().map(|&p| mask_of(p)));
    string.push((points.len() >> 8) as u8);
    string.push(points.len() as u8);
    string.extend_from_slice(&point_masks);
    string.push(surfaces.len() as u8);
    for (s, &m) in surfaces.iter().zip(&surface_masks) {
        string.push(byte("surface points", s.points.len())?);
        string.push(m);
        string.push(planes.iter().position(|&p| p == s.plane).unwrap_or(0) as u8);
        for &p in &s.points {
            let k = point_of[&p];
            string.push((k >> 8) as u8);
            string.push(k as u8);
        }
    }
    Ok(PolyList {
        planes,
        points,
        string,
    })
}

/// The polygon list's surfaces as `InteriorConvex::getPolyList` reads them back: each surface's plane index and points.
pub fn decode_poly_list(list: &PolyList) -> Option<Vec<HullSurface>> {
    let s = &list.string;
    let mut at = 0usize;
    let mut next = || {
        let b = *s.get(at)?;
        at += 1;
        Some(b)
    };
    let planes = next()? as usize;
    for _ in 0..planes {
        next()?;
    }
    let points = (usize::from(next()?) << 8) | usize::from(next()?);
    for _ in 0..points {
        next()?;
    }
    let surfaces = next()? as usize;
    let mut out = Vec::with_capacity(surfaces);
    for _ in 0..surfaces {
        let n = next()? as usize;
        next()?;
        let plane = *list.planes.get(next()? as usize)?;
        let pts = (0..n)
            .map(|_| {
                let k = (usize::from(next()?) << 8) | usize::from(next()?);
                list.points.get(k).copied()
            })
            .collect::<Option<Vec<u32>>>()?;
        out.push(HullSurface { plane, points: pts });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_faces() -> Vec<HullFace> {
        // A box's eight points (bit k of the number is the k axis's high side) and six faces.
        [
            [0, 2, 6, 4],
            [1, 5, 7, 3],
            [0, 4, 5, 1],
            [2, 3, 7, 6],
            [0, 1, 3, 2],
            [4, 6, 7, 5],
        ]
        .iter()
        .enumerate()
        .map(|(i, f)| HullFace {
            plane: i as u8,
            points: f.to_vec(),
        })
        .collect()
    }

    #[test]
    fn a_box_corner_emits_its_three_faces_and_they_decode_back() {
        let faces = box_faces();
        let s = emit_string(0, &faces).unwrap();
        // Seven points, nine edges, three faces of four.
        assert_eq!(s[0], 7);
        assert_eq!(s[8], 9);
        let decoded = decode_emit_string(&s).unwrap();
        assert_eq!(decoded.len(), 3);
        for f in &decoded {
            assert!(faces.contains(f));
        }
        let mut all: Vec<HullFace> = (0..8)
            .flat_map(|v| decode_emit_string(&emit_string(v, &faces).unwrap()).unwrap())
            .collect();
        all.sort_by_key(|f| f.plane);
        all.dedup();
        assert_eq!(all, faces);
    }

    #[test]
    fn a_poly_list_reads_back_and_groups_at_most_eight_masks() {
        let normals: Vec<[f32; 3]> = (0..12)
            .map(|i| {
                let a = i as f32 * 0.5;
                [a.cos(), a.sin(), 0.0]
            })
            .collect();
        let surfaces: Vec<HullSurface> = (0..12u16)
            .map(|i| HullSurface {
                plane: i,
                points: vec![u32::from(i), u32::from(i) + 1, 100],
            })
            .collect();
        let list = poly_list(&surfaces, |p| normals[p as usize]).unwrap();
        assert_eq!(decode_poly_list(&list).unwrap(), surfaces);
        let masks = &list.string[1..13];
        assert!(masks.iter().all(|m| m.count_ones() == 1));
        let mut distinct = masks.to_vec();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), 8);
    }
}
