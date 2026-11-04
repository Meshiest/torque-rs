//! The screen's marks over the world: hurt, a red edge that flashes and fades and a mark around the crosshair toward where the
//! shot came from; a hit, an X over the crosshair.
//!
//! The scripts count the hurts and hits (`$Arena::HurtCount`, `$Arena::Hits`) and keep where the last hurt came from
//! (`$Arena::HurtFrom`, the attacker's feet); [`ScreenMarks::update`] starts a mark at each new count and ages it, and
//! [`ScreenMarks::batches`] draws them as GUI quads over the world's rectangle, under the canvas's controls. The textures are made
//! here (a soft red edge, a soft dot, an X).

use std::sync::Arc;

use torque::gui::draw::{GuiBatch, GuiBlend, GuiDraws, GuiQuad};
use torque::gui::textures::{GuiTexture, texture_of};
use torque::images::{Bitmap, Format};

/// How long the red edge lasts after a hurt (seconds), and how strong it starts.
pub const HURT_SECONDS: f32 = 0.8;
pub const HURT_ALPHA: f32 = 0.75;
/// How long the mark toward the attacker lasts (seconds).
pub const DIRECTION_SECONDS: f32 = 1.2;
/// The mark's distance from the crosshair (a fraction of the world's shorter side), its dots, its spread (radians) and the
/// dots' size (pixels).
pub const DIRECTION_RADIUS: f32 = 0.16;
pub const DIRECTION_DOTS: usize = 9;
pub const DIRECTION_SPREAD: f32 = 0.5;
pub const DIRECTION_DOT: f32 = 14.0;
/// How long the X lasts after a hit (seconds), and its size (pixels).
pub const HIT_SECONDS: f32 = 0.3;
pub const HIT_SIZE: f32 = 30.0;

/// A hurt being shown: its age and the attacker's feet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hurt {
    pub age: f32,
    pub from: Option<[f32; 3]>,
}

/// The marks and the counts last seen.
#[derive(Debug, Default)]
pub struct ScreenMarks {
    pub hurt: Option<Hurt>,
    /// The X's age.
    pub hit: Option<f32>,
    hurts: Option<i64>,
    hits: Option<i64>,
    textures: Option<[Arc<GuiTexture>; 3]>,
}

/// `value` of a script global as an integer count.
pub fn count(value: &str) -> i64 {
    value.trim().parse().unwrap_or(0)
}

/// A point's three words (`"x y z"`).
pub fn point(value: &str) -> Option<[f32; 3]> {
    let v: Vec<f32> = value
        .split_whitespace()
        .filter_map(|w| w.parse().ok())
        .collect();
    (v.len() == 3 && v.iter().all(|x| x.is_finite())).then(|| [v[0], v[1], v[2]])
}

impl ScreenMarks {
    /// The frame: `hurts` and `hits` as the scripts count them now (a new count starts a mark; the first seen starts none),
    /// `from` where the last hurt came from, the marks `dt` seconds older.
    pub fn update(&mut self, hurts: i64, from: Option<[f32; 3]>, hits: i64, dt: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        if let Some(h) = self.hurt.as_mut() {
            h.age += dt;
        }
        if let Some(h) = self.hit.as_mut() {
            *h += dt;
        }
        if self.hurts.is_some_and(|c| c != hurts) {
            self.hurt = Some(Hurt { age: 0.0, from });
        }
        if self.hits.is_some_and(|c| c != hits) {
            self.hit = Some(0.0);
        }
        self.hurts = Some(hurts);
        self.hits = Some(hits);
        if self
            .hurt
            .is_some_and(|h| h.age > HURT_SECONDS.max(DIRECTION_SECONDS))
        {
            self.hurt = None;
        }
        if self.hit.is_some_and(|h| h > HIT_SECONDS) {
            self.hit = None;
        }
    }

    /// Forgets every mark (the game left).
    pub fn clear(&mut self) {
        self.hurt = None;
        self.hit = None;
    }

    /// The red edge's strength now (0 to 1).
    pub fn edge_alpha(&self) -> f32 {
        self.hurt.map_or(0.0, |h| {
            HURT_ALPHA * (1.0 - h.age / HURT_SECONDS).clamp(0.0, 1.0).powi(2)
        })
    }

    /// The X's strength now (0 to 1).
    pub fn hit_alpha(&self) -> f32 {
        self.hit
            .map_or(0.0, |h| (1.0 - h / HIT_SECONDS).clamp(0.0, 1.0))
    }

