//! The enhanced renderer's screen-space distortion, drawn offscreen for review: a shockwave's sphere growing and fading over a
//! textured yard (a ring of pillars, the kit's figure), a glass ball that magnifies what is behind it, and a heat haze rising
//! over a glowing block (a card with a scrolling normal map). The camera circles slowly.
//!
//! Writes `distortion-clip.mp4` (1920 x 1080 at 30 frames a second, through `ffmpeg` on the path), `shockwave.png` and
//! `glass-haze.png` (1080p stills), and prints the GPU frame time at Ultra with and without the distorters (the median of
//! `--bench` frames, 120 by default; run it under the bench lock).
//!
//! `cargo run -p torque-example-common --release --example distortion-clip -- <dir> [--bench <frames>]`

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use torque::app::view::{Daylight, SceneShape, WorldView};
use torque::app::window::offscreen_gpu;
use torque_example_common::{KitAssets, kit_path};
use torque_render_scene::hd::distortion::{DistortionLook, HdDistorter, sphere};
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::hd::{DynamicObject, HdDraw, HdVertex, Quality, material, oct_encode};
use torque_render_scene::math::Mat;
use torque_render_scene::source::Eye;
use torque_render_scene::{Cull, Filter, Sampler, Texture};

const SIZE: (u32, u32) = (1920, 1080);
const FPS: u32 = 30;
const LENGTH: f32 = 8.0;
/// A shockwave's period (seconds), the radius it grows to and where it starts.
const WAVE_PERIOD: f32 = 2.6;
const WAVE_RADIUS: f32 = 14.0;
const WAVE_AT: [f32; 3] = [0.0, 6.0, 0.6];
const GLASS_AT: [f32; 3] = [-3.2, 2.0, 1.3];
const HAZE_AT: [f32; 3] = [3.4, 2.4, 0.0];
/// The heat haze's normal map's side (texels).
const HAZE_MAP: u32 = 128;

/// A tileable bumpy normal map: sums of waves whose slopes are its xy.
fn haze_map() -> Texture {
    let n = HAZE_MAP as f32;
    let tau = std::f32::consts::TAU;
    let waves = [
        (3.0, 1.0, 0.0, 1.0),
        (-2.0, 5.0, 1.3, 0.6),
        (7.0, 4.0, 2.1, 0.35),
        (1.0, -9.0, 0.4, 0.25),
    ];
    let mut px = Vec::with_capacity((HAZE_MAP * HAZE_MAP * 4) as usize);
    for y in 0..HAZE_MAP {
        for x in 0..HAZE_MAP {
            let (u, v) = (x as f32 / n, y as f32 / n);
            let (mut dx, mut dy) = (0.0f32, 0.0f32);
            for (kx, ky, phase, a) in waves {
                let c = (tau * (kx * u + ky * v) + phase).cos() * a;
                dx += c * kx;
                dy += c * ky;
            }
            let enc = |s: f32| ((0.5 + 0.5 * (s / 6.0).clamp(-1.0, 1.0)) * 255.0).round() as u8;
            px.extend_from_slice(&[enc(dx), enc(dy), 255, 255]);
        }
    }
    Texture {
        path: "distortion haze".into(),
        width: HAZE_MAP,
        height: HAZE_MAP,
        levels: std::sync::Arc::new(vec![px]),
    }
}

/// A vertical card `w` wide and `h` high standing at `at`, facing `eye` (two-sided), its texture coordinates its own. The bend
/// (the vertex alpha) fades to none at its sides and its top, and is weaker at its foot: no edge of the card shows.
fn card(at: [f32; 3], w: f32, h: f32, eye: [f32; 3]) -> HdDraw {
    const COLUMNS: u32 = 8;
    const ROWS: u32 = 8;
    let to = [eye[0] - at[0], eye[1] - at[1]];
    let l = (to[0] * to[0] + to[1] * to[1]).sqrt().max(1e-3);
    let (nx, ny) = (to[0] / l, to[1] / l);
    let side = [-ny * w, nx * w];
    let mut d = HdDraw::empty();
    d.cull = Cull::None;
    let n = oct_encode([nx, ny, 0.0]);
    for r in 0..=ROWS {
        let v = r as f32 / ROWS as f32;
        for c in 0..=COLUMNS {
            let u = c as f32 / COLUMNS as f32;
            let across = (std::f32::consts::PI * u).sin();
            let up = (std::f32::consts::PI * v).sin() * (0.4 + 0.6 * v.min(0.5) * 2.0);
            let s = u - 0.5;
            d.vertices.push(HdVertex {
                position: [
                    at[0] + s * side[0],
                    at[1] + s * side[1],
                    at[2] + h * (1.0 - v),
                ],
                normal: n,
                uv: [u, v * h / w],
                color: [255, 255, 255, (across * up * 255.0) as u8],
                material: 0,
                extra: 0,
            });
        }
    }
    let row = COLUMNS + 1;
    for r in 0..ROWS {
        for c in 0..COLUMNS {
            let a = r * row + c;
            d.indices
                .extend_from_slice(&[a, a + 1, a + row, a + 1, a + row + 1, a + row]);
        }
    }
    d
}

