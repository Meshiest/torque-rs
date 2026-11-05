//! A Quake or Valve 220 `.map` (TrenchBroom's, J.A.C.K.'s and Hammer's exports): entities of `"key" "value"` lines and
//! brushes of face lines, each face three points on its plane, a texture and its alignment.
//!
//! - **Planes.** The points `a b c` give the plane facing along `(a - b) x (c - b)` (out of the brush), in map units; the
//!   interior is in [`MapOptions::units_per_meter`] map units a unit (32 by default, this crate's choice: a 64-unit wall
//!   texture over two units).
//! - **Texture planes.** A Valve 220 face names its axes: `u = (p . U) / su + ou` texels. A Quake face is textured by the
//!   format's projection: onto the world plane across the axis its normal points most along, then turned by its rotation,
//!   scaled and shifted ([`quake_axes`]). Texels become repeats by the texture's size ([`MapOptions::texture_size`];
//!   [`DEFAULT_TEXTURE_SIZE`] for one not known, with a warning).
//! - **Entities.** `worldspawn`'s brushes, and those of `func_group` and `func_detail`, are the interior. A brush entity of
//!   class `water`, `trigger` or `physical_zone` (or `WaterBlock`, `Trigger`, `PhysicalZone`) is a mission object over its
//!   brushes' box; `info_player_start` and `info_player_deathmatch` are `SpawnSphere`s; another point entity whose class is
//!   one of [`POINT_CLASSES`] is passed through; the rest are left out with a warning. Keys other than `classname`, `origin`,
//!   `angle` and `targetname` (the object's name) are written as the object's fields.
//! - **Editor textures.** `clip`, `caulk` and `skip` (any case, any folder) are null faces like TGE's `NULL`; a brush all of
//!   `ORIGIN`, `TRIGGER` or `PORTAL` is left out (zones and portals come later) with a warning.
//!
//! The file is untrusted: its size, its entities and their keys, brushes, faces, names, numbers and each brush's size are
//! capped ([`MAX_MAP_BYTES`], [`MAX_ENTITIES`], [`MAX_KEYS`], [`MAX_BRUSHES`], [`MAX_FACES`], [`MAX_NAME`], [`MAX_VALUE`],
//! [`MAX_TEXTURES`], [`MAX_COORD`], [`MAX_BRUSH_SIZE`]) before anything is built, and every error names its line.

use crate::brush::{Brush, Face, TexGen, main_axis};
use std::collections::HashMap;

use crate::geom::{Plane, V3};
use crate::mission::{MissionObject, Shape};

/// The largest `.map` read.
pub const MAX_MAP_BYTES: usize = 64 << 20;
/// The most brushes of a file (the most one interior is built of).
pub const MAX_BRUSHES: usize = crate::build::MAX_BRUSHES;
/// The most keys of an entity.
pub const MAX_KEYS: usize = 256;
/// The longest key or texture name (a `.dif` or a mission names it in at most 255 bytes).
pub const MAX_NAME: usize = 255;
/// The most distinct texture names of a file (a `.dif`'s material numbers are 16-bit).
pub const MAX_TEXTURES: usize = 0xffff;
/// The longest key value.
pub const MAX_VALUE: usize = 4_096;
/// The largest side of a brush's box (map units; 2,048 units of the interior at the default scale).
pub const MAX_BRUSH_SIZE: f64 = 65_536.0;
/// The most faces of a brush.
pub const MAX_FACES: usize = 128;
/// The most entities of a file.
pub const MAX_ENTITIES: usize = 65_535;
/// The largest coordinate taken (map units).
pub const MAX_COORD: f64 = 1.0e6;
/// The texture size taken for a texture not found.
pub const DEFAULT_TEXTURE_SIZE: (u32, u32) = (256, 256);

/// The point entity classes passed through as they are.
pub const POINT_CLASSES: &[&str] = &[
    "SpawnSphere",
    "Item",
    "StaticShape",
    "TSStatic",
    "AudioEmitter",
    "ParticleEmitterNode",
    "Marker",
    "MissionMarker",
    "WayPoint",
    "fxLight",
];

/// How a `.map` is read.
pub struct MapOptions<'a> {
    /// Map units a unit of the interior.
    pub units_per_meter: f64,
    /// A texture's size in texels by its name, when known.
    pub texture_size: &'a dyn Fn(&str) -> Option<(u32, u32)>,
}

impl Default for MapOptions<'_> {
    fn default() -> Self {
        MapOptions {
            units_per_meter: 32.0,
            texture_size: &|_| None,
        }
    }
}

