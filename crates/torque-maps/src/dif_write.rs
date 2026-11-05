//! The writer of a `.dif`: a [`Dif`] laid out as [`super::parse`] reads it, every version and form the reader takes, so that
//! `parse(write(d))` gives `d` back (the plane normals are deduplicated on the way, and a light direction map the form needs but
//! the interior lacks is written as a blank 2 x 2 PNG, as Torque3D's `Interior::write` does).
//!
//! The layout follows Torque3D 2.0's MIT-licensed `Interior::write`, `writePlaneVector` and `writeLMapTexGen`
//! (`Engine/source/interior/interiorIO.cpp`) and `InteriorResource::write` (`interiorRes.cpp`), each `Copyright (c) 2012
//! GarageGames, LLC`; the code here is this crate's own.

use super::{
    AiNode, BspChild, ConvexHull, Dif, ForceField, GameEntity, Hulls, Interior, Lightmap,
    MATERIAL_LIST_VERSION, MIRROR_KEY, NullSurface, PathFollower, Plane, RESOURCE_VERSION,
    StaticMesh, Surface, TexGen, Trigger, TsMaterials, VehicleCollision,
};
use crate::MapError;
use crate::png::{self, Png};

/// The bytes of `d`; an error names a value its field cannot hold in the interior's version (a count past a byte, an index past
/// 16 bits, a string past 255 bytes, lightmap planes that are not one axis each at a power-of-two scale).
pub fn write(d: &Dif) -> Result<Vec<u8>, MapError> {
    let mut o = Out(Vec::new());
    o.u32(RESOURCE_VERSION);
    match &d.preview {
        Some(p) => {
            o.u8(1);
            o.png(p);
        }
        None => o.u8(0),
    }
    o.u32(d.details.len() as u32);
    for i in &d.details {
        interior(&mut o, i, d.border_pad)?;
    }
    o.u32(d.sub_objects.len() as u32);
    for i in &d.sub_objects {
        interior(&mut o, i, d.border_pad)?;
    }
    o.u32(d.triggers.len() as u32);
    for t in &d.triggers {
        trigger(&mut o, t)?;
    }
    o.u32(d.path_followers.len() as u32);
    for p in &d.path_followers {
        path_follower(&mut o, p)?;
    }
    o.u32(d.force_fields.len() as u32);
    for f in &d.force_fields {
        force_field(&mut o, f)?;
    }
    o.u32(d.ai_nodes.len() as u32);
    for AiNode { name, pos } in &d.ai_nodes {
        o.string(name)?;
        o.f32s(pos);
    }
    match d.details.first().and_then(|i| i.vehicle.as_ref()) {
        Some(v) => {
            o.u32(1);
            vehicle(&mut o, v)?;
        }
        None => o.u32(0),
    }
    if d.game_entities.is_empty() {
        o.u32(0);
    } else {
        o.u32(2);
        o.u32(d.game_entities.len() as u32);
        for GameEntity {
            datablock,
            class,
            pos,
            dict,
        } in &d.game_entities
        {
            o.string(datablock)?;
            o.string(class)?;
            o.f32s(pos);
            o.dict(dict)?;
        }
        o.u32(0);
    }
    Ok(o.0)
}

struct Out(Vec<u8>);

fn fits(what: &'static str, value: u64, cap: u64) -> Result<(), MapError> {
    if value <= cap {
        Ok(())
    } else {
        Err(MapError::Count { what, value, cap })
    }
}

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32s(&mut self, v: &[f32]) {
        v.iter().for_each(|&x| self.f32(x));
    }
    fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }
    /// A `u8` count, or with `wide` a `u32`.
    fn small(&mut self, what: &'static str, v: u32, wide: bool) -> Result<(), MapError> {
        if wide {
            self.u32(v);
        } else {
            fits(what, u64::from(v), 255)?;
            self.u8(v as u8);
        }
        Ok(())
    }
    /// `Stream::writeString`: a length byte and the name's bytes (each character Latin-1, as [`crate::latin1`] read it).
    fn string(&mut self, s: &str) -> Result<(), MapError> {
        let bytes: Vec<u8> = s.chars().map(|c| c as u32 as u8).collect();
        fits("string length", bytes.len() as u64, 255)?;
        if s.chars().any(|c| c as u32 > 255) {
            return Err(MapError::Count {
                what: "string character",
                value: s.chars().map(|c| c as u64).max().unwrap_or(0),
                cap: 255,
            });
        }
        self.u8(bytes.len() as u8);
        self.0.extend_from_slice(&bytes);
        Ok(())
    }
    fn png(&mut self, p: &Png) {
        self.0.extend_from_slice(&p.bytes);
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
    fn u16s(&mut self, v: &[u16]) {
        self.u32(v.len() as u32);
        v.iter().for_each(|&x| self.u16(x));
    }
    fn u32s(&mut self, v: &[u32]) {
        self.u32(v.len() as u32);
        v.iter().for_each(|&x| self.u32(x));
    }
    fn points(&mut self, v: &[[f32; 3]]) {
        self.u32(v.len() as u32);
        v.iter().for_each(|p| self.f32s(p));
    }
    fn dict(&mut self, d: &[(String, String)]) -> Result<(), MapError> {
        self.u32(d.len() as u32);
        for (k, v) in d {
            self.string(k)?;
            self.string(v)?;
        }
        Ok(())
    }
    fn aabb(&mut self, b: &super::Aabb) {
        self.f32s(&b.min);
        self.f32s(&b.max);
    }
}