/// The distorters at time `t` seen from `eye`.
fn distorters(t: f32, eye: [f32; 3], map: usize) -> Vec<HdDistorter> {
    let phase = (t % WAVE_PERIOD) / WAVE_PERIOD;
    let radius = 0.4 + WAVE_RADIUS * phase.powf(0.6);
    let fade = ((1.0 - phase).powf(1.6) * 255.0) as u8;
    let wave = HdDistorter::new(
        sphere(WAVE_AT, radius, 32, 64, fade),
        DistortionLook {
            strength: 1.0,
            edge: 3.0,
            chroma: 0.12,
            ..DistortionLook::default()
        },
    );
    let glass = HdDistorter::new(
        sphere(GLASS_AT, 1.1, 32, 64, 255),
        DistortionLook {
            strength: -0.35,
            edge: 0.0,
            tint: [0.8, 0.92, 1.0, 0.5],
            chroma: 0.04,
            ..DistortionLook::default()
        },
    );
    let haze = HdDistorter::new(
        card([HAZE_AT[0], HAZE_AT[1], HAZE_AT[2] + 0.8], 2.2, 3.4, eye),
        DistortionLook {
            strength: 0.05,
            map: Some((
                map,
                Sampler {
                    repeat: true,
                    mag: Filter::Linear,
                    min: Filter::Linear,
                    mip: Filter::Linear,
                    mipmaps: false,
                },
            )),
            map_scale: 1.5,
            scroll: [0.03, 0.45],
            ..DistortionLook::default()
        },
    );
    vec![wave, glass, haze]
}

/// The yard: pillars in a ring, a glowing block under the haze, a pedestal under the glass.
fn yard(figure: Option<DynamicObject>) -> Vec<DynamicObject> {
    let mut out = Vec::new();
    for k in 0..14 {
        let a = k as f32 / 14.0 * std::f32::consts::TAU;
        let mut o = box_object(
            10 + k,
            [WAVE_AT[0] + 9.0 * a.cos(), WAVE_AT[1] + 9.0 * a.sin(), 1.6],
            [0.35, 0.35, 1.6],
        );
        let tone = if k % 2 == 0 {
            [70, 110, 190, 255]
        } else {
            [210, 205, 190, 255]
        };
        o.draws
            .iter_mut()
            .flat_map(|d| d.vertices.iter_mut())
            .for_each(|v| v.color = tone);
        out.push(o);
    }
    let mut glow = box_object(40, [HAZE_AT[0], HAZE_AT[1], 0.35], [0.6, 0.6, 0.35]);
    for v in glow.draws.iter_mut().flat_map(|d| d.vertices.iter_mut()) {
        v.color = [255, 120, 30, 255];
        v.material |= material::GLOW | material::UNLIT;
    }
    out.push(glow);
    let mut pedestal = box_object(41, [GLASS_AT[0], GLASS_AT[1], 0.1], [0.5, 0.5, 0.1]);
    pedestal
        .draws
        .iter_mut()
        .flat_map(|d| d.vertices.iter_mut())
        .for_each(|v| v.color = [90, 90, 95, 255]);
    out.push(pedestal);
    out.extend(figure);
    out
}

/// The camera at time `t`: circling the yard slowly at a person's height.
fn eye_at(t: f32) -> Eye {
    let a = -1.75 + 0.06 * t;
    let at = [
        WAVE_AT[0] + 13.0 * a.cos(),
        WAVE_AT[1] + 13.0 * a.sin(),
        2.4,
    ];
    let mut eye = look_at(at, [WAVE_AT[0] * 0.6, WAVE_AT[1] * 0.5, 1.2]);
    eye.fov = Some(70);
    eye
}

