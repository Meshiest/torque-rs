//! The voxels' light against the CPU: a plane lit by each kind of light (the sun, a point, a spot, a rectangle, a tube) and a
//! glowing plane, each voxel's radiance against the forward pass's diffuse light at its surface (an area light's summed over
//! its shape).

use glam::Vec3;
use torque_render_core::Gpu;
use torque_render_scene::hd::cells::StaticCells;
use torque_render_scene::hd::{
    AreaShape, HdAreaLight, HdCamera, HdLight, HdMode, HdScene, HdSpot, HdSun, Quality, material,
    srgb_to_linear,
};

use super::clipmap::{N, TILE, TILE_VOXELS};
use super::tests::{filled, gpu, quad, scene_with, word};
use crate::HdWorld;
use crate::ext::{NoExtension, PlainScene};

/// The plane's height: inside the voxels of z 0 to 0.25.
const PLANE_Z: f32 = 0.1;

/// The plane the checks light: 6 x 6 at [`PLANE_Z`], facing up.
fn lit_plane() -> Vec<[Vec3; 3]> {
    quad(
        Vec3::new(0.0, 0.0, PLANE_Z),
        Vec3::new(3.0, 0.0, 0.0),
        Vec3::new(0.0, 3.0, 0.0),
    )
    .to_vec()
}

/// The plane's albedo as the voxels hold it (rgba8 of the linear color).
fn plane_albedo() -> [f32; 3] {
    [200u8, 150, 100].map(|c| (srgb_to_linear(f32::from(c) / 255.0) * 255.0).round() / 255.0)
}

/// The finest level's voxels of the plane (`bits` in its material word) lit by what `light` puts in the scene, each with its
/// surface's point and radiance, after one frame that made and lit every tile.
fn injected(
    gpu: &Gpu,
    bits: u32,
    light: impl Fn(&mut HdScene<StaticCells>),
) -> Vec<([f32; 3], [f32; 3])> {
    let mut frame = scene_with(&lit_plane(), bits);
    frame.sun.diffuse = [0.0; 3];
    frame.sun.ambient = [0.0; 3];
    light(&mut frame);
    let scene = PlainScene {
        frame,
        mode: Some(HdMode::new(Quality::High)),
        time: 0.0,
    };
    let mut world: HdWorld<NoExtension> = HdWorld::new(gpu);
    world.gi_mut().enabled = true;
    world.gi_mut().converge = true;
    let target = gpu.offscreen(64, 64).expect("a target");
    let camera = HdCamera::new(
        torque_render_scene::hd::kit::look_at([0.0, -4.0, 3.0], [0.0, 0.0, 0.0]),
        90.0_f32,
        200.0,
        64,
        64,
    );
    world.upload(&scene);
    world
        .draw(
            &scene,
            &camera,
            Some((&target.color, target.color.texture().format(), [0, 0])),
        )
        .expect("a frame");
    let bytes = world
        .gi()
        .read_voxels(gpu.device(), gpu.queue())
        .expect("the voxels");
    let mut out = Vec::new();
    for y in -11..11 {
        for x in -11..11 {
            let v = [x, y, 0];
            if !filled(&bytes, 0, v) {
                continue;
            }
            let t = v.map(|c| c.div_euclid(TILE));
            let s = t.map(|c| c.rem_euclid(N / TILE));
            let tiles = N / TILE;
            let slot = (s[0] + tiles * (s[1] + tiles * s[2])) as usize;
            let l = v.map(|c| c.rem_euclid(TILE));
            let i = slot * TILE_VOXELS as usize + (l[0] + TILE * (l[1] + TILE * l[2])) as usize;
            let radiance = super::sources::unpack_rgb9e5(word(&bytes[2], 4 * i + 3));
            // Lit at the plane, where the voxel's surface lies.
            out.push((
                [(x as f32 + 0.5) * 0.25, (y as f32 + 0.5) * 0.25, PLANE_Z],
                radiance,
            ));
        }
    }
    assert!(out.len() > 300, "the plane's voxels: {}", out.len());
    out
}

