//! The sky as TGE 1.4's `Sky` draws it (`terrain/sky.cc`): the first render image of the frame.
//!
//! - **`Sky::renderObject`**: a quad over the whole viewport in the fog colour (`mRealFogColor`, the fog colour's bytes, times
//!   the fog multiplier of [`Multipliers`]: 1 without one), with blending `ONE, ZERO`: the frame's clear. Then the camera's
//!   position as a translation, textures on, depth writes off, and `Sky::render`.
//! - **`Sky::render`**: the ban heights (no fog volume around the camera: the lower ban at the horizon, 0, the upper at
//!   `OFFSET_HEIGHT`, 60, or the caller's [`SkyProfile::ban_offset`]), the box (`renderSkyBox`), the sun's flare (the caller's, drawn between the box and the
//!   clouds: [`SkyDraws::after_flare`]), the cloud layers, and the fog bans unless the Sky's `noRenderBans`.
//! - **`calcPoints`**: the box's eight corners at `normalize((1, 1, 1)) * 0.95 * visible distance` (the radius as a float:
//!   the double 0.95 as a float is `0.949999988`), the texture coordinates `x * (d - 1) / d + 0.5 / d` with `d` the first face's
//!   width, `mSkyBoxPt` = corner 1, `mSpherePt` = `(x, 0, z)` scaled to length `x`.
//! - **`renderSkyBox`**: `GL_REPLACE` (or, with [`SkyProfile::tinted_box`], `GL_MODULATE` with `glColor3f` of the Sky's colour
//!   multiplier: the Sky's `skyColor` without another multiplier), each face its dml texture (clamped, mipmapped, trilinear). The four
//!   sides are clipped to `z >= lower ban` (`sgUtil_clipToPlane`, which keeps the vertex order 0, 1, then the two cut points), their
//!   lower texture row at `(top z - cut z) / (int)(|top z| + |bottom z|)`; the top face; the bottom only with `renderBottomTexture`.
//! - **`calcBans`** and **`renderBans`**: 17 points around the camera (angles 0 to 2 pi by pi / 8, rotated as
//!   TGE's `EulerF(0, 0, a)` matrix turns `(m, 0, 0)`) at the lower ban height (radius `skyBoxPt.x * sqrt(1 - (h0 / skyBoxPt.z)^2)`)
//!   and at the upper (`h1 / skyBoxPt.x` instead); a strip between them, opaque at the lower ring and `alphaBan[0]` at the upper
//!   (and, with [`SkyProfile::skirt_depth`], an opaque strip on down below the lower ring); the top fan (`alphaBan[1]` at the
//!   top centre); four corner fillers one unit below the lower ban. The colours are the fog bytes times the fog multiplier,
//!   truncated (C's `U8()` conversion); blending `SRC_ALPHA, ONE_MINUS_SRC_ALPHA`, no texture.
//!
//! A caller may draw some of these its own way ([`SkyProfile`]); [`SkyProfile::TGE`] is TGE 1.4's.
//!
//! Not drawn: the fog-volume case (the camera below a fog volume's top: `calcAlphas_Heights`), storms.
//!
//! Credit: follows TGE 1.4's `terrain/sky.cc` and `sceneGraph/windingClipper.cc` (Copyright (C) GarageGames.com, Inc.); the code
//! here is this crate's own.

use crate::source::SceneSource;

use crate::textures::TextureSet;
use crate::{Assets, Blend, Cull, Draw, Filter, Sampler, TexEnv, Vertex};

/// The radius factor of the box (0.95 as a float).
const RADIUS: f64 = 0.949_999_988_079_071;
/// The upper ban's height above the lower when no fog volume is around the camera (`OFFSET_HEIGHT`).
pub const BAN_OFFSET: f32 = 60.0;
/// The ban rings' step (pi / 8 as a float), the loop's bound and the points the strips use.
const BAN_STEP: f64 = 0.392_699_092_626_571_66;
const BAN_POINTS: usize = 17;
/// The corner fillers' points from the lower ring (`x < 5`).
const FILLER_RING_POINTS: usize = 5;
/// The first cloud texture's line in the dml and the most layers (`MAX_NUM_LAYERS`).
const CLOUD_OFFSET: usize = 7;
const MAX_CLOUD_LAYERS: usize = 3;
/// `Cloud::setHeights(h, h - 0.05, 0.05)` (`initSkyData`) and `Cloud::calcAlpha`'s numbers.
const CLOUD_INNER: f32 = 0.05;
const CLOUD_EDGE: f32 = 0.05;
const CLOUD_ALPHA_TOP: f64 = 1.3;
const CLOUD_ALPHA_LOW: f32 = 0.4;
const CLOUD_ALPHA_HIGH: f32 = 0.8;