/// `writePlaneVector`: the distinct normals (bit for bit), then each plane as a normal index and a distance.
fn planes(o: &mut Out, planes: &[Plane]) -> Result<(), MapError> {
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut seen: std::collections::HashMap<[u32; 3], u16> = std::collections::HashMap::new();
    let mut index = Vec::with_capacity(planes.len());
    for p in planes {
        let key = p.normal.map(f32::to_bits);
        let k = match seen.get(&key) {
            Some(&k) => k,
            None => {
                fits("plane normals", normals.len() as u64, u64::from(u16::MAX))?;
                let k = normals.len() as u16;
                normals.push(p.normal);
                seen.insert(key, k);
                k
            }
        };
        index.push(k);
    }
    o.points(&normals);
    o.u32(planes.len() as u32);
    for (p, k) in planes.iter().zip(index) {
        o.u16(k);
        o.f32(p.d);
    }
    Ok(())
}

fn bsp_child(c: BspChild, version: u32) -> Result<u32, MapError> {
    let (leaf, solid, cap) = if version >= 14 {
        (0x8_0000, 0x4_0000, 0x3_ffff)
    } else {
        (0x8000, 0x4000, 0x3fff)
    };
    Ok(match c {
        BspChild::Node(i) => {
            fits("BSP child node", u64::from(i), cap)?;
            i
        }
        BspChild::Solid(i) => {
            fits("BSP solid leaf", u64::from(i), cap)?;
            leaf | solid | i
        }
        BspChild::Empty(z) => {
            fits("BSP empty leaf zone", u64::from(z), cap)?;
            leaf | z
        }
    })
}

/// `writeLMapTexGen`: which axis each plane reads, its scale as a power of two, and the two offsets.
fn lightmap_texgen(o: &mut Out, t: &TexGen) -> Result<(), MapError> {
    let bad = |value| MapError::Count {
        what: "lightmap texgen",
        value,
        cap: 0,
    };
    let axis = |g: &[f32; 4]| (0..3).find(|&k| g[k] != 0.0);
    let (Some(s), Some(t_axis)) = (axis(&t.x), axis(&t.y)) else {
        return Err(bad(0));
    };
    let log = |v: f32| -> Result<u16, MapError> {
        let inv = (1.0 / f64::from(v) + 0.5).floor();
        if !(1.0..=2f64.powi(62)).contains(&inv) || inv.log2().fract() != 0.0 {
            return Err(bad(inv as u64));
        }
        let l = inv.log2() as u16;
        if 0.5f64.powi(i32::from(l)) as f32 != v {
            return Err(bad(u64::from(l)));
        }
        Ok(l)
    };
    let code: u16 = match (s, t_axis) {
        (0, 1) => 0,
        (0, 2) => 1,
        (1, 0) => 2,
        (1, 2) => 3,
        (2, 0) => 4,
        (2, 1) => 5,
        _ => return Err(bad(1)),
    };
    o.u16((code << 13) | (log(t.x[s])? << 6) | log(t.y[t_axis])?);
    o.f32(t.x[3]);
    o.f32(t.y[3]);
    Ok(())
}

fn surface(o: &mut Out, s: &Surface, v: u32, plain: bool) -> Result<(), MapError> {
    let wide = v >= 13;
    o.u32(s.winding_start);
    o.small("surface winding count", s.winding_count, wide)?;
    o.u16(s.plane);
    o.u16(s.texture);
    o.u32(s.texgen);
    o.u8(s.flags);
    o.u32(s.fan_mask);
    lightmap_texgen(o, &s.lightmap_texgen)?;
    o.u16(s.light_count);
    o.u32(s.light_state_start);
    for x in s.map_offset.iter().chain(&s.map_size) {
        o.small("surface lightmap rectangle", *x, wide)?;
    }
    if !plain && (v == 0 || v == 5 || v >= 12) {
        o.u8(0);
    }
    if v == 5 {
        o.u32(0);
    }
    Ok(())
}

