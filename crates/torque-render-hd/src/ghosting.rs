//! The ghosting check's measures (`examples/ghosting.rs`, and a game's own scenes through the same calls): a frame drawn with
//! every history against the reference of that instant ([`settled`]: the scene frozen, every history started again and the
//! frame drawn until its samples settle), as the share of pixels off ([`share_off`]) frame after frame, summed up per scene
//! ([`Measure`]).

use torque_render_core::{Frame, Gpu, Offscreen};
use torque_render_scene::hd::HdCamera;

use crate::HdWorld;
use crate::ext::HdExtension;

/// A channel this far (of 255) from the reference is off.
pub const TOLERANCE: f32 = 8.0;
/// A frame with at most this share of its pixels off is within the reference.
pub const AREA_OK: f64 = 0.002;
/// The reference: frames drawn at a frozen instant from a fresh history, of which the last are averaged.
pub const REF_FRAMES: usize = 32;
pub const REF_AVERAGED: usize = 8;
/// The pass condition of a scene drawn by default (the checks fail past it; `--max-settle` and `--max-trail` change it): it
/// settles within this many frames after each change, and its worst share off after a change is at most this (percent).
pub const MAX_SETTLE: usize = 8;
pub const MAX_TRAIL: f64 = 3.0;
/// The settled frames before a scene's change whose shares are its floor (the noise left once settled).
pub const FLOOR_FRAMES: usize = 8;

/// An image as floats (0 to 255 a channel, RGB).
pub type Image = Vec<[f32; 3]>;

pub fn image(f: &Frame) -> Image {
    f.rgba
        .chunks_exact(4)
        .map(|p| [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])])
        .collect()
}

/// The image as RGB bytes.
pub fn bytes(img: &Image) -> Vec<u8> {
    img.iter()
        .flat_map(|p| p.map(|c| c.round().clamp(0.0, 255.0) as u8))
        .collect()
}

/// The share of the pixels of `a` off `b` (a channel more than [`TOLERANCE`] away).
pub fn share_off(a: &Image, b: &Image) -> f64 {
    let off = a
        .iter()
        .zip(b)
        .filter(|(p, q)| (0..3).any(|c| (p[c] - q[c]).abs() > TOLERANCE + 0.5))
        .count();
    off as f64 / a.len().max(1) as f64
}

/// The difference of `a` from `b`, four times as bright: red where `a` is lighter, blue where darker.
pub fn difference(a: &Image, b: &Image) -> Image {
    a.iter()
        .zip(b)
        .map(|(p, q)| {
            let d = (0..3).map(|c| p[c] - q[c]).sum::<f32>() / 3.0;
            [(d * 4.0).max(0.0), 0.0, (-d * 4.0).max(0.0)]
        })
        .collect()
}

/// The reference of `scene` (uploaded) from `camera`: `world`'s histories started again, [`REF_FRAMES`] drawn into `target`
/// without anything moving, the last [`REF_AVERAGED`] averaged.
pub fn settled<E: HdExtension>(
    gpu: &Gpu,
    world: &mut HdWorld<E>,
    scene: &E::Scene,
    camera: &HdCamera,
    target: &Offscreen,
) -> Result<Image, String> {
    world.reset_history();
    let format = target.color.texture().format();
    let mut sum: Image = Vec::new();
    for k in 0..REF_FRAMES {
        world.draw(scene, camera, Some((&target.color, format, [0, 0])))?;
        if k >= REF_FRAMES - REF_AVERAGED {
            let img = image(&gpu.read_back(target).map_err(|e| e.to_string())?);
            sum.resize(img.len(), [0.0; 3]);
            for (s, p) in sum.iter_mut().zip(img) {
                for c in 0..3 {
                    s[c] += p[c] / REF_AVERAGED as f32;
                }
            }
        }
    }
    Ok(sum)
}

/// A scene's summary: `floor` the share off once settled before the change, `worst` and `mean` from the change on, `settle`
/// the most frames after a change ended until every frame to the next change is within ([`AREA_OK`], or half again the
/// floor), `trail` the worst share after a change ended, `worst_at` the frame of the worst share.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Measure {
    pub floor: f64,
    pub worst: f64,
    pub mean: f64,
    pub settle: usize,
    pub trail: f64,
    pub worst_at: usize,
}

impl Measure {
    /// Of the frames' shares off `areas`, the change starting at frame `start` (after at least [`FLOOR_FRAMES`]) and ending at
    /// each of `ends` (in order; each runs to the next or the last frame).
    pub fn of(areas: &[f64], start: usize, ends: &[usize]) -> Measure {
        let floor_frames = &areas[start.saturating_sub(FLOOR_FRAMES)..start];
        let floor = floor_frames.iter().sum::<f64>() / floor_frames.len().max(1) as f64;
        let (worst_at, worst) = areas
            .iter()
            .enumerate()
            .skip(start)
            .fold((start, 0.0), |a, (i, &v)| if v > a.1 { (i, v) } else { a });
        let rest = &areas[start..];
        let mean = rest.iter().sum::<f64>() / rest.len().max(1) as f64;
        let within = AREA_OK.max(floor * 1.5);
        let (mut settle, mut trail) = (0, 0.0f64);
        for (k, &end) in ends.iter().enumerate() {
            let next = ends.get(k + 1).copied().unwrap_or(areas.len());
            let span = &areas[end.min(next)..next];
            trail = trail.max(span.iter().copied().fold(0.0, f64::max));
            settle = settle.max(span.iter().rposition(|&a| a > within).map_or(0, |i| i + 1));
        }
        Measure {
            floor,
            worst,
            mean,
            settle,
            trail,
            worst_at,
        }
    }

    /// The table's header and this scene's row.
    pub const HEADER: &str =
        "scene          histories             floor%  worst%   mean%  settle  trail%";

    pub fn row(&self, scene: &str, histories: &str) -> String {
        format!(
            "{:<14} {:<20} {:>7.2} {:>7.2} {:>7.2} {:>7} {:>7.2}",
            scene,
            histories,
            self.floor * 100.0,
            self.worst * 100.0,
            self.mean * 100.0,
            self.settle,
            self.trail * 100.0
        )
    }
}

/// A run's settings: its name and what it changes of a world.
pub type Setting<E> = (&'static str, fn(&mut HdWorld<E>));

/// The runs: as drawn by default, then (`audit`) each pass's history off on its own ([`crate::Histories`]), the reflection
/// capture drawn whole every frame, and every history off.
pub fn settings<E: HdExtension>(audit: bool) -> Vec<Setting<E>> {
    let mut s: Vec<Setting<E>> = vec![("default", |_| {})];
    if audit {
        let each: [Setting<E>; 6] = [
            ("no taa history", |w| w.histories.taa = false),
            ("no ssr history", |w| w.histories.ssr = false),
            ("no cloud history", |w| w.histories.clouds = false),
            ("no exposure history", |w| w.histories.exposure = false),
            ("capture every frame", |w| w.capture_every_frame = true),
            ("no history at all", |w| {
                w.histories = crate::Histories {
                    taa: false,
                    ssr: false,
                    clouds: false,
                    exposure: false,
                };
                w.capture_every_frame = true;
            }),
        ];
        s.extend(each);
    }
    s
}