/// The most material list lines read (TGE allows a handful: six faces, the environment map, three clouds).
const MAX_DML_LINES: usize = 64;

/// Where a caller draws the Sky other than TGE 1.4 does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyProfile {
    /// The upper ban's height above the lower when no fog volume is around the camera.
    pub ban_offset: f32,
    /// The upper ring's limit: at or past it (`h1 / skyBoxPt.x`) the ring closes on the centre. `None`: no limit.
    pub upper_limit: Option<f64>,
    /// An opaque strip in the fog colour from the lower ring down this far. `None`: none.
    pub skirt_depth: Option<f32>,
    /// The box's faces drawn in `GL_MODULATE` with the Sky's colour multiplier, not `GL_REPLACE`.
    pub tinted_box: bool,
    /// A Sky with no material list is drawn in its solid colour only where the world holds a map ([`solid`]).
    pub solid_only_on_map: bool,
}

impl SkyProfile {
    /// TGE 1.4's Sky.
    pub const TGE: SkyProfile = SkyProfile {
        ban_offset: BAN_OFFSET,
        upper_limit: None,
        skirt_depth: None,
        tinted_box: false,
        solid_only_on_map: false,
    };
}

impl Default for SkyProfile {
    fn default() -> SkyProfile {
        SkyProfile::TGE
    }
}

/// What the Sky gives the frame.
pub struct SkyDraws {
    /// The fog-colour quad's colour (the frame's clear), RGB 0..1.
    pub clear: [f32; 3],
    /// The box, in `renderSkyBox`'s order.
    pub boxes: Vec<Draw>,
    /// The cloud layers and the fog bans, drawn after the sun's flare.
    pub after_flare: Vec<Draw>,
}

/// The fog and sky colour multipliers the Sky draws with: without a caller's own (a cycle of the day's colours), `[1; 3]` and
/// the Sky's `skyColor`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Multipliers {
    pub fog: [f32; 3],
    pub sky: [f32; 3],
}

/// The fog colour's bytes as `Sky::setVisibility` keeps them (`mRealFogColor`).
pub fn real_fog(src: &dyn SceneSource) -> [u8; 3] {
    src.sky()
        .and_then(|s| s.main)
        .map_or([255; 3], |m| m.fog_bytes)
}

/// The Sky's `skyColor` (three 8-bit ranged floats), white when none was sent.
pub fn sky_color(src: &dyn SceneSource) -> [f32; 3] {
    src.sky()
        .and_then(|s| s.main)
        .map_or([1.0; 3], |m| m.sky_color)
}

/// Whether the Sky is drawn in its solid colour: TGE 1.x draws a Sky whose `useSkyTextures` is 0 in its `SkySolidColor` and
/// reads none of its material list. A source gives such a Sky no material list and its `SkySolidColor` as the `skyColor`; the
/// box is then drawn untextured in that colour (`GL_MODULATE` of the colour over a white face), with no cloud layers. With
/// [`SkyProfile::solid_only_on_map`] only where the world holds a map ([`SceneSource::has_map`]); a Sky without a material list
/// anywhere else then draws no box.
pub fn solid(src: &dyn SceneSource, profile: &SkyProfile) -> bool {
    src.sky()
        .and_then(|s| s.main)
        .is_some_and(|m| m.material.is_empty())
        && (!profile.solid_only_on_map || src.has_map())
}

/// A box face: its dml texture, the solid colour, or nothing.
#[derive(Clone, Copy)]
enum Face {
    Texture(usize),
    Solid,
    None,
}

/// A byte times a multiplier as the bans' `glColor4ub` takes it: truncated (C's `U8()` conversion).
fn chop(byte: u8, mul: f32) -> u8 {
    (f64::from(byte) * f64::from(mul)).clamp(0.0, 255.0) as u8
}

/// The box's corners and texture coordinates (`calcPoints`).
pub struct SkyBox {
    /// `0.95 *` the visible distance (the clouds' radius too: `Cloud::setRadius`).
    pub radius: f32,
    pub points: [[f32; 3]; 10],
    pub tex: [[f32; 2]; 4],
    pub sky_box_pt: [f32; 3],
    pub sphere_z: f32,
}

