//! Digital rain: columns of glyphs falling down sheets of dark air, green on black, the camera drifting slowly forward
//! through sheet after sheet. Each column's head is a bright near-white glyph racing down; the glyphs it leaves behind
//! glow green and fade over a couple of seconds, changing to other glyphs now and then as they fade. The glyphs are an
//! atlas made here: strokes from a small vocabulary (bars, stems, hooks, slashes, ticks) combined at random into
//! katakana-like signs, each its own small sprite.

use glam::Vec3;
use torque_render_scene::hd::HdLight;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::particles::{EmitterData, ParticleData, Rng};
use torque_render_scene::source::Eye;

use super::{Effect, Lcg, Node, Setting, emitter, one, particle, plain};

/// The glyphs in the atlas and their sprite names.
pub const GLYPH_COUNT: usize = 48;
const GLYPH_SIDE: u32 = 32;

pub fn glyph_name(k: usize) -> String {
    format!("fx/rain{k}")
}

/// A stroke in the glyph's square (0..1 across, 0 at the top).
type Stroke = [(f32, f32); 2];

/// The vocabulary: bars, stems, slashes, hooks and ticks.
const STROKES: [Stroke; 16] = [
    [(0.15, 0.12), (0.85, 0.12)],
    [(0.15, 0.5), (0.85, 0.5)],
    [(0.15, 0.88), (0.85, 0.88)],
    [(0.2, 0.12), (0.2, 0.88)],
    [(0.5, 0.12), (0.5, 0.88)],
    [(0.8, 0.12), (0.8, 0.88)],
    [(0.2, 0.88), (0.8, 0.12)],
    [(0.2, 0.3), (0.8, 0.3)],
    [(0.5, 0.12), (0.5, 0.55)],
    [(0.5, 0.55), (0.82, 0.88)],
    [(0.5, 0.55), (0.18, 0.88)],
    [(0.3, 0.35), (0.3, 0.65)],
    [(0.7, 0.3), (0.7, 0.6)],
    [(0.15, 0.3), (0.5, 0.3)],
    [(0.5, 0.7), (0.85, 0.7)],
    [(0.3, 0.12), (0.7, 0.4)],
];

/// The distance from (`x`, `y`) to the stroke `s`.
fn distance(s: &Stroke, x: f32, y: f32) -> f32 {
    let [(ax, ay), (bx, by)] = *s;
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = (((x - ax) * dx + (y - ay) * dy) / len2).clamp(0.0, 1.0);
    let (px, py) = (ax + dx * t, ay + dy * t);
    (x - px).hypot(y - py)
}

/// The glyph sprites as PNG files: each two to four strokes of the vocabulary, half of them mirrored.
pub fn glyph_sprites() -> Vec<(String, Vec<u8>)> {
    let mut g = Lcg(0x6c79_7068);
    (0..GLYPH_COUNT)
        .map(|k| {
            let count = 2 + (g.unit() * 3.0) as usize;
            let mut strokes: Vec<&Stroke> = Vec::with_capacity(count);
            while strokes.len() < count {
                let s = &STROKES[(g.unit() * STROKES.len() as f32) as usize % STROKES.len()];
                if !strokes.iter().any(|t| std::ptr::eq(*t, s)) {
                    strokes.push(s);
                }
            }
            let mirror = g.unit() < 0.5;
            let n = GLYPH_SIDE;
            let mut rgba = Vec::with_capacity((n * n * 4) as usize);
            for j in 0..n {
                for i in 0..n {
                    let x = (i as f32 + 0.5) / n as f32;
                    let x = if mirror { 1.0 - x } else { x };
                    let y = (j as f32 + 0.5) / n as f32;
                    let d = strokes
                        .iter()
                        .map(|s| distance(s, x, y))
                        .fold(f32::MAX, f32::min);
                    // A stroke two texels wide with a soft edge of one.
                    let a = ((0.075 - d) / 0.035).clamp(0.0, 1.0);
                    rgba.extend_from_slice(&[255, 255, 255, (a * 255.0).round() as u8]);
                }
            }
            (glyph_name(k), crate::sprites::png(n, n, &rgba))
        })
        .collect()
}

/// The sheets of rain: how far apart they stand along the camera's way, how many are up at a time, the columns across
/// each and the cell a glyph takes.
const SHEET_GAP: f32 = 8.0;
const SHEETS: usize = 6;
const COLUMNS: usize = 44;
const PITCH: f32 = CELL * 1.3;
/// How close a sheet comes to the camera before it stands up again ahead.
const CELL: f32 = 0.5;
const NEAREST: f32 = 3.5;
const TOP: f32 = 9.0;
const BOTTOM: f32 = -9.0;
/// The camera's height over the stage and how fast it drifts forward.
const HEIGHT: f32 = 60.0;
const DRIFT: f32 = 0.7;
/// How many animated glyph sequences a trail picks from, and how many glyphs each runs through.
const SEQUENCES: usize = 16;
const SEQUENCE: usize = 8;

/// One column: where its head is (None while it waits at the top), how fast it falls, and when it goes again.
struct Column {
    x: f32,
    head: Option<f32>,
    speed: f32,
    next: f32,
    /// The last cell the head left a glyph in.
    cell: i32,
}

struct Sheet {
    y: f32,
    columns: Vec<Column>,
}