fn null_surface(o: &mut Out, s: &NullSurface, wide: bool) -> Result<(), MapError> {
    o.u32(s.winding_start);
    o.u16(s.plane);
    o.u8(s.flags);
    o.small("null surface winding count", s.winding_count, wide)
}

fn hull(o: &mut Out, h: &ConvexHull, with_static: bool) {
    o.u32(h.hull_start);
    o.u16(h.hull_count);
    let (a, b) = (h.bounds.min, h.bounds.max);
    o.f32s(&[a[0], b[0], a[1], b[1], a[2], b[2]]);
    o.u32(h.surface_start);
    o.u16(h.surface_count);
    o.u32(h.plane_start);
    o.u32(h.poly_list_plane_start);
    o.u32(h.poly_list_point_start);
    o.u32(h.poly_list_string_start);
    if with_static {
        o.bool(h.static_mesh);
    }
}

fn hulls(o: &mut Out, h: &Hulls, with_static: bool) {
    o.u32(h.hulls.len() as u32);
    h.hulls.iter().for_each(|x| hull(o, x, with_static));
    o.bytes(&h.emit_strings);
    o.u32s(&h.hull_indices);
    o.u16s(&h.hull_plane_indices);
    o.u32s(&h.hull_emit_string_indices);
    o.u32s(&h.hull_surface_indices);
    o.u16s(&h.poly_list_planes);
    o.u32s(&h.poly_list_points);
    o.bytes(&h.poly_list_strings);
}

fn material_names(o: &mut Out, names: &[String]) -> Result<(), MapError> {
    o.u8(MATERIAL_LIST_VERSION);
    o.u32(names.len() as u32);
    names.iter().try_for_each(|n| o.string(n))
}

fn ts_materials(o: &mut Out, m: &TsMaterials) -> Result<(), MapError> {
    material_names(o, &m.names)?;
    let n = m.names.len();
    let words = |o: &mut Out, v: &[u32]| {
        (0..n).for_each(|k| o.u32(v.get(k).copied().unwrap_or(0)));
    };
    words(o, &m.flags);
    words(o, &m.reflectance);
    words(o, &m.bump);
    words(o, &m.detail);
    words(o, &[]);
    let bits = |v: &[f32]| v.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
    words(o, &bits(&m.detail_scales));
    words(o, &bits(&m.reflection_amounts));
    Ok(())
}

fn static_mesh(o: &mut Out, m: &StaticMesh) -> Result<(), MapError> {
    o.u32(m.primitives.len() as u32);
    for p in &m.primitives {
        o.bool(p.alpha);
        o.u32(p.tex_s);
        o.u32(p.tex_t);
        o.i32(p.diffuse);
        o.i32(p.lightmap);
        o.u32(p.start);
        o.u32(p.count);
        o.f32s(&p.lightmap_x);
        o.f32s(&p.lightmap_y);
        p.lightmap_offset.iter().for_each(|&x| o.i32(x));
        p.lightmap_size.iter().for_each(|&x| o.i32(x));
    }
    o.u16s(&m.indices);
    o.points(&m.verts);
    o.points(&m.normals);
    for uvs in [&m.diffuse_uvs, &m.lightmap_uvs] {
        o.u32(uvs.len() as u32);
        uvs.iter().for_each(|uv| o.f32s(uv));
    }
    match &m.materials {
        Some(t) => {
            o.u8(1);
            ts_materials(o, t)?;
        }
        None => o.u8(0),
    }
    o.u32(m.diffuse_bitmaps.len() as u32);
    for b in &m.diffuse_bitmaps {
        match b {
            Some(p) => {
                o.u8(1);
                o.png(p);
            }
            None => o.u8(0),
        }
    }
    o.bool(m.has_solid);
    o.bool(m.has_translucency);
    o.aabb(&m.bounds);
    o.f32s(&m.transform);
    o.f32s(&m.scale);
    Ok(())
}

/// A blank 2 x 2 PNG: the light direction map a form needs where the interior has none.
fn blank_direction_map() -> Png {
    Png {
        width: 2,
        height: 2,
        bytes: png::encode_rgb(2, 2, &[0; 12]),
    }
}