impl SkyBox {
    pub fn new(visible: f32, face_width: u32) -> SkyBox {
        let r = (f64::from(visible) * f64::from(RADIUS as f32)) as f32;
        // m_point3F_normalize_f: the vector times len / |v|.
        let factor = (f64::from(r) / f64::from(3.0f32.sqrt())) as f32;
        let (x, y, z) = (factor, factor, factor);
        let mut points = [[0.0f32; 3]; 10];
        points[0] = [-x, -y, z];
        points[4] = points[0];
        points[5] = [-x, -y, -z];
        points[9] = points[5];
        points[1] = [x, -y, z];
        points[6] = [x, -y, -z];
        points[2] = [x, y, z];
        points[7] = [x, y, -z];
        points[3] = [-x, y, z];
        points[8] = [-x, y, -z];
        let dem = face_width as f32;
        let s = (dem - 1.0) / dem;
        let off = (0.5 / f64::from(dem)) as f32;
        let mut tex = [[0.0f32; 2]; 4];
        for yy in 0..2 {
            for xx in 0..2 {
                tex[xx + yy * 2] = [xx as f32 * s + off, yy as f32 * s + off];
            }
        }
        let sky_box_pt = points[1];
        let len = (f64::from(x) * f64::from(x) + f64::from(z) * f64::from(z)).sqrt();
        let sphere_z = (f64::from(z) * (f64::from(sky_box_pt[0]) / len)) as f32;
        SkyBox {
            radius: r,
            points,
            tex,
            sky_box_pt,
            sphere_z,
        }
    }
}

/// The ban rings and corner points (`calcBans`) for ban heights `h`.
pub fn bans(
    b: &SkyBox,
    h: [f32; 2],
    profile: &SkyProfile,
) -> ([[[f32; 3]; BAN_POINTS]; 2], [[f32; 3]; 8]) {
    let mut rings = [[[0.0f32; 3]; BAN_POINTS]; 2];
    let ring_mul = |v: f32| -> f32 {
        match profile.upper_limit {
            Some(limit) if f64::from(v) >= limit => 0.0,
            _ => -(1.0 - v * v).sqrt(),
        }
    };
    let muls = [
        -(1.0 - (h[0] / b.sky_box_pt[2]).powi(2)).sqrt(),
        ring_mul(h[1] / b.sky_box_pt[0]),
    ];
    for (k, ring) in rings.iter_mut().enumerate() {
        let m = b.sky_box_pt[0] * muls[k];
        let mut angle = 0.0f32;
        for p in ring.iter_mut() {
            let (s, c) = angle.sin_cos();
            *p = [c * m, -s * m, h[k]];
            angle += BAN_STEP as f32;
        }
    }
    let p3 = b.points[3];
    let low = h[0] - 1.0;
    let mut corners = [[0.0f32; 3]; 8];
    corners[0] = [p3[0], p3[1], low];
    corners[1] = [p3[0], 0.0, low];
    corners[2..2 + FILLER_RING_POINTS].copy_from_slice(&rings[0][..FILLER_RING_POINTS]);
    corners[7] = [0.0, p3[1], low];
    (rings, corners)
}

fn vtx(eye: [f32; 3], p: [f32; 3], color: [u8; 4], uv: [f32; 2]) -> Vertex {
    Vertex {
        position: [eye[0] + p[0], eye[1] + p[1], eye[2] + p[2]],
        normal: [0.0, 0.0, 1.0],
        color,
        uv,
    }
}