/// The forward pass's point light falloff (`forward.wgsl`'s `falloff`).
fn falloff(d: f32, radius: f32) -> f32 {
    let x = d / radius.max(1e-4);
    let w = (1.0 - x.powi(4)).clamp(0.0, 1.0);
    w * w / (1.0 + 0.25 * d * d)
}

/// An area light's light at `p` facing up per unit of its color (the forward pass's units), summed over its shape in 96 x 96
/// pieces (a tube's side and its two ends), windowed to its radius.
fn area_light_at(a: &HdAreaLight, p: Vec3) -> f32 {
    let c = Vec3::from(a.light.position);
    let k = 96;
    let mut sum = 0.0f64;
    let mut add = |q: Vec3, ln: Vec3, da: f32, radiance: f32| {
        let to = q - p;
        let d2 = to.length_squared();
        let dir = to / d2.sqrt();
        let (cp, cl) = (dir.z, -ln.dot(dir));
        if cp > 0.0 && cl > 0.0 {
            sum += f64::from(radiance * cp * cl / d2 * da);
        }
    };
    let cell = |i: u32| (i as f32 + 0.5) / k as f32;
    match a.shape {
        AreaShape::Rect { half_u, half_v } => {
            let (u, v) = (Vec3::from(half_u), Vec3::from(half_v));
            let ln = u.cross(v).normalize();
            let radiance = 1.0 / u.cross(v).length();
            let da = 4.0 * u.cross(v).length() / (k * k) as f32;
            for j in 0..k {
                for i in 0..k {
                    add(
                        c + u * (cell(i) * 2.0 - 1.0) + v * (cell(j) * 2.0 - 1.0),
                        ln,
                        da,
                        radiance,
                    );
                }
            }
        }
        AreaShape::Tube { half_axis, radius } => {
            let axis = Vec3::from(half_axis);
            let len = axis.length();
            let t = axis / len;
            let e1 = t.any_orthonormal_vector();
            let e2 = t.cross(e1);
            let radiance = 1.0 / (len * radius);
            let side = 2.0 * len * std::f32::consts::TAU * radius / (k * k) as f32;
            let disk = std::f32::consts::PI * radius * radius / (k * k) as f32;
            for j in 0..k {
                let phi = cell(j) * std::f32::consts::TAU;
                let out = e1 * phi.cos() + e2 * phi.sin();
                for i in 0..k {
                    add(
                        c + axis * (cell(i) * 2.0 - 1.0) + out * radius,
                        out,
                        side,
                        radiance,
                    );
                    for end in [-1.0f32, 1.0] {
                        add(
                            c + axis * end + out * (radius * cell(i).sqrt()),
                            t * end,
                            disk,
                            radiance,
                        );
                    }
                }
            }
        }
    }
    let x = (c - p).length() / a.gpu_light().radius.max(1e-4);
    let w = (1.0 - x.powi(4)).clamp(0.0, 1.0);
    (sum as f32) * w * w
}

/// The most relative error of the voxels' radiance against `expected` at their surfaces, over the values of at least a tenth
/// of the most light, and their count.
fn worst_error(
    voxels: &[([f32; 3], [f32; 3])],
    expected: impl Fn(Vec3) -> [f32; 3],
) -> (f32, usize) {
    let want: Vec<[f32; 3]> = voxels
        .iter()
        .map(|(p, _)| expected(Vec3::from(*p)))
        .collect();
    let top = want.iter().flatten().fold(0f32, |a, b| a.max(*b));
    assert!(top > 0.0, "the reference is lit");
    let mut worst = 0f32;
    let mut n = 0;
    for ((_, got), want) in voxels.iter().zip(&want) {
        for k in 0..3 {
            if want[k] < 0.1 * top {
                continue;
            }
            worst = worst.max((got[k] - want[k]).abs() / want[k]);
            n += 1;
        }
    }
    (worst, n)
}

