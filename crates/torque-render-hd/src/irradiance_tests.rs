//! The irradiance's projection (`irradiance.wgsl`) against the analytic irradiance of skies whose L2 harmonics are exact for the
//! normals checked: a uniform sky of radiance L gives pi L at every normal, a sky over a ground of two radiances gives each
//! hemisphere's pi L straight up and down and their mean sideways, and the parts the resolve adds (the sky's scale, the ratio's
//! horizontal normals, the ground lit by the sun and the sky, the capture's geometry beside its sky) land where they should.

use torque_render_core::{Gpu, wgpu};
use torque_render_scene::hd::sh::basis;

use super::{IrradianceParams, IrradiancePipe, LEVEL_SIZE, OUTPUT_BYTES, SkyLight, level_view};

/// An IEEE half of `v` (a normal number, 0, an infinity or not a number).
fn half(v: f32) -> u16 {
    if v == 0.0 {
        return 0;
    }
    if v.is_nan() {
        return 0x7e00;
    }
    if v.is_infinite() {
        return 0x7c00;
    }
    let bits = v.to_bits();
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    assert!((1..31).contains(&exp), "{v} is not a normal half");
    let mant = (bits >> 13) & 0x3ff;
    (((bits >> 16) & 0x8000) | (exp as u32) << 10 | mant) as u16
}

/// The direction of a cube face's texel centre (the cube map convention, `cubeface.wgsl`'s `face_dir`).
fn texel_dir(face: u32, x: u32, y: u32) -> [f32; 3] {
    let s = LEVEL_SIZE as f32;
    let (u, v) = (
        (x as f32 + 0.5) / s * 2.0 - 1.0,
        (y as f32 + 0.5) / s * 2.0 - 1.0,
    );
    let d = match face {
        0 => [1.0, -v, -u],
        1 => [-1.0, -v, u],
        2 => [u, 1.0, v],
        3 => [u, -1.0, -v],
        4 => [u, -v, 1.0],
        _ => [-u, -v, -1.0],
    };
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    d.map(|c| c / l)
}

/// A cube of [`LEVEL_SIZE`] whose texels are `radiance` of their direction (RGBA).
fn cube(gpu: &Gpu, radiance: impl Fn([f32; 3]) -> [f32; 4]) -> wgpu::Texture {
    let size = LEVEL_SIZE;
    let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("irradiance test cube"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: crate::capture::CUBE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for face in 0..6 {
        let mut bytes = Vec::with_capacity((size * size * 8) as usize);
        for y in 0..size {
            for x in 0..size {
                for c in radiance(texel_dir(face, x, y)) {
                    bytes.extend_from_slice(&half(c).to_le_bytes());
                }
            }
        }
        gpu.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: face,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 8),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
    }
    texture
}