fn plain(label: &str, vertices: Vec<Vertex>, blend: Blend) -> Draw {
    Draw {
        label: label.to_string(),
        vertices,
        lighting: false,
        tex_env: TexEnv::None,
        texture: None,
        blend,
        cull: Cull::None,
        depth_test: false,
        depth_write: false,
        fog: false,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// A triangle strip as triangles.
fn strip(v: &[Vertex]) -> Vec<Vertex> {
    let mut out = Vec::new();
    for i in 2..v.len() {
        out.extend_from_slice(&[v[i - 2], v[i - 1], v[i]]);
    }
    out
}

/// A triangle fan as triangles.
fn fan(v: &[Vertex]) -> Vec<Vertex> {
    let mut out = Vec::new();
    for i in 2..v.len() {
        out.extend_from_slice(&[v[0], v[i - 1], v[i]]);
    }
    out
}

/// The sky box's material list: the dml's lines (names relative to its folder).
fn dml_names(assets: &dyn Assets, path: &str) -> Option<Vec<String>> {
    let bytes = assets.read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut names: Vec<String> = text
        .lines()
        .take(MAX_DML_LINES)
        .map(|l| l.trim().to_string())
        .collect();
    while names.last().is_some_and(String::is_empty) {
        names.pop();
    }
    Some(names)
}

/// The sky's draws for the camera at `eye` with the scene's visible distance, drawn as `profile` says.
#[allow(clippy::too_many_arguments)]
pub fn draws(
    profile: &SkyProfile,
    src: &dyn SceneSource,
    assets: &dyn Assets,
    eye: [f32; 3],
    visible: f32,
    mul: Multipliers,
    textures: &mut TextureSet<'_>,
    notes: &mut Vec<String>,
) -> Option<SkyDraws> {
    let sky = src.sky()?;
    let main = sky.main.as_ref()?;
    let fog = real_fog(src);
    let fog_rgb = [
        chop(fog[0], mul.fog[0]),
        chop(fog[1], mul.fog[1]),
        chop(fog[2], mul.fog[2]),
    ];
    let clear = fog_rgb.map(|c| f32::from(c) / 255.0);
    let mut out = SkyDraws {
        clear,
        boxes: Vec::new(),
        after_flare: Vec::new(),
    };
    let (render_bottom, no_bans) = (main.render_bottom, main.no_fog_bans);
    if main.fog_volumes != 0 {
        notes.push("sky: fog volumes are not drawn around the camera".to_string());
    }
    let material = main.material.replace('\\', "/");
    let folder = material.rsplit_once('/').map_or("", |(d, _)| d).to_string();
    // A Sky drawn in its solid colour ([`solid`]): the box's faces untextured, no clouds.
    let solid = solid(src, profile);
    let names = match dml_names(assets, &material) {
        _ if solid => Vec::new(),
        Some(n) => n,
        None => {
            notes.push(format!("sky: no material list {material}"));
            Vec::new()
        }
    };
    let face = |i: usize, textures: &mut TextureSet<'_>| -> Option<usize> {
        let name = names.get(i).filter(|n| !n.is_empty())?;
        textures.get(&format!("{folder}/{name}"))
    };
    let faces: Vec<Face> = (0..6)
        .map(|i| match face(i, textures) {
            Some(t) => Face::Texture(t),
            None if solid => Face::Solid,
            None => Face::None,
        })
        .collect();
    let face_width = match faces[0] {
        Face::Texture(t) => textures.width(t).unwrap_or(512),
        _ => 512,
    };
    let b = SkyBox::new(visible, face_width);
    // No fog volume around the camera: the bans at the horizon and the profile's offset above it.
    let mut h = [0.0f32, profile.ban_offset];
    if b.sphere_z <= h[0] {
        h = [b.sphere_z, b.sphere_z];
    }
    let alpha_ban = [0.0f32, 0.0f32];
    let sampler = Sampler {
        repeat: false,
        mag: Filter::Linear,
        min: Filter::Linear,
        mip: Filter::Linear,
        mipmaps: true,
    };
    let col = mul
        .sky
        .map(|c| (f64::from(c.clamp(0.0, 1.0)) * 255.0).round() as u8);
    let color = [col[0], col[1], col[2], 255];
    let sides = if render_bottom { 6 } else { 5 };
    let mut index = 0usize;
    for (side, tex) in faces.iter().enumerate().take(sides) {
        if !(h[0] != b.sphere_z || (side == 4 && alpha_ban[1] < 1.0)) {
            continue;
        }
        let texture = match *tex {
            Face::Texture(t) => Some((t, sampler)),
            Face::Solid => None,
            Face::None => {
                if side < 4 {
                    index += 1;
                }
                continue;
            }
        };
        let mut v = Vec::new();
        if side < 4 {
            let p = [
                b.points[index],
                b.points[index + 1],
                b.points[index + 6],
                b.points[index + 5],
            ];
            let mut quad = p.to_vec();
            if !no_bans {
                quad = clip_above(&p, h[0]);
            }
            if quad.len() >= 3 {
                let length = (b.points[index][2].abs() + b.points[index + 5][2].abs()) as i32;
                let per = b.points[index][2] - quad[quad.len() - 1][2];
                let lower = per / length as f32;
                let tc = [
                    b.tex[0],
                    b.tex[1],
                    [b.tex[3][0], lower],
                    [b.tex[2][0], lower],
                ];
                let vs: Vec<Vertex> = quad
                    .iter()
                    .enumerate()
                    .map(|(k, q)| vtx(eye, *q, color, tc[k.min(3)]))
                    .collect();
                v = fan(&vs);
            }
            index += 1;
        } else {
            let (start, step): (isize, isize) = if side == 5 { (5, 1) } else { (3, -1) };
            let at = |k: isize| b.points[(start + k * step) as usize];
            let vs = [
                vtx(eye, at(0), color, b.tex[0]),
                vtx(eye, at(1), color, b.tex[1]),
                vtx(eye, at(2), color, b.tex[3]),
                vtx(eye, at(3), color, b.tex[2]),
            ];
            v = fan(&vs);
        }
        if v.is_empty() {
            continue;
        }
        out.boxes.push(Draw {
            label: format!("sky face {side}"),
            vertices: v,
            lighting: false,
            tex_env: if profile.tinted_box || texture.is_none() {
                TexEnv::Modulate
            } else {
                TexEnv::Replace
            },
            texture,
            blend: Blend::Opaque,
            cull: Cull::None,
            depth_test: false,
            depth_write: false,
            fog: false,
            lights: Vec::new(),
            vertex_key: None,
        });
    }
    // The cloud layers: the dml's lines from the eighth on (`CloudMaterialOffset` 7), at most three.
    let layers = names
        .len()
        .saturating_sub(CLOUD_OFFSET)
        .min(MAX_CLOUD_LAYERS);
    for i in 0..layers {
        let Some(t) = names
            .get(CLOUD_OFFSET + i)
            .filter(|n| !n.is_empty())
            .and_then(|n| textures.get(&format!("{folder}/{n}")))
        else {
            continue;
        };
        let height = main.clouds[i].height;
        if main.clouds[i].speed != 0.0 {
            notes.push(format!(
                "sky: cloud layer {i} moves with the client's clock; drawn at offset 0"
            ));
        }
        out.after_flare.push(cloud_layer(
            eye,
            b.radius,
            height,
            h[0],
            b.sky_box_pt[0],
            t,
            i,
        ));
    }
    if !no_bans {
        let (rings, corners) = bans(&b, h, profile);
        let a_in = (alpha_ban[1] * 255.0) as u8;
        let a_out = (alpha_ban[0] * 255.0) as u8;
        let fc = |a: u8| [fog_rgb[0], fog_rgb[1], fog_rgb[2], a];
        if h[0] < b.sphere_z {
            let mut s = Vec::new();
            for (lower, upper) in rings[0].iter().zip(rings[1].iter()) {
                s.push(vtx(eye, *lower, fc(255), [0.0; 2]));
                s.push(vtx(eye, *upper, fc(a_out), [0.0; 2]));
            }
            out.after_flare
                .push(plain("sky side bans", strip(&s), Blend::Alpha));
            if let Some(depth) = profile.skirt_depth {
                let mut k = Vec::new();
                for p in rings[0].iter() {
                    k.push(vtx(eye, *p, fc(255), [0.0; 2]));
                    k.push(vtx(eye, [p[0], p[1], p[2] - depth], fc(255), [0.0; 2]));
                }
                out.after_flare
                    .push(plain("sky skirt", strip(&k), Blend::Alpha));
            }
        }
        let mut top = vec![vtx(eye, [0.0, 0.0, b.sky_box_pt[2]], fc(a_in), [0.0; 2])];
        top.extend(rings[1].iter().map(|p| vtx(eye, *p, fc(a_out), [0.0; 2])));
        out.after_flare
            .push(plain("sky top ban", fan(&top), Blend::Alpha));
        // The fillers: glRotatef by 0, 90, then 180 and 270 more (the angle grows by 90 each time on the same matrix).
        let mut total = 0.0f32;
        let mut fill = Vec::new();
        for side in 0..4 {
            total += 90.0 * side as f32;
            let (s, c) = total.to_radians().sin_cos();
            let rot = |p: [f32; 3]| [p[0] * c - p[1] * s, p[0] * s + p[1] * c, p[2]];
            let vs: Vec<Vertex> = corners
                .iter()
                .map(|p| vtx(eye, rot(*p), fc(255), [0.0; 2]))
                .collect();
            fill.extend(fan(&vs));
        }
        out.after_flare
            .push(plain("sky fillers", fill, Blend::Alpha));
    }
    Some(out)
}

/// A cloud layer (`Cloud::setPoints`, `Cloud::render`): a 5 x 5 grid over `[-r, r]` (x from west, y from north)
/// at heights `r * (edge, inner, centre)` (the centre point, its eight neighbours, the outer ring; each outer corner then moved onto
/// the plane of its neighbours), texture coordinates `(x, y)` per grid step at offset 0 (the scroll is frozen), each point's alpha
/// `1.3 - |xy| / r` (0 under 0.4, 1 over 0.8), drawn cell by cell (`{0, 5, 6, 1}`) with `glColor4f(1, 1, 1, alpha)`,
/// `GL_MODULATE`, blending `SRC_ALPHA, ONE_MINUS_SRC_ALPHA`, the texture repeated. The cells are clipped by the planes through the
/// lower ban (z >= 0 with no fog volume): no cloud point is below it.
fn cloud_layer(
    eye: [f32; 3],
    r: f32,
    height: f32,
    lower_ban: f32,
    box_x: f32,
    texture: usize,
    layer: usize,
) -> Draw {
    let d = (f64::from(r - -r) * 0.25) as f32;
    let (c, i, e) = (height * r, (height - CLOUD_INNER) * r, CLOUD_EDGE * r);
    #[rustfmt::skip]
    let z = [
        e, e, e, e, e,
        e, i, i, i, e,
        e, i, c, i, e,
        e, i, i, i, e,
        e, e, e, e, e,
    ];
    let mut p = [[0.0f32; 3]; 25];
    for y in 0..5 {
        for x in 0..5 {
            p[y * 5 + x] = [-r + d * x as f32, r - d * y as f32, z[y * 5 + x]];
        }
    }
    let fix = |p: &mut [[f32; 3]; 25], corner: usize, a: usize, b: usize, m: usize| {
        let v = [0, 1, 2].map(|k| (p[b][k] - p[a][k]) * 0.5 + p[a][k] - p[m][k]);
        p[corner] = [0, 1, 2].map(|k| v[k] + v[k] + p[m][k]);
    };
    fix(&mut p, 0, 5, 1, 6);
    fix(&mut p, 4, 9, 3, 8);
    fix(&mut p, 20, 21, 15, 16);
    fix(&mut p, 24, 23, 19, 18);
    let alpha: Vec<f32> = p
        .iter()
        .map(|q| {
            let len = (q[0] * q[0] + q[1] * q[1] + 0.0).sqrt();
            let a = (CLOUD_ALPHA_TOP - f64::from(len) / f64::from(r)) as f32;
            if a < CLOUD_ALPHA_LOW {
                0.0
            } else if a > CLOUD_ALPHA_HIGH {
                1.0
            } else {
                a
            }
        })
        .collect();
    // Sky::render's four planes through the lower ban: `atan2(lower ban, box x)`, normals (+-sin, 0, cos) and (0, +-sin, cos).
    let ang = f64::from(lower_ban).atan2(f64::from(box_x));
    let (s, c) = (ang.sin() as f32, ang.cos() as f32);
    let planes = [[s, 0.0, c], [-s, 0.0, c], [0.0, s, c], [0.0, -s, c]];
    let mut v = Vec::new();
    for row in 0..4 {
        for col in 0..4 {
            let start = row * 5 + col;
            let mut cell: Vec<CloudPoint> = [0, 5, 6, 1]
                .iter()
                .map(|&o| {
                    let k = start + o;
                    CloudPoint {
                        p: p[k],
                        uv: [(k % 5) as f32, (k / 5) as f32],
                        a: alpha[k],
                    }
                })
                .collect();
            for n in &planes {
                cell = clip_cloud(&cell, *n);
            }
            let vs: Vec<Vertex> = cell
                .iter()
                .map(|q| {
                    let a = (f64::from(q.a.clamp(0.0, 1.0)) * 255.0).round() as u8;
                    vtx(eye, q.p, [255, 255, 255, a], q.uv)
                })
                .collect();
            v.extend(fan(&vs));
        }
    }
    Draw {
        label: format!("sky cloud layer {layer}"),
        vertices: v,
        lighting: false,
        tex_env: TexEnv::Modulate,
        texture: Some((
            texture,
            Sampler {
                repeat: true,
                mag: Filter::Linear,
                min: Filter::Linear,
                mip: Filter::Linear,
                mipmaps: true,
            },
        )),
        blend: Blend::Alpha,
        cull: Cull::None,
        depth_test: false,
        depth_write: false,
        fog: false,
        lights: Vec::new(),
        vertex_key: None,
    }
}

/// A cloud cell's point: position, texture coordinates and alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CloudPoint {
    p: [f32; 3],
    uv: [f32; 2],
    a: f32,
}