fn png(path: &Path, frame: &torque_render_core::Frame) -> std::io::Result<()> {
    let w = frame.width;
    let bytes = torque_example_assets::textures::png(frame.width, frame.height, |x, y| {
        let i = 4 * (y * w + x) as usize;
        [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
    })?;
    std::fs::write(path, bytes)
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().unwrap_or_else(|| "distortion-clip".to_string()));
    let mut bench = 120usize;
    while let Some(a) = args.next() {
        if a == "--bench" {
            bench = args
                .next()
                .and_then(|n| n.parse().ok())
                .ok_or("--bench <frames>")?;
        }
    }
    std::fs::create_dir_all(&dir)?;
    let gpu = offscreen_gpu()?;
    let assets = KitAssets::new()?;
    let mut view = WorldView::new(&gpu);
    view.set_quality(Quality::Ultra);
    view.set_floor(&assets, &kit_path("arena_floor.png"), 60.0, 2.0);
    view.set_daylight(&Daylight {
        towards: [0.5, -0.45, 0.62],
        ..Daylight::default()
    });
    let figure = SceneShape::load(&assets, &kit_path("figure.dts")).ok_or("figure.dts")?;
    let pose = figure.instance().ok_or("no instance")?;
    let mut place = Mat::IDENTITY;
    place.m[3] = 1.6;
    place.m[7] = 9.0;
    let figure = view.shape_object(&assets, &figure, &pose, &place, 2);
    let map = view.scene_mut().frame.textures.intern(haze_map());
    let objects = yard(Some(figure));

    let mp4 = dir.join("distortion-clip.mp4");
    let mut ffmpeg = Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
        ])
        .arg(format!("{}x{}", SIZE.0, SIZE.1))
        .args([
            "-r",
            &FPS.to_string(),
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-crf",
            "16",
        ])
        .arg(&mp4)
        .stdin(Stdio::piped())
        .spawn()?;
    let mut pipe = ffmpeg.stdin.take().ok_or("no ffmpeg input")?;
    let dt = 1.0 / FPS as f32;
    // The stills: the wave half grown, and the glass and the haze at the end.
    let stills = [
        (WAVE_PERIOD * 0.15, "shockwave.png"),
        (LENGTH - dt, "glass-haze.png"),
    ];
    let mut rgb = vec![0u8; (SIZE.0 * SIZE.1 * 3) as usize];
    view.reset_history();
    for k in 0..(LENGTH * FPS as f32) as usize {
        let t = k as f32 * dt;
        let eye = eye_at(t);
        view.set_objects(objects.clone());
        view.scene_mut().frame.distorters = distorters(t, eye.position, map);
        view.advance_time(dt);
        let frame = view.render_world(&gpu, SIZE, eye)?;
        for (out, px) in rgb
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .zip(frame.rgba.as_chunks::<4>().0)
        {
            out.copy_from_slice(&px[..3]);
        }
        pipe.write_all(&rgb)?;
        for (at, name) in stills {
            if (t - at).abs() < dt * 0.5 {
                png(&dir.join(name), &frame)?;
                println!("{}", dir.join(name).display());
            }
        }
    }
    drop(pipe);
    if !ffmpeg.wait()?.success() {
        return Err("ffmpeg failed".into());
    }
    println!("{}", mp4.display());

    // The GPU frame time with and without the distorters, at the shockwave still's moment.
    if bench > 0 {
        let t = stills[0].0;
        let eye = eye_at(t);
        let times = |on: bool, view: &mut WorldView| -> Result<f64, Box<dyn std::error::Error>> {
            let mut ms = Vec::new();
            for _ in 0..bench {
                view.set_objects(objects.clone());
                view.scene_mut().frame.distorters = if on {
                    distorters(t, eye.position, map)
                } else {
                    Vec::new()
                };
                view.render_world(&gpu, SIZE, eye)?;
                if let Some(g) = view.gpu_ms() {
                    ms.push(g);
                }
            }
            Ok(median(ms))
        };
        let before = times(false, &mut view)?;
        let with = times(true, &mut view)?;
        let after = times(false, &mut view)?;
        println!(
            "gpu ms at 1080p Ultra: without {before:.3} / {after:.3}, with {with:.3}, the distortion {:.3}",
            with - 0.5 * (before + after)
        );
    }
    Ok(())
}