/// The main view's and the sky's coefficients after one encode (or more: `faces` per call).
fn run(
    gpu: &Gpu,
    pipe: &mut IrradiancePipe,
    params: &IrradianceParams,
    calls: &[u32],
) -> [[[f32; 3]; 9]; 2] {
    let device = gpu.device();
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("irradiance readback"),
        size: OUTPUT_BYTES,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for (k, &faces) in calls.iter().enumerate() {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        pipe.encode(gpu.queue(), &mut encoder, params, faces);
        if k + 1 == calls.len() {
            encoder.copy_buffer_to_buffer(&pipe.output, 0, &readback, 0, OUTPUT_BYTES);
        }
        gpu.queue().submit([encoder.finish()]);
    }
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("mapped"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("the copy finishes");
    let data = slice.get_mapped_range().expect("the mapped bytes");
    let f: Vec<f32> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    std::array::from_fn(|set| {
        std::array::from_fn(|i| std::array::from_fn(|k| f[(set * 9 + i) * 4 + k]))
    })
}

/// The ambient's radiance (irradiance over pi) the coefficients give normal `n`.
fn ambient_at(c: &[[f32; 3]; 9], n: [f32; 3]) -> [f32; 3] {
    let y = basis(n);
    std::array::from_fn(|k| (0..9).map(|i| c[i][k] * y[i]).sum())
}

/// Asserts `got` is within `share` of `want` (each channel) and gives back its largest relative error.
fn near(got: [f32; 3], want: [f32; 3], share: f32, what: &str) -> f32 {
    let mut worst = 0.0f32;
    for k in 0..3 {
        let off = (got[k] - want[k]).abs() / want[k].abs().max(1e-3);
        assert!(off <= share, "{what}: {got:?} against {want:?}");
        worst = worst.max(off);
    }
    worst
}

fn gpu() -> Option<Gpu> {
    match Gpu::new() {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn pipe(gpu: &Gpu, sky: &wgpu::Texture, capture: &wgpu::Texture) -> IrradiancePipe {
    IrradiancePipe::new(
        gpu.device(),
        None,
        &level_view(sky, 0),
        &level_view(capture, 0),
    )
}

const PLAIN: IrradianceParams = IrradianceParams {
    sky: SkyLight::Scaled(1.0),
    bounce: false,
    ground: None,
    sun: [0.0; 3],
    sun_z: 0.0,
};

/// The normals checked: the axes and the diagonals.
fn normals() -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    for x in -1..=1 {
        for y in -1..=1 {
            for z in -1..=1 {
                if (x, y, z) != (0, 0, 0) {
                    let l = ((x * x + y * y + z * z) as f32).sqrt();
                    out.push([x as f32 / l, y as f32 / l, z as f32 / l]);
                }
            }
        }
    }
    out
}

#[test]
fn a_uniform_sky_gives_pi_l_at_every_normal() {
    let Some(gpu) = gpu() else { return };
    let l = [0.3, 0.5, 0.8];
    let sky = cube(&gpu, |_| [l[0], l[1], l[2], 1.0]);
    let mut p = pipe(&gpu, &sky, &sky);
    let [main, sky_only] = run(&gpu, &mut p, &PLAIN, &[0x3f]);
    let pi = std::f32::consts::PI;
    let mut worst = 0.0f32;
    for n in normals() {
        let e = ambient_at(&main, n).map(|v| v * pi);
        worst = worst.max(near(
            e,
            l.map(|v| v * pi),
            0.01,
            &format!("irradiance at {n:?}"),
        ));
        near(
            ambient_at(&sky_only, n),
            ambient_at(&main, n),
            1e-5,
            "the sky's own",
        );
    }
    println!("uniform sky: worst {:.3} % off pi L", worst * 100.0);
}

#[test]
fn a_sky_over_a_ground_gives_each_hemispheres_irradiance() {
    let Some(gpu) = gpu() else { return };
    let (above, below) = ([0.4, 0.6, 1.0], [0.3, 0.2, 0.1]);
    let sky = cube(&gpu, |d| {
        let c = if d[2] > 0.0 { above } else { below };
        [c[0], c[1], c[2], 1.0]
    });
    let mut p = pipe(&gpu, &sky, &sky);
    let [main, _] = run(&gpu, &mut p, &PLAIN, &[0x3f]);
    let pi = std::f32::consts::PI;
    let irradiance = |n| ambient_at(&main, n).map(|v| v * pi);
    let up = near(
        irradiance([0.0, 0.0, 1.0]),
        above.map(|v| v * pi),
        0.02,
        "up",
    );
    let down = near(
        irradiance([0.0, 0.0, -1.0]),
        below.map(|v| v * pi),
        0.02,
        "down",
    );
    let side: [f32; 3] = std::array::from_fn(|k| pi * (above[k] + below[k]) / 2.0);
    let mut sideways = 0.0f32;
    for n in [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.6, 0.8, 0.0]] {
        sideways = sideways.max(near(irradiance(n), side, 0.02, &format!("sideways {n:?}")));
    }
    println!(
        "two-tone sky: up {:.3} %, down {:.3} %, sideways {:.3} % off",
        up * 100.0,
        down * 100.0,
        sideways * 100.0
    );
}

#[test]
fn the_ground_takes_the_sun_and_the_sky_above_it() {
    let Some(gpu) = gpu() else { return };
    let above = [0.2, 0.3, 0.5];
    // Whatever the sky draws below the horizon, the ground stands there.
    let sky = cube(&gpu, |d| {
        if d[2] > 0.0 {
            [above[0], above[1], above[2], 1.0]
        } else {
            [9.0, 0.0, 9.0, 1.0]
        }
    });
    let mut p = pipe(&gpu, &sky, &sky);
    let albedo = [0.3, 0.25, 0.2];
    let params = IrradianceParams {
        sky: SkyLight::Scaled(2.0),
        ground: Some(albedo),
        sun: [1.0, 0.9, 0.8],
        sun_z: 0.5,
        ..PLAIN
    };
    let [main, sky_only] = run(&gpu, &mut p, &params, &[0x3f]);
    let lit = above.map(|v| 2.0 * v);
    near(
        ambient_at(&main, [0.0, 0.0, 1.0]),
        lit,
        0.02,
        "a level face",
    );
    let ground: [f32; 3] = std::array::from_fn(|k| albedo[k] * (params.sun[k] * 0.5 + lit[k]));
    near(
        ambient_at(&main, [0.0, 0.0, -1.0]),
        ground,
        0.02,
        "a face turned down",
    );
    near(
        ambient_at(&sky_only, [0.0, 0.0, -1.0]),
        ground,
        0.02,
        "the sky's own",
    );
}

#[test]
fn the_ratio_gives_the_horizontal_normals_the_scenes_ambient() {
    let Some(gpu) = gpu() else { return };
    // A sky three times as bright overhead as at the horizon, tinted (the ratio keeps the ambient's color, not the sky's).
    let sky = cube(&gpu, |d| {
        let g = 0.1 + 0.2 * d[2].max(0.0);
        [g * 0.5, g, g * 2.0, 1.0]
    });
    let mut p = pipe(&gpu, &sky, &sky);
    let ambient = [0.12, 0.1, 0.08];
    let params = IrradianceParams {
        sky: SkyLight::Ambient(ambient),
        ..PLAIN
    };
    let [main, _] = run(&gpu, &mut p, &params, &[0x3f]);
    // The horizontal normals take the scene's ambient on average.
    let mut mean = [0.0f32; 3];
    for k in 0..16 {
        let t = k as f32 * std::f32::consts::TAU / 16.0;
        let e = ambient_at(&main, [t.cos(), t.sin(), 0.0]);
        for c in 0..3 {
            mean[c] += e[c] / 16.0;
        }
    }
    near(mean, ambient, 0.01, "the horizontal normals");
    let up = ambient_at(&main, [0.0, 0.0, 1.0]);
    let side = ambient_at(&main, [1.0, 0.0, 0.0]);
    let down = ambient_at(&main, [0.0, 0.0, -1.0]);
    assert!(
        up[1] > side[1] && side[1] > down[1],
        "{up:?} {side:?} {down:?}"
    );
    for (s, a) in side.iter().zip(ambient) {
        assert!(
            (s / side[1] - a / ambient[1]).abs() < 1e-3,
            "the ambient's color: {side:?}"
        );
    }
}

#[test]
fn the_capture_bounces_its_scene_and_lights_its_sky_as_the_sky() {
    let Some(gpu) = gpu() else { return };
    let open = [0.5, 0.6, 0.8];
    let sky = cube(&gpu, |_| [open[0], open[1], open[2], 1.0]);
    // A red wall fills the capture's -x half (its alpha a distance), the sky the rest.
    let wall = [0.8, 0.1, 0.1];
    let capture = cube(&gpu, |d| {
        if d[0] < 0.0 {
            [wall[0], wall[1], wall[2], 5.0]
        } else {
            [open[0], open[1], open[2], 1.0]
        }
    });
    let mut p = pipe(&gpu, &sky, &capture);
    let params = IrradianceParams {
        sky: SkyLight::Scaled(0.5),
        bounce: true,
        ..PLAIN
    };
    let [main, sky_only] = run(&gpu, &mut p, &params, &[0xfff]);
    let lit = open.map(|v| 0.5 * v);
    near(
        ambient_at(&main, [-1.0, 0.0, 0.0]),
        wall,
        0.02,
        "facing the wall",
    );
    near(
        ambient_at(&main, [1.0, 0.0, 0.0]),
        lit,
        0.02,
        "facing the open sky",
    );
    // The secondary views take the sky the capture sees and nothing of the wall (no light of the capture's own).
    near(
        ambient_at(&sky_only, [1.0, 0.0, 0.0]),
        lit,
        0.02,
        "the gaps' sky",
    );
    let behind = ambient_at(&sky_only, [-1.0, 0.0, 0.0]);
    assert!(
        behind.iter().all(|v| v.abs() < 0.01 * lit[2]),
        "the wall lights no secondary view: {behind:?}"
    );
    // A frame that draws no face keeps its sums; one that draws a capture face projects that face alone.
    let [again, _] = run(&gpu, &mut p, &params, &[0]);
    near(ambient_at(&again, [-1.0, 0.0, 0.0]), wall, 0.02, "kept");
    let [one, _] = run(&gpu, &mut p, &params, &[1 << 6]);
    near(
        ambient_at(&one, [-1.0, 0.0, 0.0]),
        wall,
        0.02,
        "one face again",
    );
}

#[test]
fn a_closed_room_bounces_its_own_light_and_no_sky() {
    let Some(gpu) = gpu() else { return };
    let open = [0.5, 0.6, 0.8];
    let sky = cube(&gpu, |_| [open[0], open[1], open[2], 1.0]);
    // Walls all round the capture, lit to a warm gray.
    let walls = [0.3, 0.25, 0.2];
    let capture = cube(&gpu, |_| [walls[0], walls[1], walls[2], 4.0]);
    let mut p = pipe(&gpu, &sky, &capture);
    let params = IrradianceParams {
        sky: SkyLight::Ambient([0.1, 0.1, 0.1]),
        bounce: true,
        ..PLAIN
    };
    let [main, secondary] = run(&gpu, &mut p, &params, &[0xfff]);
    for n in normals() {
        near(ambient_at(&main, n), walls, 0.01, "the walls' light");
        let s = ambient_at(&secondary, n);
        assert!(
            s.iter().all(|v| v.abs() < 1e-4),
            "no sky light in a closed room: {s:?}"
        );
    }
}

/// The poisoned texels of `a_texel_that_is_not_finite_adds_nothing`: the ones straight up.
fn is_poisoned(d: [f32; 3]) -> bool {
    d[2] > 0.999
}

#[test]
fn a_texel_that_is_not_finite_adds_nothing() {
    let Some(gpu) = gpu() else { return };
    let l = [0.3, 0.5, 0.8];
    let poisoned = (0..6)
        .flat_map(|f| (0..LEVEL_SIZE).flat_map(move |y| (0..LEVEL_SIZE).map(move |x| (f, x, y))))
        .filter(|&(f, x, y)| is_poisoned(texel_dir(f, x, y)))
        .count();
    assert!(poisoned > 0, "the test cube has no texel to poison");
    // The texels straight up overflowed (a half's infinity) and are not a number in another channel.
    let sky = cube(&gpu, |d| {
        if is_poisoned(d) {
            [f32::INFINITY, f32::NAN, 1.0, 1.0]
        } else {
            [l[0], l[1], l[2], 1.0]
        }
    });
    let mut p = pipe(&gpu, &sky, &sky);
    let [main, _] = run(&gpu, &mut p, &PLAIN, &[0x3f]);
    for n in normals() {
        let e = ambient_at(&main, n);
        assert!(e.iter().all(|v| v.is_finite()), "{e:?}");
        near(e, l, 0.01, "the rest of the sky");
    }
}

#[test]
fn a_change_of_the_skys_kind_projects_every_face_again() {
    let Some(gpu) = gpu() else { return };
    // Below the horizon the ground stands where the sky draws something else.
    let sky = cube(&gpu, |d| {
        if d[2] > 0.0 {
            [0.2, 0.3, 0.5, 1.0]
        } else {
            [9.0, 0.0, 9.0, 1.0]
        }
    });
    let grounded = IrradianceParams {
        ground: Some([0.3, 0.25, 0.2]),
        ..PLAIN
    };
    let down = [0.0, 0.0, -1.0];
    let mut toggled = pipe(&gpu, &sky, &sky);
    let [drawn, _] = run(&gpu, &mut toggled, &PLAIN, &[0x3f]);
    // No face is drawn this time: the sums of the sky as drawn must not stand under the ground.
    let [again, _] = run(&gpu, &mut toggled, &grounded, &[0]);
    let mut fresh = pipe(&gpu, &sky, &sky);
    let [want, _] = run(&gpu, &mut fresh, &grounded, &[0x3f]);
    let moved = (0..3)
        .map(|k| (ambient_at(&again, down)[k] - ambient_at(&drawn, down)[k]).abs())
        .fold(0.0, f32::max);
    assert!(moved > 0.1, "the ground changed nothing: {moved}");
    near(
        ambient_at(&again, down),
        ambient_at(&want, down),
        1e-4,
        "a face turned down",
    );
}