/// TGE's `PlaneF::whichSide` epsilon.
const PLANE_EPSILON: f32 = 0.005;

/// `Cloud::clipToPlane`: `sgUtil_clipToPlane`'s walk for a cell against the plane through the origin with normal `n`, the texture
/// coordinates and the alpha interpolated at each cut.
fn clip_cloud(pts: &[CloudPoint], n: [f32; 3]) -> Vec<CloudPoint> {
    let count = pts.len();
    if count == 0 {
        return Vec::new();
    }
    let dist = |q: &CloudPoint| n[0] * q.p[0] + n[1] * q.p[1] + n[2] * q.p[2];
    // 1 front, 0 on, -1 back.
    let side = |q: &CloudPoint| {
        let d = dist(q);
        if d > PLANE_EPSILON {
            1
        } else if d < -PLANE_EPSILON {
            -1
        } else {
            0
        }
    };
    let Some(base) = (0..count).find(|&i| side(&pts[i]) == 1) else {
        return Vec::new();
    };
    let lerp = |a: &CloudPoint, b: &CloudPoint| {
        let v = [0, 1, 2].map(|k| b.p[k] - a.p[k]);
        let t = -(dist(a) / (n[0] * v[0] + n[1] * v[1] + n[2] * v[2]));
        CloudPoint {
            p: [0, 1, 2].map(|k| a.p[k] + v[k] * t),
            uv: [0, 1].map(|k| a.uv[k] + (b.uv[k] - a.uv[k]) * t),
            a: a.a + (b.a - a.a) * t,
        }
    };
    let mut pts = pts.to_vec();
    let mut out = Vec::new();
    let (mut start, mut end) = (base, (base + 1) % count);
    while end != base {
        let (fs, es) = (side(&pts[start]), side(&pts[end]));
        if es >= 0 || fs < 0 {
            // f f, f o, o f, o o (b f does not happen in a convex walk from a front point).
            out.push(pts[start]);
            start = end;
            end = (end + 1) % count;
        } else {
            // f b or o b: the start, (for f b) the cut, then on to the first point not behind, its edge cut too.
            out.push(pts[start]);
            if fs == 1 {
                let cut = lerp(&pts[start], &pts[end]);
                out.push(cut);
            }
            let mut seek = (end + 1) % count;
            while side(&pts[seek]) == -1 {
                seek = (seek + 1) % count;
            }
            end = seek;
            start = (end + count - 1) % count;
            let cut = lerp(&pts[start], &pts[end]);
            pts[start] = cut;
        }
    }
    out.push(pts[start]);
    out
}