fn light(position: [f32; 3], color: [f32; 3], radius: f32) -> HdLight {
    HdLight {
        position,
        color,
        radius,
        carrier: None,
    }
}

#[test]
fn each_light_kind_lights_the_voxels_as_the_forward_pass() {
    let Some(gpu) = gpu() else {
        return;
    };
    let albedo = plane_albedo();
    let lit = |e: [f32; 3]| -> [f32; 3] { std::array::from_fn(|k| albedo[k] * e[k]) };
    let mut worst = Vec::new();

    // The sun, from the side and above.
    let sun = HdSun {
        towards: Vec3::new(0.4, -0.3, 0.866).normalize().to_array(),
        diffuse: [0.7, 0.6, 0.5],
        ambient: [0.0; 3],
        shadow: [0.0; 3],
    };
    let voxels = injected(&gpu, 0, |s| s.sun = sun);
    let e = sun.diffuse.map(|c| srgb_to_linear(c) * sun.towards[2]);
    worst.push(("sun", worst_error(&voxels, |_| lit(e))));

    // A point light over the plane.
    let point = light([0.5, -0.4, 2.0], [1.0, 0.8, 0.6], 9.0);
    let voxels = injected(&gpu, 0, |s| s.lights = vec![point]);
    worst.push((
        "point",
        worst_error(&voxels, |p| {
            let to = Vec3::from(point.position) - p;
            let d = to.length();
            let nl = (to.z / d).max(0.0);
            lit(point.color.map(|c| c * nl * falloff(d, point.radius)))
        }),
    ));

    // A spot light looking down at a slant.
    let spot = HdSpot {
        light: light([-0.5, 0.3, 2.2], [1.2, 1.0, 0.9], 9.0),
        direction: [0.3, 0.0, -1.0],
        inner: 0.35,
        outer: 0.6,
        shadows: false,
    };
    let voxels = injected(&gpu, 0, |s| s.spots = vec![spot]);
    worst.push((
        "spot",
        worst_error(&voxels, |p| {
            let to = Vec3::from(spot.light.position) - p;
            let d = to.length();
            let nl = (to.z / d).max(0.0);
            let cone = spot.share(p.to_array());
            lit(spot
                .light
                .color
                .map(|c| c * nl * cone * falloff(d, spot.light.radius)))
        }),
    ));

    // A rectangle and a tube over the plane.
    for (name, shape) in [
        (
            "rectangle",
            AreaShape::Rect {
                half_u: [0.0, 0.5, 0.0],
                half_v: [1.0, 0.0, 0.0],
            },
        ),
        (
            "tube",
            AreaShape::Tube {
                half_axis: [0.9, 0.0, 0.0],
                radius: 0.05,
            },
        ),
    ] {
        let a = HdAreaLight {
            light: light([0.2, 0.1, 1.6], [1.0, 0.9, 0.8], 8.0),
            shape,
            shadows: false,
        };
        let voxels = injected(&gpu, 0, |s| s.area_lights = vec![a]);
        worst.push((
            name,
            worst_error(&voxels, |p| {
                let k = area_light_at(&a, p);
                lit(a.light.color.map(|c| c * k))
            }),
        ));
    }

    // A glowing plane in the dark: its voxels emit its color.
    let voxels = injected(&gpu, material::GLOW, |_| {});
    let glow = [200u8, 150, 100].map(|c| srgb_to_linear(f32::from(c) / 255.0));
    worst.push(("emission", worst_error(&voxels, |_| glow)));

    for (name, (e, n)) in &worst {
        eprintln!("{name}: worst {:.2} % over {n} values", e * 100.0);
    }
    for (name, (e, _)) in &worst {
        assert!(
            *e <= 0.03,
            "{name}: the voxels' light is {:.2} % off the reference",
            e * 100.0
        );
    }
}