/// What a `.map` holds.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MapFile {
    /// The interior's brushes, in units.
    pub brushes: Vec<Brush>,
    /// The objects that come with it.
    pub objects: Vec<MissionObject>,
    pub warnings: Vec<String>,
}

/// A refused `.map`: the line and what was wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapError {
    pub line: usize,
    pub what: String,
}

impl std::fmt::Display for MapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.what)
    }
}

impl std::error::Error for MapError {}

fn err(line: usize, what: impl Into<String>) -> MapError {
    MapError {
        line,
        what: what.into(),
    }
}

/// A face as the file gives it, before its texture is sized.
struct RawFace {
    plane: Plane,
    texture: String,
    /// Texels: `u . p + u[3]` with `p` in map units.
    u: [f64; 4],
    v: [f64; 4],
}

#[derive(Default)]
struct Entity {
    keys: Vec<(String, String)>,
    brushes: Vec<(usize, Vec<RawFace>)>,
}

impl Entity {
    fn key(&self, k: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
}

fn number(line: usize, s: &str) -> Result<f64, MapError> {
    let v: f64 = s
        .parse()
        .map_err(|_| err(line, format!("{s:?} is not a number")))?;
    if !v.is_finite() || v.abs() > MAX_COORD {
        return Err(err(line, format!("{s} is out of range")));
    }
    Ok(v)
}

/// `"key" "value"`.
fn key_value(line: usize, s: &str) -> Result<(String, String), MapError> {
    let parts: Vec<&str> = s.split('"').collect();
    if parts.len() != 5 || !parts[0].trim().is_empty() || !parts[4].trim().is_empty() {
        return Err(err(line, "expected \"key\" \"value\""));
    }
    if parts[1].len() > MAX_NAME || parts[3].len() > MAX_VALUE {
        return Err(err(line, "a key or a value is too long"));
    }
    Ok((parts[1].to_string(), parts[3].to_string()))
}

/// A Quake face's texture axes, the format's projection: the world plane across the axis the normal points most along (z
/// before x before y where two are equal), `u` along the first of that plane's two world axes and `v` down the second (an
/// image's rows run down), both turned by `degrees` within the plane (from the first axis toward the second).
pub fn quake_axes(n: V3, degrees: f64) -> (V3, V3) {
    let (a, b) = match main_axis(n) {
        2 => (0, 1),
        0 => (1, 2),
        _ => (0, 2),
    };
    // Quarter turns exactly.
    let (sin, cos) = match degrees {
        0.0 => (0.0, 1.0),
        90.0 => (1.0, 0.0),
        180.0 => (0.0, -1.0),
        270.0 => (-1.0, 0.0),
        d => d.to_radians().sin_cos(),
    };
    let mut u = [0.0; 3];
    let mut v = [0.0; 3];
    u[a] = cos;
    u[b] = sin;
    v[a] = sin;
    v[b] = -cos;
    (u, v)
}

/// A face line: `( x y z ) ( x y z ) ( x y z ) TEXTURE` then `xoff yoff rot xs ys` (Quake) or `[ ux uy uz uo ] [ vx vy vz vo ]
/// rot xs ys` (Valve 220); anything after is ignored.
fn face(line: usize, s: &str) -> Result<RawFace, MapError> {
    let mut rest = s;
    let mut pts = [[0.0; 3]; 3];
    for p in &mut pts {
        let open = rest
            .find('(')
            .ok_or_else(|| err(line, "a face needs three points"))?;
        let close = rest[open..]
            .find(')')
            .ok_or_else(|| err(line, "a point is not closed"))?
            + open;
        let nums: Vec<&str> = rest[open + 1..close].split_whitespace().collect();
        if nums.len() != 3 {
            return Err(err(line, "a point needs three numbers"));
        }
        for k in 0..3 {
            p[k] = number(line, nums[k])?;
        }
        rest = &rest[close + 1..];
    }
    let rest = rest.trim_start();
    let (texture, tail) = rest
        .split_once(char::is_whitespace)
        .ok_or_else(|| err(line, "a face needs a texture and its alignment"))?;
    if texture.len() > MAX_NAME {
        return Err(err(line, "the texture name is too long"));
    }
    let plane = Plane::from_points(pts[0], pts[1], pts[2])
        .ok_or_else(|| err(line, "the three points are on one line"))?;
    let valve = tail.contains('[');
    let nums: Vec<f64> = tail
        .replace(['[', ']'], " ")
        .split_whitespace()
        .take(if valve { 11 } else { 5 })
        .map(|t| number(line, t))
        .collect::<Result<_, _>>()?;
    let need = if valve { 11 } else { 5 };
    if nums.len() < need {
        return Err(err(line, "the texture alignment is short"));
    }
    let scale = |s: f64| if s == 0.0 { 1.0 } else { s };
    let (u, v) = if valve {
        let (su, sv) = (scale(nums[9]), scale(nums[10]));
        (
            [nums[0] / su, nums[1] / su, nums[2] / su, nums[3]],
            [nums[4] / sv, nums[5] / sv, nums[6] / sv, nums[7]],
        )
    } else {
        let (shift, rot, s) = (
            [nums[0], nums[1]],
            nums[2],
            [scale(nums[3]), scale(nums[4])],
        );
        let (a, b) = quake_axes(plane.n, rot);
        (
            [a[0] / s[0], a[1] / s[0], a[2] / s[0], shift[0]],
            [b[0] / s[1], b[1] / s[1], b[2] / s[1], shift[1]],
        )
    };
    Ok(RawFace {
        plane,
        texture: texture.to_string(),
        u,
        v,
    })
}

/// The material a texture name gives: its last path part, the editor's invisible solids as `NULL`.
fn material(texture: &str) -> String {
    let name = texture.rsplit(['/', '\\']).next().unwrap_or(texture);
    let lower = name.to_ascii_lowercase();
    if ["clip", "caulk", "skip", "playerclip"].contains(&lower.as_str()) {
        "NULL".to_string()
    } else {
        name.to_string()
    }
}

/// Reads a `.map` (module docs).
pub fn parse(text: &str, options: &MapOptions) -> Result<MapFile, MapError> {
    if text.len() > MAX_MAP_BYTES {
        return Err(err(0, format!("{} bytes is over the cap", text.len())));
    }
    let k = options.units_per_meter;
    if !(k.is_finite() && k > 0.0) {
        return Err(err(0, "the scale is not a positive number"));
    }
    let mut entities: Vec<Entity> = Vec::new();
    let mut depth = 0;
    let mut brushes = 0usize;
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let s = raw.trim();
        if s.is_empty() || s.starts_with("//") {
            continue;
        }
        match (depth, s) {
            (0, "{") => {
                if entities.len() >= MAX_ENTITIES {
                    return Err(err(line, "too many entities"));
                }
                entities.push(Entity::default());
                depth = 1;
            }
            (1, "{") => {
                brushes += 1;
                if brushes > MAX_BRUSHES {
                    return Err(err(line, "too many brushes"));
                }
                if let Some(e) = entities.last_mut() {
                    e.brushes.push((line, Vec::new()));
                }
                depth = 2;
            }
            (1 | 2, "}") => depth -= 1,
            (1, _) if s.starts_with('"') => {
                let kv = key_value(line, s)?;
                if let Some(e) = entities.last_mut() {
                    if e.keys.len() >= MAX_KEYS {
                        return Err(err(line, "too many keys in an entity"));
                    }
                    e.keys.push(kv);
                }
            }
            (2, _) if s.starts_with('(') => {
                let f = face(line, s)?;
                if let Some((_, b)) = entities.last_mut().and_then(|e| e.brushes.last_mut()) {
                    if b.len() >= MAX_FACES {
                        return Err(err(line, "too many faces in a brush"));
                    }
                    b.push(f);
                }
            }
            (2, _) => {
                return Err(err(
                    line,
                    format!("{s:?}: only plane brushes are read (no patches or brush primitives)"),
                ));
            }
            _ => return Err(err(line, format!("unexpected {s:?}"))),
        }
    }
    if depth != 0 {
        return Err(err(text.lines().count(), "the file ends inside a block"));
    }
    let mut out = MapFile::default();
    let mut sizes: HashMap<String, (u32, u32)> = HashMap::new();
    for e in &entities {
        let class = e.key("classname").unwrap_or("").to_string();
        let lower = class.to_ascii_lowercase();
        let volume_class = match lower.as_str() {
            "water" | "waterblock" => Some("WaterBlock"),
            "trigger" => Some("Trigger"),
            "physical_zone" | "physicalzone" => Some("PhysicalZone"),
            _ => None,
        };
        let fields: Vec<(String, String)> = e
            .keys
            .iter()
            .filter(|(n, _)| {
                !["classname", "origin", "angle", "targetname"]
                    .iter()
                    .any(|x| n.eq_ignore_ascii_case(x))
            })
            .cloned()
            .collect();
        let name = e.key("targetname").map(str::to_string);
        if let Some(class) = volume_class {
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for (line, b) in &e.brushes {
                let brush = make_brush(*line, b, options, &mut sizes, &mut out.warnings)?;
                let (a, z) = brush
                    .bounds()
                    .map_err(|e| err(*line, format!("the brush bounds no solid: {e:?}")))?;
                for k in 0..3 {
                    lo[k] = lo[k].min(a[k]);
                    hi[k] = hi[k].max(z[k]);
                }
            }
            if e.brushes.is_empty() {
                out.warnings
                    .push(format!("a {class} entity has no brushes: left out"));
                continue;
            }
            out.objects.push(MissionObject {
                class: class.to_string(),
                name,
                shape: Shape::Volume(lo, hi),
                yaw: 0.0,
                fields,
            });
            continue;
        }
        if e.brushes.is_empty() {
            let point_class = match lower.as_str() {
                "info_player_start" | "info_player_deathmatch" => Some("SpawnSphere".to_string()),
                _ => POINT_CLASSES
                    .iter()
                    .find(|c| c.eq_ignore_ascii_case(&class))
                    .map(|c| c.to_string()),
            };
            let Some(point_class) = point_class else {
                if !class.is_empty() && lower != "worldspawn" {
                    out.warnings.push(format!(
                        "point entity {class:?} has no mission class: left out"
                    ));
                }
                continue;
            };
            let origin = match e.key("origin") {
                Some(o) => {
                    let v: Vec<f64> = o
                        .split_whitespace()
                        .map(|t| number(0, t))
                        .collect::<Result<_, _>>()?;
                    if v.len() != 3 {
                        return Err(err(
                            0,
                            format!("{class}'s origin {o:?} is not three numbers"),
                        ));
                    }
                    [v[0] / k, v[1] / k, v[2] / k]
                }
                None => [0.0; 3],
            };
            // A `.map` angle is a yaw from +x; Torque's objects face +y.
            let yaw = match e.key("angle") {
                Some(a) => number(0, a.trim())? - 90.0,
                None => 0.0,
            };
            out.objects.push(MissionObject {
                class: point_class,
                name,
                shape: Shape::Point(origin),
                yaw,
                fields,
            });
            continue;
        }
        if !["worldspawn", "func_group", "func_detail", ""].contains(&lower.as_str()) {
            out.warnings
                .push(format!("brush entity {class:?} is built into the interior"));
        }
        for (line, b) in &e.brushes {
            let all = |m: &str| {
                b.iter()
                    .all(|f| material(&f.texture).eq_ignore_ascii_case(m))
            };
            if let Some(special) = ["ORIGIN", "TRIGGER", "PORTAL"].into_iter().find(|m| all(m)) {
                out.warnings
                    .push(format!("line {line}: a brush all {special} is left out"));
                continue;
            }
            out.brushes.push(make_brush(
                *line,
                b,
                options,
                &mut sizes,
                &mut out.warnings,
            )?);
        }
    }
    Ok(out)
}