fn interior(o: &mut Out, i: &Interior, pad: bool) -> Result<(), MapError> {
    let v = i.version;
    let wide = v >= 13;
    o.u32(v);
    o.u32(i.detail_level);
    o.u32(i.min_pixels);
    o.aabb(&i.bounds);
    o.f32s(&i.sphere);
    o.bool(i.has_alarm_state);
    o.u32(i.light_state_entries);
    planes(o, &i.planes)?;
    o.points(&i.points);
    o.bytes(&i.point_visibility);
    o.u32(i.texgens.len() as u32);
    for t in &i.texgens {
        o.f32s(&t.x);
        o.f32s(&t.y);
    }
    o.u32(i.bsp_nodes.len() as u32);
    for n in &i.bsp_nodes {
        o.u16(n.plane);
        let (f, b) = (bsp_child(n.front, v)?, bsp_child(n.back, v)?);
        if v >= 14 {
            o.u32(f);
            o.u32(b);
        } else {
            o.u16(f as u16);
            o.u16(b as u16);
        }
    }
    o.u32(i.bsp_solid_leaves.len() as u32);
    for l in &i.bsp_solid_leaves {
        o.u32(l.surface_start);
        o.u16(l.surface_count);
    }
    material_names(o, &i.materials)?;
    o.u32s(&i.windings);
    o.u32(i.winding_indices.len() as u32);
    for f in &i.winding_indices {
        o.u32(f.start);
        o.u32(f.count);
    }
    if v >= 12 {
        o.u32(i.edges.len() as u32);
        for e in &i.edges {
            e.vertices.iter().chain(&e.faces).for_each(|&x| o.i32(x));
        }
    }
    o.u32(i.zones.len() as u32);
    for z in &i.zones {
        o.u16(z.portal_start);
        o.u16(z.portal_count);
        o.u32(z.surface_start);
        o.u16(z.surface_count);
        if v >= 12 {
            o.u32(z.static_mesh_start);
            o.u32(z.static_mesh_count);
        }
        o.u16(z.flags);
    }
    o.u16s(&i.zone_surfaces);
    if v >= 12 {
        o.u32s(&i.zone_static_meshes);
    }
    o.u16s(&i.zone_portals);
    o.u32(i.portals.len() as u32);
    for p in &i.portals {
        o.u16(p.plane);
        o.u16(p.fan_count);
        o.u32(p.fan_start);
        o.u16(p.zone_front);
        o.u16(p.zone_back);
    }
    o.u32(i.surfaces.len() as u32);
    for s in &i.surfaces {
        surface(o, s, v, i.plain_surfaces)?;
    }
    if v == 5 {
        o.u32(i.edges.len() as u32);
        for e in &i.edges {
            e.vertices.iter().for_each(|&x| o.i32(x));
            o.u32(0);
            o.u32(0);
            e.faces.iter().for_each(|&x| o.i32(x));
        }
        o.u32(0);
        o.u32(0);
    }
    for list in [&i.normal_lightmaps, &i.alarm_lightmaps] {
        o.u32(list.len() as u32);
        for &x in list.iter() {
            o.small("lightmap index", x, wide)?;
        }
    }
    o.u32(i.null_surfaces.len() as u32);
    for s in &i.null_surfaces {
        null_surface(o, s, wide)?;
    }
    let with_direction = !i.plain_surfaces && (v == 0 || v == 5 || v >= 12);
    o.u32(i.lightmaps.len() as u32);
    for Lightmap {
        png,
        direction,
        keep,
    } in &i.lightmaps
    {
        o.png(png);
        if with_direction {
            match direction {
                Some(d) => o.png(d),
                None => o.png(&blank_direction_map()),
            }
        }
        o.bool(*keep);
    }
    o.u32s(&i.solid_leaf_surfaces);
    o.u32(i.animated_lights.len() as u32);
    for a in &i.animated_lights {
        o.u32(a.name);
        o.u32(a.state_start);
        o.u16(a.state_count);
        o.u16(a.flags);
        o.u32(a.duration_ms);
    }
    o.u32(i.light_states.len() as u32);
    for s in &i.light_states {
        s.rgb.iter().for_each(|&c| o.u8(c));
        o.u32(s.active_ms);
        o.u32(s.data_start);
        o.u16(s.data_count);
    }
    o.u32(i.state_data.len() as u32);
    for s in &i.state_data {
        o.u32(s.surface);
        o.u32(s.map);
        o.u16(s.light_state);
    }
    o.u32(i.state_data_buffer.len() as u32);
    o.u32(i.state_data_flags);
    o.0.extend_from_slice(&i.state_data_buffer);
    o.bytes(&i.name_buffer);
    o.u32(i.mirrors.len() as u32);
    for m in &i.mirrors {
        o.u32(MIRROR_KEY);
        o.u32(m.detail_level);
        o.u32(m.zone);
        o.f32(m.alpha);
        o.u32(m.surface_count);
        o.u32(m.surface_start);
        o.f32s(&m.centroid);
    }
    hulls(o, &i.hulls, v >= 12);
    fits(
        "coord bins",
        i.coord_bins.len() as u64,
        super::COORD_BINS as u64,
    )?;
    for k in 0..super::COORD_BINS {
        let [start, count] = i.coord_bins.get(k).copied().unwrap_or([0, 0]);
        o.u32(start);
        o.u32(count);
    }
    o.u16s(&i.coord_bin_indices);
    o.u32(i.coord_bin_mode);
    i.base_ambient.iter().for_each(|&c| o.u8(c));
    i.alarm_ambient.iter().for_each(|&c| o.u8(c));
    if v >= 10 {
        o.u32(i.static_meshes.len() as u32);
        for m in &i.static_meshes {
            static_mesh(o, m)?;
        }
    }
    if v >= 11 {
        o.points(&i.normals);
        o.u32(i.tex_matrices.len() as u32);
        for t in &i.tex_matrices {
            t.iter().for_each(|&x| o.i32(x));
        }
        o.u32s(&i.tex_mat_indices);
    }
    if v < 10 {
        o.u32(0);
    }
    if v < 11 {
        o.u32(0);
        o.u32(0);
    }
    match i.lightmap_border {
        Some(b) => {
            o.u32(1);
            o.u32(b);
            o.u32(0);
        }
        None => {
            o.u32(0);
            if pad {
                o.u32(0);
            }
        }
    }
    Ok(())
}