impl Sheet {
    fn new(y: f32, g: &mut Lcg) -> Sheet {
        let columns = (0..COLUMNS)
            .map(|k| Column {
                x: (k as f32 - (COLUMNS as f32 - 1.0) / 2.0) * PITCH,
                head: None,
                speed: 0.0,
                next: g.range(0.0, 8.0),
                cell: i32::MIN,
            })
            .collect();
        Sheet { y, columns }
    }
}

pub struct Glyphs {
    nodes: Vec<Node>,
    head: usize,
    trail: usize,
    sheets: Vec<Sheet>,
    lcg: Lcg,
}

impl Glyphs {
    pub fn new() -> Glyphs {
        let mut lcg = Lcg(0x7261_696e);
        let still = EmitterData {
            velocity: 0.0,
            velocity_variance: 0.0,
            theta_max: 0.0,
            ..emitter()
        };
        let head = ParticleData {
            lifetime_ms: 260,
            colors: [
                [0.9, 1.0, 0.92, 1.0],
                [0.85, 1.0, 0.9, 1.0],
                [0.6, 1.0, 0.7, 0.9],
                [0.3, 1.0, 0.45, 0.7],
            ],
            sizes: [CELL * 1.1; 4],
            times: [0.0, 0.4, 0.8, 1.0],
            animate: true,
            fps: 20,
            textures: (0..6).map(|_| random_glyph(&mut lcg)).collect(),
            ..particle(&glyph_name(0))
        };
        // The trails: each glyph runs through its own sequence of signs as it fades.
        let trail = ParticleData {
            lifetime_ms: 2600,
            lifetime_variance_ms: 700,
            colors: [
                [0.3, 1.0, 0.45, 0.8],
                [0.2, 0.95, 0.35, 0.5],
                [0.1, 0.7, 0.25, 0.18],
                [0.05, 0.4, 0.12, 0.0],
            ],
            sizes: [CELL; 4],
            times: [0.0, 0.2, 0.7, 1.0],
            animate: true,
            fps: 4,
            ..particle(&glyph_name(0))
        };
        let trails = (0..SEQUENCES)
            .map(|_| {
                plain(ParticleData {
                    textures: (0..SEQUENCE).map(|_| random_glyph(&mut lcg)).collect(),
                    ..trail.clone()
                })
            })
            .collect();
        let mut nodes = Vec::new();
        let head = one(&mut nodes, still.clone(), vec![plain(head)]);
        let trail = one(&mut nodes, still, trails);
        let sheets = (0..SHEETS)
            .map(|k| Sheet::new(NEAREST + 2.0 + SHEET_GAP * k as f32, &mut lcg))
            .collect();
        Glyphs {
            nodes,
            head,
            trail,
            sheets,
            lcg,
        }
    }

    /// Where the camera is at `t`: drifting forward, swaying a little.
    fn camera(t: f32) -> Vec3 {
        Vec3::new(
            0.5 * (0.11 * t).sin(),
            DRIFT * t,
            HEIGHT + 0.3 * (0.17 * t).sin(),
        )
    }
}

fn random_glyph(g: &mut Lcg) -> String {
    glyph_name((g.unit() * GLYPH_COUNT as f32) as usize % GLYPH_COUNT)
}

impl Effect for Glyphs {
    fn setting(&self) -> Setting {
        let eye = Glyphs::camera(0.0);
        Setting {
            sun: ([0.0, 0.0, 1.0], [0.0; 3], [0.0; 3]),
            clear: [0.0, 0.003, 0.001],
            ..Setting::new(eye.to_array(), (eye + Vec3::Y * 10.0).to_array(), 60)
        }
    }

    fn step(&mut self, t: f32, ms: u32, rng: &mut Rng) {
        let dt = ms as f32 / 1000.0;
        let cam_y = Glyphs::camera(t).y;
        let farthest = self.sheets.iter().map(|s| s.y).fold(f32::MIN, f32::max);
        for sheet in &mut self.sheets {
            // A sheet the camera has passed stands up again ahead of the farthest.
            if sheet.y < cam_y + NEAREST {
                *sheet = Sheet::new(farthest + SHEET_GAP, &mut self.lcg);
                for c in &mut sheet.columns {
                    c.next += t;
                }
            }
            for c in &mut sheet.columns {
                let Some(z) = &mut c.head else {
                    if t >= c.next {
                        c.head = Some(TOP);
                        c.speed = 3.0 + 5.0 * rng.f();
                        c.cell = i32::MIN;
                    }
                    continue;
                };
                *z -= c.speed * dt;
                if *z < BOTTOM {
                    c.head = None;
                    c.next = t + 1.5 + 7.5 * rng.f();
                    continue;
                }
                // Each cell the head crosses gets its glyph: the head's bright one and the trail's behind it.
                let cell = (*z / CELL).floor() as i32;
                if cell == c.cell {
                    continue;
                }
                c.cell = cell;
                let at = Vec3::new(c.x, sheet.y, HEIGHT + (cell as f32 + 0.5) * CELL);
                self.nodes[self.head].burst(at, Vec3::Z, 1, rng);
                self.nodes[self.trail].burst(at, Vec3::Z, 1, rng);
            }
        }
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    fn lights(&self, _t: f32, _frame: u32) -> Vec<HdLight> {
        Vec::new()
    }

    fn eye(&self, t: f32) -> Option<Eye> {
        let at = Glyphs::camera(t);
        let ahead = at + Vec3::new(0.3 * (0.09 * t).sin(), 10.0, 0.0);
        let mut eye = look_at(at.to_array(), ahead.to_array());
        eye.fov = Some(60);
        Some(eye)
    }
}