fn make_brush(
    line: usize,
    faces: &[RawFace],
    options: &MapOptions,
    sizes: &mut HashMap<String, (u32, u32)>,
    warnings: &mut Vec<String>,
) -> Result<Brush, MapError> {
    let k = options.units_per_meter;
    let mut out = Vec::with_capacity(faces.len());
    for f in faces {
        let mat = material(&f.texture);
        let (w, h) = match sizes.get(&f.texture) {
            Some(&s) => s,
            None => {
                if sizes.len() >= MAX_TEXTURES {
                    return Err(err(line, format!("more than {MAX_TEXTURES} textures")));
                }
                let s = (options.texture_size)(&f.texture)
                    .filter(|&(w, h)| w > 0 && h > 0)
                    .unwrap_or_else(|| {
                        if !crate::build::is_null_material(&mat) {
                            warnings.push(format!(
                                "texture {:?} not found: taken as {} x {}",
                                f.texture, DEFAULT_TEXTURE_SIZE.0, DEFAULT_TEXTURE_SIZE.1
                            ));
                        }
                        DEFAULT_TEXTURE_SIZE
                    });
                sizes.insert(f.texture.clone(), s);
                s
            }
        };
        let (w, h) = (f64::from(w), f64::from(h));
        // `p` in units is `k p` in map units.
        let texgen = TexGen {
            u: [f.u[0] * k / w, f.u[1] * k / w, f.u[2] * k / w, f.u[3] / w],
            v: [f.v[0] * k / h, f.v[1] * k / h, f.v[2] * k / h, f.v[3] / h],
        };
        out.push(Face {
            plane: Plane {
                n: f.plane.n,
                d: f.plane.d / k,
            },
            material: mat,
            texgen,
        });
    }
    let brush = Brush { faces: out };
    let (lo, hi) = brush
        .bounds()
        .map_err(|e| err(line, format!("the brush bounds no solid: {e:?}")))?;
    let side = (0..3).map(|i| (hi[i] - lo[i]) * k).fold(0.0, f64::max);
    if side > MAX_BRUSH_SIZE {
        return Err(err(
            line,
            format!("the brush is {side} map units across, over the {MAX_BRUSH_SIZE} taken"),
        ));
    }
    Ok(brush)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE: &str = r#"// A test map.
{
"classname" "worldspawn"
{
( -64 -64 -16 ) ( -64 -63 -16 ) ( -64 -64 -15 ) rock 0 0 0 1 1
( -64 -64 -16 ) ( -64 -64 -15 ) ( -63 -64 -16 ) rock 0 0 0 1 1
( -64 -64 -16 ) ( -63 -64 -16 ) ( -64 -63 -16 ) rock 0 0 0 1 1
( 64 64 16 ) ( 64 65 16 ) ( 65 64 16 ) rock 0 0 0 1 1
( 64 64 16 ) ( 65 64 16 ) ( 64 64 17 ) rock 0 0 0 1 1
( 64 64 16 ) ( 64 64 17 ) ( 64 65 16 ) rock 0 0 0 1 1
}
}
{
"classname" "info_player_start"
"origin" "32 0 24"
"angle" "90"
}
"#;

    #[test]
    fn a_quake_box_reads_in_meters_with_its_texture_planes() {
        let sizes = |name: &str| (name == "rock").then_some((64, 32));
        let m = parse(
            CUBE,
            &MapOptions {
                units_per_meter: 32.0,
                texture_size: &sizes,
            },
        )
        .unwrap();
        assert_eq!(m.brushes.len(), 1);
        let b = &m.brushes[0];
        let (lo, hi) = b.bounds().unwrap();
        assert_eq!((lo, hi), ([-2.0, -2.0, -0.5], [2.0, 2.0, 0.5]));
        // The floor's texture: u = x (map units) / 64 texels, v = -y / 32.
        let top = b
            .faces
            .iter()
            .find(|f| f.plane.n == [0.0, 0.0, 1.0])
            .unwrap();
        let uv = top.texgen.at([1.0, 2.0, 0.5]);
        assert!((uv[0] - 32.0 / 64.0).abs() < 1e-12 && (uv[1] + 64.0 / 32.0).abs() < 1e-12);
        assert_eq!(m.objects.len(), 1);
        assert_eq!(m.objects[0].class, "SpawnSphere");
        assert_eq!(m.objects[0].shape, Shape::Point([1.0, 0.0, 0.75]));
        assert_eq!(m.objects[0].yaw, 0.0);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    }

    /// A box brush from `lo` to `hi` (map units) as face lines.
    fn box_brush(lo: [i64; 3], hi: [i64; 3]) -> String {
        let [a, b, c] = lo;
        let [x, y, z] = hi;
        let faces = [
            [[a, b, c], [a, b + 1, c], [a, b, c + 1]],
            [[a, b, c], [a, b, c + 1], [a + 1, b, c]],
            [[a, b, c], [a + 1, b, c], [a, b + 1, c]],
            [[x, y, z], [x, y + 1, z], [x + 1, y, z]],
            [[x, y, z], [x + 1, y, z], [x, y, z + 1]],
            [[x, y, z], [x, y, z + 1], [x, y + 1, z]],
        ];
        let mut out = String::from("{\n");
        for f in faces {
            for p in f {
                out.push_str(&format!("( {} {} {} ) ", p[0], p[1], p[2]));
            }
            out.push_str("m 0 0 0 1 1\n");
        }
        out.push_str("}\n");
        out
    }

    #[test]
    fn a_box_brush_reads_as_its_box() {
        let text = format!(
            "{{
\"classname\" \"worldspawn\"
{}}}
",
            box_brush([-32, -64, 0], [32, 64, 16])
        );
        let m = parse(&text, &MapOptions::default()).unwrap();
        assert_eq!(
            m.brushes[0].bounds().unwrap(),
            ([-1.0, -2.0, 0.0], [1.0, 2.0, 0.5])
        );
    }

    #[test]
    fn the_quake_projection_turns_its_axes_within_the_face_plane() {
        assert_eq!(
            quake_axes([0.0, 0.0, 1.0], 0.0),
            ([1.0, 0.0, 0.0], [0.0, -1.0, 0.0])
        );
        assert_eq!(
            quake_axes([1.0, 0.0, 0.0], 90.0),
            ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0])
        );
        assert_eq!(
            quake_axes([0.0, -1.0, 0.0], 0.0),
            ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0])
        );
        // Two axes as near: z before x before y.
        assert_eq!(quake_axes([0.6, 0.0, 0.6], 0.0).0, [1.0, 0.0, 0.0]);
        assert_eq!(quake_axes([0.6, 0.6, 0.0], 0.0).0, [0.0, 1.0, 0.0]);
        let (u, v) = quake_axes([0.0, 0.0, 1.0], 30.0);
        assert!(
            (u[0] - 30f64.to_radians().cos()).abs() < 1e-12
                && (v[1] + 30f64.to_radians().cos()).abs() < 1e-12
        );
    }

    #[test]
    fn too_many_textures_are_refused() {
        // Brushes of 128 faces: a box, then copies of its first face, each face a texture of its own.
        let mut text = String::from("{\n\"classname\" \"worldspawn\"\n");
        let mut names = 0;
        let mut i = 0i64;
        while names <= MAX_TEXTURES {
            let faces: Vec<String> = box_brush([i * 2, 0, 0], [i * 2 + 1, 1, 1])
                .lines()
                .filter(|l| l.starts_with('('))
                .map(str::to_string)
                .collect();
            text.push_str("{\n");
            for k in 0..MAX_FACES {
                let f = &faces[if k < 6 { k } else { 0 }];
                text.push_str(&f.replace(" m ", &format!(" t{names} ")));
                text.push('\n');
                names += 1;
            }
            text.push_str("}\n");
            i += 1;
        }
        text.push_str("}\n");
        let e = parse(&text, &MapOptions::default()).unwrap_err();
        assert!(e.what.contains("textures"), "{e}");
    }

    #[test]
    fn an_enormous_brush_is_refused_before_it_is_built() {
        let text = format!(
            "{{
\"classname\" \"worldspawn\"
{}}}
",
            box_brush([-900_000, -900_000, -900_000], [900_000, 900_000, 900_000])
        );
        let e = parse(&text, &MapOptions::default()).unwrap_err();
        assert!(e.what.contains("map units across"), "{e}");
    }

    #[test]
    fn too_many_brushes_or_keys_are_refused_while_reading() {
        let mut text = String::from(
            "{
\"classname\" \"worldspawn\"
",
        );
        for i in 0..=MAX_BRUSHES as i64 {
            text.push_str(&box_brush([i * 2, 0, 0], [i * 2 + 1, 1, 1]));
        }
        text.push_str(
            "}
",
        );
        let e = parse(&text, &MapOptions::default()).unwrap_err();
        assert!(e.what.contains("too many brushes"), "{e}");
        let mut text = String::from(
            "{
",
        );
        for i in 0..=MAX_KEYS {
            text.push_str(&format!(
                "\"k{i}\" \"v\"
"
            ));
        }
        text.push_str(
            "}
",
        );
        assert!(
            parse(&text, &MapOptions::default())
                .unwrap_err()
                .what
                .contains("too many keys")
        );
        let long = format!(
            "{{
\"k\" \"{}\"
}}
",
            "v".repeat(MAX_VALUE + 1)
        );
        assert!(parse(&long, &MapOptions::default()).is_err());
    }

    #[test]
    fn bad_files_are_refused_with_their_line() {
        let e = parse(
            "{\n\"classname\" \"worldspawn\"\n{\n( 0 0 0 ) ( 1 1 1 ) ( 2 2 2 ) x 0 0 0 1 1\n}\n}\n",
            &MapOptions::default(),
        )
        .unwrap_err();
        assert_eq!(e.line, 4);
        assert!(parse("{\n{\npatchDef2\n}\n}\n", &MapOptions::default()).is_err());
        assert!(parse("{\n", &MapOptions::default()).is_err());
        assert!(
            parse(
                "{\n{\n( 0 0 0 ) ( 0 1 0 ) ( 1e400 0 0 ) x 0 0 0 1 1\n}\n}\n",
                &MapOptions::default()
            )
            .is_err()
        );
    }
}