    /// Which way the last hurt came from as the screen shows it (radians, 0 straight ahead, positive to the right), seen from
    /// `eye` turned to `yaw` (a figure of yaw `a` faces `(sin a, cos a)`), and the mark's strength; none without a hurt or a
    /// place it came from.
    pub fn direction(&self, eye: [f32; 3], yaw: f32) -> Option<(f32, f32)> {
        let h = self.hurt?;
        let from = h.from?;
        let (dx, dy) = (from[0] - eye[0], from[1] - eye[1]);
        if dx * dx + dy * dy < 1e-6 {
            return None;
        }
        let toward = dx.atan2(dy) - yaw;
        let angle = (toward + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        let alpha = (1.0 - h.age / DIRECTION_SECONDS).clamp(0.0, 1.0);
        (alpha > 0.0).then_some((angle, alpha))
    }

    fn textures(&mut self) -> Option<[Arc<GuiTexture>; 3]> {
        if self.textures.is_none() {
            self.textures = Some([
                texture_of("arena:edge", &edge_bitmap(), false, true)?,
                texture_of("arena:dot", &dot_bitmap(), false, true)?,
                texture_of("arena:cross", &cross_bitmap(), false, true)?,
            ]);
        }
        self.textures.clone()
    }

    /// The marks as GUI quads over the world's rectangle `rect` (x, y, width, height), seen from `eye` turned to `yaw`.
    pub fn batches(&mut self, rect: [i32; 4], eye: [f32; 3], yaw: f32) -> Vec<GuiBatch> {
        let mut out = Vec::new();
        let edge = self.edge_alpha();
        let direction = self.direction(eye, yaw);
        let hit = self.hit_alpha();
        if edge <= 0.0 && direction.is_none() && hit <= 0.0 {
            return out;
        }
        let Some([edge_tex, dot_tex, cross_tex]) = self.textures() else {
            return out;
        };
        let [x, y, w, h] = rect.map(|v| v as f32);
        let (cx, cy) = (x + w * 0.5, y + h * 0.5);
        let batch = |texture: &Arc<GuiTexture>, quads: Vec<GuiQuad>| GuiBatch {
            clip: rect,
            texture: Some(Arc::clone(texture)),
            blend: GuiBlend::Alpha,
            quads,
        };
        let full_uv = |t: &GuiTexture| {
            [
                0.0,
                0.0,
                t.bitmap_size.0 as f32 / t.size.0 as f32,
                t.bitmap_size.1 as f32 / t.size.1 as f32,
            ]
        };
        let alpha = |a: f32| (a.clamp(0.0, 1.0) * 255.0).round() as u8;
        if edge > 0.0 {
            out.push(batch(
                &edge_tex,
                vec![GuiQuad {
                    rect: [x, y, x + w, y + h],
                    uv: full_uv(&edge_tex),
                    color: [255, 255, 255, alpha(edge)],
                }],
            ));
        }
        if let Some((angle, a)) = direction {
            let radius = w.min(h) * DIRECTION_RADIUS;
            let quads = (0..DIRECTION_DOTS)
                .map(|i| {
                    let t = i as f32 / (DIRECTION_DOTS - 1) as f32 - 0.5;
                    let at = angle + t * DIRECTION_SPREAD;
                    let (px, py) = (cx + radius * at.sin(), cy - radius * at.cos());
                    // Brightest in the middle of the arc.
                    let fade = 1.0 - (2.0 * t).abs() * 0.6;
                    let half = DIRECTION_DOT * 0.5;
                    GuiQuad {
                        rect: [px - half, py - half, px + half, py + half],
                        uv: full_uv(&dot_tex),
                        color: [235, 40, 30, alpha(a * fade)],
                    }
                })
                .collect();
            out.push(batch(&dot_tex, quads));
        }
        if hit > 0.0 {
            let half = HIT_SIZE * 0.5;
            out.push(batch(
                &cross_tex,
                vec![GuiQuad {
                    rect: [cx - half, cy - half, cx + half, cy + half],
                    uv: full_uv(&cross_tex),
                    color: [255, 255, 255, alpha(hit)],
                }],
            ));
        }
        out
    }
}

/// Puts `batches` into `draws` just after the world's frame (under the canvas's own controls), moving the later worlds and
/// extras on.
pub fn insert_over_world(draws: &mut GuiDraws, batches: Vec<GuiBatch>) {
    let Some(at) = draws.worlds.first().map(|w| w.at_batch) else {
        return;
    };
    let at = at.min(draws.batches.len());
    let n = batches.len();
    if n == 0 {
        return;
    }
    draws.batches.splice(at..at, batches);
    for w in draws.worlds.iter_mut().skip(1) {
        if w.at_batch > at {
            w.at_batch += n;
        }
    }
    for e in &mut draws.extras {
        if e.at_batch > at {
            e.at_batch += n;
        }
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn rgba(side: u32, texel: impl Fn(f32, f32) -> [u8; 4]) -> Bitmap {
    let mut bits = Vec::with_capacity((side * side * 4) as usize);
    for j in 0..side {
        for i in 0..side {
            // From -1 to 1 across, at the texel's middle.
            let u = (i as f32 + 0.5) / side as f32 * 2.0 - 1.0;
            let v = (j as f32 + 0.5) / side as f32 * 2.0 - 1.0;
            bits.extend_from_slice(&texel(u, v));
        }
    }
    Bitmap {
        width: side,
        height: side,
        format: Format::Rgba,
        bits,
    }
}

/// A red edge: clear in the middle, thickening toward the corners.
fn edge_bitmap() -> Bitmap {
    rgba(128, |u, v| {
        let r = (u * u + v * v).sqrt() / std::f32::consts::SQRT_2;
        let a = smoothstep(0.45, 1.0, r);
        [200, 10, 10, (a * 255.0).round() as u8]
    })
}

/// A soft white dot.
fn dot_bitmap() -> Bitmap {
    rgba(32, |u, v| {
        let a = 1.0 - smoothstep(0.55, 1.0, (u * u + v * v).sqrt());
        [255, 255, 255, (a * 255.0).round() as u8]
    })
}

/// A white X with an open middle, its arms soft-edged.
fn cross_bitmap() -> Bitmap {
    rgba(64, |u, v| {
        let d = (u - v).abs().min((u + v).abs()) / std::f32::consts::SQRT_2;
        let r = (u * u + v * v).sqrt();
        let arm = 1.0 - smoothstep(0.06, 0.12, d);
        let length = smoothstep(0.3, 0.38, r) * (1.0 - smoothstep(0.92, 1.0, r));
        [255, 255, 255, (arm * length * 255.0).round() as u8]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hurt_flashes_the_edge_and_points_at_the_attacker_then_fades() {
        let mut m = ScreenMarks::default();
        m.update(0, None, 0, 0.0);
        assert_eq!(m.edge_alpha(), 0.0, "the first count seen starts nothing");
        m.update(1, Some([10.0, 0.0, 0.0]), 0, 0.016);
        assert!((m.edge_alpha() - HURT_ALPHA).abs() < 1e-6);
        // Facing north, the attacker due east: a quarter turn to the right.
        let (angle, a) = m.direction([0.0, 0.0, 1.8], 0.0).unwrap();
        assert!((angle - std::f32::consts::FRAC_PI_2).abs() < 1e-5 && a > 0.99);
        // Turned to face it, the mark is straight ahead.
        let (angle, _) = m
            .direction([0.0, 0.0, 1.8], std::f32::consts::FRAC_PI_2)
            .unwrap();
        assert!(angle.abs() < 1e-5);
        let quads: usize = m
            .batches([0, 0, 800, 600], [0.0; 3], 0.0)
            .iter()
            .map(|b| b.quads.len())
            .sum();
        assert_eq!(quads, 1 + DIRECTION_DOTS);
        m.update(1, None, 0, HURT_SECONDS);
        assert_eq!(m.edge_alpha(), 0.0);
        assert!(
            m.direction([0.0; 3], 0.0).is_some(),
            "the mark outlasts the edge"
        );
        m.update(1, None, 0, DIRECTION_SECONDS);
        assert!(m.hurt.is_none());
        assert!(m.batches([0, 0, 800, 600], [0.0; 3], 0.0).is_empty());
    }

    #[test]
    fn a_hit_shows_the_x_for_a_moment() {
        let mut m = ScreenMarks::default();
        m.update(0, None, 3, 0.0);
        m.update(0, None, 4, 0.016);
        assert_eq!(m.hit_alpha(), 1.0);
        let b = m.batches([0, 0, 800, 600], [0.0; 3], 0.0);
        assert_eq!(b.len(), 1);
        assert_eq!(
            b[0].quads[0].rect,
            [385.0, 285.0, 415.0, 315.0],
            "centered on the crosshair"
        );
        m.update(0, None, 4, HIT_SECONDS + 0.01);
        assert_eq!(m.hit_alpha(), 0.0);
    }

    #[test]
    fn the_marks_go_just_over_the_world() {
        let mut d = GuiDraws::default();
        let quad = GuiQuad {
            rect: [0.0; 4],
            uv: [0.0; 4],
            color: [0; 4],
        };
        let b = |n: usize| GuiBatch {
            clip: [0; 4],
            texture: None,
            blend: GuiBlend::Alpha,
            quads: vec![quad; n],
        };
        d.batches = vec![b(1), b(2)];
        d.worlds.push(torque::gui::draw::WorldDraw {
            at_batch: 1,
            rect: [0; 4],
            control: [0; 4],
        });
        insert_over_world(&mut d, vec![b(3)]);
        assert_eq!(
            d.batches.iter().map(|b| b.quads.len()).collect::<Vec<_>>(),
            [1, 3, 2]
        );
    }
}