fn vehicle(o: &mut Out, v: &VehicleCollision) -> Result<(), MapError> {
    o.u32(v.version);
    hulls(o, &v.hulls, false);
    o.u32(v.null_surfaces.len() as u32);
    for s in &v.null_surfaces {
        null_surface(o, s, true)?;
    }
    o.points(&v.points);
    o.u32(v.planes.len() as u32);
    for p in &v.planes {
        o.f32s(&p.normal);
        o.f32(p.d);
    }
    o.u32s(&v.windings);
    o.u32(v.winding_indices.len() as u32);
    for f in &v.winding_indices {
        o.u32(f.start);
        o.u32(f.count);
    }
    Ok(())
}

fn trigger(o: &mut Out, t: &Trigger) -> Result<(), MapError> {
    o.string(&t.name)?;
    o.string(&t.datablock)?;
    o.dict(&t.dict)?;
    o.points(&t.points);
    o.u32(t.planes.len() as u32);
    t.planes.iter().for_each(|p| o.f32s(p));
    o.u32(t.edges.len() as u32);
    t.edges.iter().flatten().for_each(|&x| o.u32(x));
    o.f32s(&t.offset);
    Ok(())
}

fn path_follower(o: &mut Out, p: &PathFollower) -> Result<(), MapError> {
    o.string(&p.name)?;
    o.string(&p.datablock)?;
    o.u32(p.interior);
    o.f32s(&p.offset);
    o.dict(&p.dict)?;
    o.u32s(&p.triggers);
    o.u32(p.way_points.len() as u32);
    for w in &p.way_points {
        o.f32s(&w.pos);
        o.f32s(&w.rot);
        o.u32(w.ms_to_next);
        o.u32(w.smoothing);
    }
    o.u32(p.total_ms);
    Ok(())
}

fn force_field(o: &mut Out, f: &ForceField) -> Result<(), MapError> {
    o.u32(super::FORCE_FIELD_VERSION);
    o.string(&f.name)?;
    o.u32(f.triggers.len() as u32);
    f.triggers.iter().try_for_each(|t| o.string(t))?;
    o.aabb(&f.bounds);
    o.f32s(&f.sphere);
    planes(o, &f.planes)?;
    o.points(&f.points);
    o.u32(f.bsp_nodes.len() as u32);
    f.bsp_nodes.iter().flatten().for_each(|&x| o.u16(x));
    o.u32(f.bsp_solid_leaves.len() as u32);
    for l in &f.bsp_solid_leaves {
        o.u32(l.surface_start);
        o.u16(l.surface_count);
    }
    o.u32s(&f.windings);
    o.u32(f.surfaces.len() as u32);
    for s in &f.surfaces {
        o.u32(s.winding_start);
        o.u8(s.winding_count);
        o.u16(s.plane);
        o.u8(s.flags);
        o.u32(s.fan_mask);
    }
    o.u32s(&f.solid_leaf_surfaces);
    f.color.iter().for_each(|&c| o.u8(c));
    Ok(())
}