/// `sgUtil_clipToPlane` of a convex polygon against `z >= h`, as TGE orders the result (from the first front point on, each cut
/// point after the point it follows).
pub fn clip_above(p: &[[f32; 3]], h: f32) -> Vec<[f32; 3]> {
    let n = p.len();
    let front = |q: &[f32; 3]| q[2] - h > 0.0;
    let Some(start0) = (0..n).find(|&i| front(&p[i])) else {
        return Vec::new();
    };
    let mut pts = p.to_vec();
    let cut = |a: [f32; 3], b: [f32; 3]| {
        let t = -((a[2] - h) / (b[2] - a[2]));
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    };
    let mut out = Vec::new();
    let mut start = start0;
    let mut end = (start + 1) % n;
    while end != start0 {
        if front(&pts[start]) && !front(&pts[end]) && pts[end][2] - h < 0.0 {
            out.push(pts[start]);
            out.push(cut(pts[start], pts[end]));
            let mut seek = (end + 1) % n;
            while pts[seek][2] - h < 0.0 {
                seek = (seek + 1) % n;
            }
            end = seek;
            start = (end + n - 1) % n;
            pts[start] = cut(pts[start], pts[end]);
        } else {
            out.push(pts[start]);
            start = end;
            end = (end + 1) % n;
        }
    }
    out.push(pts[start]);
    out
}

#[cfg(test)]
#[path = "sky_tests.rs"]
mod tests;
