//! What the decal checks and examples share: the decals' textures made in code (`decal/<name>.png`), their datablocks, and a
//! scene of static surfaces (a floor, walls, a ball) drawn as planes, each with its triangles for the CPU path's clip.

#![allow(
    dead_code,
    reason = "each check and example takes the part of the kit it needs"
)]

use torque_render_scene::decals::{DecalData, Triangle};
use torque_render_scene::hd::distortion::sphere;
use torque_render_scene::hd::kit::box_object;
use torque_render_scene::hd::{HdDraw, material};
use torque_render_scene::{Assets, Blend, Cull};

/// The textures' side (texels).
pub const SIDE: u32 = 128;

/// A hash of a texel and a seed, 0 to 1.
fn noise(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x
        .wrapping_mul(0x8da6_b343)
        .wrapping_add(y.wrapping_mul(0xd816_3841))
        .wrapping_add(seed.wrapping_mul(0xcb1a_b31f));
    h ^= h >> 13;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 16;
    (h & 0xffff) as f32 / 65535.0
}

/// Smooth value noise at `(u, v)` (cells of one unit), 0 to 1.
fn smooth_noise(u: f32, v: f32, seed: u32) -> f32 {
    let (x0, y0) = (u.floor(), v.floor());
    let (fx, fy) = (u - x0, v - y0);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let at = |dx: f32, dy: f32| noise((x0 + dx + 64.0) as u32, (y0 + dy + 64.0) as u32, seed);
    let top = at(0.0, 0.0) + (at(1.0, 0.0) - at(0.0, 0.0)) * sx;
    let bottom = at(0.0, 1.0) + (at(1.0, 1.0) - at(0.0, 1.0)) * sx;
    top + (bottom - top) * sy
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Texel `(x, y)` of the texture `name`: `hole` (a bullet hole: a dark pit, a scorched ring and cracks), `scorch` (a blast's
/// mottled soot), `splat` (an alpha-blended paint splat). The multiplied ones are gray 128 (the surface as it is) and
/// transparent toward their edge, so no square of the box shows.
pub fn texel(name: &str, x: u32, y: u32) -> [u8; 4] {
    let u = (x as f32 + 0.5) / SIDE as f32 * 2.0 - 1.0;
    let v = (y as f32 + 0.5) / SIDE as f32 * 2.0 - 1.0;
    let r = (u * u + v * v).sqrt();
    let angle = v.atan2(u);
    let gray = |g: f32, a: f32| {
        let g = (g.clamp(0.0, 1.0) * 255.0).round() as u8;
        [g, g, g, (a.clamp(0.0, 1.0) * 255.0).round() as u8]
    };
    match name {
        "hole" => {
            let pit = 1.0 - smoothstep(0.12, 0.26, r);
            let ring = 1.0 - smoothstep(0.0, 0.22, (r - 0.34).abs());
            let rays = (angle * 7.0 + 0.6 * (angle * 3.0).sin())
                .sin()
                .abs()
                .powf(24.0);
            let cracks = rays * (1.0 - smoothstep(0.3, 0.85, r)) * smoothstep(0.2, 0.3, r);
            let shade = 0.5 - 0.46 * pit - 0.16 * ring - 0.2 * cracks;
            gray(shade, 1.0 - smoothstep(0.7, 0.98, r))
        }
        "scorch" => {
            let mottle =
                0.7 * smooth_noise(u * 4.0, v * 4.0, 7) + 0.3 * smooth_noise(u * 11.0, v * 11.0, 9);
            let reach =
                0.75 + 0.25 * smooth_noise(angle.cos() * 2.5 + 3.0, angle.sin() * 2.5 + 3.0, 5);
            let soot = 1.0 - smoothstep(0.0, reach, r);
            gray(
                0.5 - 0.4 * soot.powf(0.7) * (0.6 + 0.6 * mottle),
                1.0 - smoothstep(reach * 0.8, reach, r),
            )
        }
        "splat" => {
            let lobes = 0.6 + 0.16 * (angle * 5.0).sin() + 0.08 * (angle * 11.0 + 1.3).sin();
            let a = smoothstep(lobes, lobes - 0.05, r);
            let shade = 0.85 + 0.15 * smooth_noise(u * 6.0, v * 6.0, 3);
            [
                (shade * 150.0) as u8,
                (shade * 34.0) as u8,
                (shade * 28.0) as u8,
                (a * 220.0) as u8,
            ]
        }
        _ => [255, 0, 255, 255],
    }
}

/// The decals' textures as files: `decal/hole.png`, `decal/scorch.png`, `decal/splat.png`.
pub struct DecalTextures;

impl Assets for DecalTextures {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let name = path.strip_prefix("decal/")?.strip_suffix(".png")?;
        if !["hole", "scorch", "splat"].contains(&name) {
            return None;
        }
        let mut texels = Vec::with_capacity((SIDE * SIDE * 4) as usize);
        for y in 0..SIDE {
            for x in 0..SIDE {
                texels.extend_from_slice(&texel(name, x, y));
            }
        }
        let mut png = Vec::new();
        let mut enc = png::Encoder::new(&mut png, SIDE, SIDE);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(&texels).ok()?;
        drop(w);
        Some(png)
    }
}

/// A decal's datablock: `name`'s texture, `size` across, drawn with `blend`.
pub fn decal(name: &str, size: [f32; 2], blend: Blend) -> DecalData {
    DecalData {
        size,
        texture: Some(format!("decal/{name}")),
        blend,
        distortion: None,
    }
}

/// A static surface: its draw (a plane: the static geometry, which takes decals) and its triangles in the world.
pub struct Surface {
    pub draw: HdDraw,
    pub triangles: Vec<Triangle>,
}

impl Surface {
    fn of(mut draw: HdDraw, color: [u8; 4]) -> Surface {
        for v in &mut draw.vertices {
            v.material = material::KIND_PLANE << material::KIND_SHIFT;
            v.color = color;
        }
        draw.blend = Blend::Opaque;
        draw.cull = Cull::None;
        draw.depth_write = true;
        draw.depth_test = true;
        let triangles = draw
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| t.map(|i| draw.vertices[i as usize].position))
            .collect();
        Surface { draw, triangles }
    }

    /// A box of half sides `h` at `center` in `color`, its faces counter-clockwise from outside (as the CPU path's clip takes
    /// them).
    pub fn block(center: [f32; 3], h: [f32; 3], color: [u8; 4]) -> Surface {
        let mut draw = box_object(0, center, h).draws.remove(0);
        // The kit's box winds its negative faces clockwise from outside: turned over here.
        for q in draw.indices.as_chunks_mut::<6>().0 {
            let n = torque_render_scene::hd::oct_decode(draw.vertices[q[0] as usize].normal);
            let [a, b, c] = [q[0], q[1], q[2]].map(|i| draw.vertices[i as usize].position);
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let face = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            if face[0] * n[0] + face[1] * n[1] + face[2] * n[2] < 0.0 {
                q.swap(1, 2);
                q.swap(4, 5);
            }
        }
        Surface::of(draw, color)
    }

    /// A ball of `radius` at `center` in `color`.
    pub fn ball(center: [f32; 3], radius: f32, color: [u8; 4]) -> Surface {
        Surface::of(sphere(center, radius, 32, 64, 255), color)
    }
}
