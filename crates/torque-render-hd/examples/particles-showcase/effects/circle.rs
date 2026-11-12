//! A magic circle traced on the floor: nine points of light draw its parts at once over nine seconds (the two outer rings
//! from the top both ways, the inner ring, the seven-pointed star in two halves, the runes of the band, the heart last), each
//! throwing sparks along its way, the lines white-hot where just laid and cooling to blue. The figure itself is a glowing
//! decal (ribbons along the strokes, added over the floor into the bloom's range), not particles. Whole, it flashes, its
//! rings turn against each other, motes rise from its lines, sparks spurt at its rim and runes flare one by one.

use std::f32::consts::{PI, TAU};
use std::ops::Range;

use glam::{Vec2, Vec3};
use torque_render_scene::hd::HdLight;
use torque_render_scene::particles::{EmitterData, ParticleData, ParticleExtras, Rng};
use torque_render_scene::textures::TextureSet;
use torque_render_scene::{Draw, Vertex};

use super::{Effect, Node, Setting, emitter, hash, light, one, particle, plain, smooth};
use crate::scenery::{self, ribbon, vertex};
use crate::sprites::{DISC, GLOW, GLYPH, LINE, MOTE, SPARK};

/// When the tracing starts and ends (seconds), and the figure's center.
const TRACE_START: f32 = 0.5;
const TRACE_END: f32 = 9.5;
const CENTER: Vec3 = Vec3::new(0.0, 0.0, 0.02);

/// The figure's radii: the two outer rings, the rune band (inner and outer radius), the inner ring the star stands in, and
/// the heart's ring and triangle.
const OUTER: f32 = 2.1;
const OUTER_2: f32 = 2.22;
const BAND: Range<f32> = 1.68..2.0;
const INNER: f32 = 1.6;
const HEART: f32 = 0.7;
const TRIANGLE: f32 = 0.56;
const RUNES: usize = 12;

/// A part of the figure: which way it turns once the figure is whole.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Outer,
    Inner,
    Heart,
    Rune(usize),
}

impl Part {
    /// The part's turn at `t` (radians): still while traced, then the outer parts one way and the inner ones the other,
    /// starting gently.
    fn turn(self, t: f32) -> f32 {
        let after = (t - TRACE_END).max(0.0);
        let ramp = after - (1.0 - (-after).exp());
        match self {
            Part::Outer | Part::Rune(_) => 0.2 * ramp,
            Part::Inner => -0.3 * ramp,
            Part::Heart => 0.0,
        }
    }

    fn world(self, p: Vec2, t: f32) -> Vec3 {
        let (s, c) = self.turn(t).sin_cos();
        CENTER + Vec3::new(p.x * c - p.y * s, p.x * s + p.y * c, 0.0)
    }
}

struct Stroke {
    points: Vec<Vec2>,
    part: Part,
    /// How far its ribbon runs past its ends and sharp corners (an arc's ends meet another's: none).
    cap: f32,
}

/// A glyph's cap: about the glowing core's half width.
const CAP: f32 = 0.03;

/// An arc of radius `r` from angle `from` sweeping `sweep` (signed) in `n` steps.
fn arc(r: f32, from: f32, sweep: f32, n: usize) -> Vec<Vec2> {
    (0..=n)
        .map(|k| {
            let a = from + sweep * k as f32 / n as f32;
            Vec2::new(a.cos(), a.sin()) * r
        })
        .collect()
}

/// A rune's strokes on a 3 by 3 grid (x right, y up).
const GLYPHS: [&[&[(u8, u8)]]; RUNES] = [
    &[&[(1, 0), (1, 2)], &[(1, 2), (2, 1)], &[(1, 1), (2, 0)]],
    &[&[(0, 0), (0, 2)], &[(2, 0), (2, 2)], &[(0, 1), (2, 1)]],
    &[&[(0, 0), (1, 2), (2, 0)]],
    &[&[(0, 0), (2, 2)], &[(0, 2), (2, 0)]],
    &[&[(1, 0), (1, 2)], &[(0, 1), (1, 2), (2, 1)]],
    &[&[(0, 0), (0, 2), (2, 0), (2, 2)]],
    &[&[(0, 2), (2, 2), (1, 0), (0, 2)]],
    &[&[(1, 0), (1, 2)], &[(0, 0), (2, 0)], &[(0, 2), (2, 2)]],
    &[&[(0, 0), (1, 1), (0, 2)], &[(1, 1), (2, 1)]],
    &[&[(0, 0), (2, 0), (2, 2), (0, 2), (0, 0)]],
    &[&[(1, 0), (0, 1), (1, 2), (2, 1), (1, 0)]],
    &[&[(0, 2), (0, 0), (2, 0)], &[(2, 0), (2, 1)]],
];

/// Rune `k`'s strokes in the band, its up outward.
fn rune(k: usize) -> Vec<Stroke> {
    let a0 = TAU * k as f32 / RUNES as f32 + TAU / (2 * RUNES) as f32;
    let width = 0.26 / BAND.start;
    GLYPHS[k]
        .iter()
        .map(|stroke| Stroke {
            points: stroke
                .iter()
                .map(|&(u, v)| {
                    let a = a0 - (f32::from(u) / 2.0 - 0.5) * width;
                    let r = BAND.start + f32::from(v) / 2.0 * (BAND.end - BAND.start);
                    Vec2::new(a.cos(), a.sin()) * r
                })
                .collect(),
            part: Part::Rune(k),
            cap: CAP,
        })
        .collect()
}

/// The star's point `k` ({7/3}: every third point of seven on the inner ring).
fn star_point(k: usize) -> Vec2 {
    let a = PI / 2.0 + TAU * ((k * 3) % 7) as f32 / 7.0;
    Vec2::new(a.cos(), a.sin()) * INNER
}

/// A tracer: the strokes it draws in order, when it starts and ends, its spark and flash emitters, and where it was.
struct Tracer {
    strokes: Vec<Stroke>,
    /// Per stroke, per point: the length drawn up to it, and when it is laid.
    lengths: Vec<Vec<f32>>,
    laid: Vec<Vec<f32>>,
    total: f32,
    start: f32,
    end: f32,
    sparks: usize,
    flash: usize,
    last_stroke: Option<usize>,
    done: bool,
}

impl Tracer {
    fn new(strokes: Vec<Stroke>, start: f32, end: f32, sparks: usize, flash: usize) -> Tracer {
        let mut total = 0.0;
        let lengths: Vec<Vec<f32>> = strokes
            .iter()
            .map(|s| {
                let mut out = vec![total];
                for w in s.points.windows(2) {
                    total += w[0].distance(w[1]);
                    out.push(total);
                }
                out
            })
            .collect();
        let mut tracer = Tracer {
            strokes,
            lengths,
            laid: Vec::new(),
            total,
            start,
            end,
            sparks,
            flash,
            last_stroke: None,
            done: false,
        };
        tracer.laid = tracer
            .lengths
            .iter()
            .map(|ls| ls.iter().map(|&s| tracer.time_of(s)).collect())
            .collect();
        tracer
    }

    /// The fraction drawn at `t`: mostly even, a little slower at the ends.
    fn fraction(&self, t: f32) -> f32 {
        let f = ((t - self.start) / (self.end - self.start)).clamp(0.0, 1.0);
        0.85 * f + 0.15 * smooth(f)
    }

    fn drawn(&self, t: f32) -> f32 {
        self.fraction(t) * self.total
    }

    /// When length `s` is laid (the inverse of `drawn`, by bisection).
    fn time_of(&self, s: f32) -> f32 {
        let (mut lo, mut hi) = (self.start, self.end);
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            if self.drawn(mid) < s {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }

    fn active(&self, t: f32) -> bool {
        (self.start..self.end).contains(&t)
    }

    /// The point, the direction and the stroke at length `s` along the tracer's path.
    fn at(&self, s: f32) -> (Vec2, Vec2, usize) {
        for (k, (stroke, ls)) in self.strokes.iter().zip(&self.lengths).enumerate() {
            let Some(last) = ls.last() else { continue };
            if s > *last && k + 1 < self.strokes.len() {
                continue;
            }
            let i = ls.partition_point(|&l| l <= s).clamp(1, ls.len() - 1);
            let (a, b) = (stroke.points[i - 1], stroke.points[i]);
            let span = (ls[i] - ls[i - 1]).max(1e-6);
            let f = ((s - ls[i - 1]) / span).clamp(0.0, 1.0);
            return (a.lerp(b, f), (b - a).normalize_or(Vec2::X), k);
        }
        (Vec2::ZERO, Vec2::X, 0)
    }

    /// The tip at `t` (world point and direction), while tracing.
    fn tip(&self, t: f32) -> Option<(Vec3, Vec3, usize)> {
        if !self.active(t) {
            return None;
        }
        let (p, d, k) = self.at(self.drawn(t));
        let part = self.strokes[k].part;
        Some((part.world(p, t), Vec3::new(d.x, d.y, 0.0), k))
    }

    /// The ribbons of what is drawn at `t`, colored by how long ago each point was laid: the rings and the star into `out`
    /// (a line in its glow), the runes into `runes` (narrower, in a smaller glow that stays within a small glyph).
    fn mesh(
        &self,
        t: f32,
        level: f32,
        flare: &dyn Fn(Part) -> f32,
        out: &mut Vec<Vertex>,
        runes: &mut Vec<Vertex>,
    ) {
        let drawn = self.drawn(t);
        let mut points = Vec::new();
        let mut colors = Vec::new();
        for ((stroke, ls), laid) in self.strokes.iter().zip(&self.lengths).zip(&self.laid) {
            if ls[0] >= drawn {
                break;
            }
            let bright = level * flare(stroke.part);
            points.clear();
            colors.clear();
            for (i, &p) in stroke.points.iter().enumerate() {
                if ls[i] <= drawn {
                    points.push(stroke.part.world(p, t));
                    colors.push(line_color(t - laid[i], bright));
                } else {
                    // The pen is on this segment: its end at the pen.
                    let span = (ls[i] - ls[i - 1]).max(1e-6);
                    let f = ((drawn - ls[i - 1]) / span).clamp(0.0, 1.0);
                    points.push(stroke.part.world(stroke.points[i - 1].lerp(p, f), t));
                    colors.push(line_color(0.0, bright));
                    break;
                }
            }
            match stroke.part {
                Part::Rune(_) => ribbon(&points, &colors, 0.18, stroke.cap, Vec3::Z, runes),
                _ => ribbon(&points, &colors, 0.3, stroke.cap, Vec3::Z, out),
            }
        }
    }
}

/// A line's color `age` seconds after it was laid: white-hot, cooling to blue over a second, at `bright`.
fn line_color(age: f32, bright: f32) -> [u8; 4] {
    let hot = (-age / 0.8).exp();
    let steady = [0.28, 0.7, 1.0];
    let c = steady.map(|s| (s + (1.0 - s) * hot) * bright);
    [
        (c[0] * 255.0).min(255.0) as u8,
        (c[1] * 255.0).min(255.0) as u8,
        (c[2] * 255.0).min(255.0) as u8,
        255,
    ]
}

pub struct Circle {
    /// When the tracing's clock started (the window traces again).
    t0: f32,
    nodes: Vec<Node>,
    tracers: Vec<Tracer>,
    /// The whole figure as one path (where the motes rise from).
    figure: Tracer,
    rise: usize,
    rim: usize,
    burst: usize,
    flare: usize,
    /// The runes flared so far: the rune and when.
    flares: Vec<(usize, f32)>,
    rim_due: f32,
    flare_due: f32,
    burst_done: bool,
}

impl Circle {
    pub fn new() -> Circle {
        let spark = ParticleData {
            drag: 0.4,
            gravity: 0.7,
            lifetime_ms: 700,
            lifetime_variance_ms: 300,
            colors: [
                [1.0, 1.0, 0.95, 1.0],
                [0.7, 0.95, 1.0, 1.0],
                [0.3, 0.6, 1.0, 0.7],
                [0.2, 0.3, 1.0, 0.0],
            ],
            sizes: [0.035, 0.03, 0.025, 0.015],
            times: [0.0, 0.15, 0.6, 1.0],
            ..particle(SPARK)
        };
        let streak = ParticleExtras {
            size_by_velocity: 0.03,
            collide: true,
            bounce_elasticity: 0.3,
            bounce_friction: 0.4,
            ..ParticleExtras::default()
        };
        let flash = ParticleData {
            lifetime_ms: 90,
            colors: [
                [0.9, 1.0, 1.0, 0.7],
                [0.6, 0.9, 1.0, 0.5],
                [0.4, 0.7, 1.0, 0.2],
                [0.3, 0.5, 1.0, 0.0],
            ],
            sizes: [0.4, 0.55, 0.6, 0.65],
            ..particle(GLOW)
        };
        let rise = ParticleData {
            drag: 0.4,
            gravity: -0.04,
            lifetime_ms: 2600,
            lifetime_variance_ms: 800,
            colors: [
                [0.4, 0.9, 1.0, 0.0],
                [0.45, 0.9, 1.0, 0.6],
                [0.4, 0.6, 1.0, 0.35],
                [0.5, 0.4, 1.0, 0.0],
            ],
            sizes: [0.02, 0.03, 0.028, 0.015],
            ..particle(MOTE)
        };
        // A rune's flare: sparks sprayed over a wide cone, arcing down on gravity and curl, and glints that stay at the rune.
        let spray = ParticleData {
            drag: 0.5,
            gravity: 0.6,
            lifetime_ms: 750,
            lifetime_variance_ms: 450,
            sizes: [0.03, 0.028, 0.022, 0.012],
            ..spark.clone()
        };
        let glint = ParticleData {
            drag: 3.5,
            gravity: 0.0,
            lifetime_ms: 260,
            lifetime_variance_ms: 160,
            colors: [
                [1.0, 1.0, 1.0, 1.0],
                [0.8, 0.95, 1.0, 0.9],
                [0.5, 0.8, 1.0, 0.5],
                [0.3, 0.5, 1.0, 0.0],
            ],
            sizes: [0.045, 0.04, 0.03, 0.0],
            ..spark.clone()
        };
        let mut nodes = Vec::new();
        let flare = one(
            &mut nodes,
            EmitterData {
                velocity: 1.8,
                velocity_variance: 1.2,
                theta_max: 75.0,
                ..emitter()
            },
            vec![
                (
                    spray,
                    ParticleExtras {
                        size_by_velocity: 0.015,
                        turbulence: 1.5,
                        turbulence_scale: 0.5,
                        ..streak
                    },
                ),
                (glint, ParticleExtras::default()),
            ],
        );
        let tip = |nodes: &mut Vec<Node>| {
            let sparks = one(
                nodes,
                EmitterData {
                    period_ms: 1,
                    velocity: 2.4,
                    velocity_variance: 1.2,
                    theta_max: 28.0,
                    ..emitter()
                },
                vec![(spark.clone(), streak)],
            );
            let flash = one(
                nodes,
                EmitterData {
                    period_ms: 10,
                    velocity: 0.0,
                    velocity_variance: 0.0,
                    ..emitter()
                },
                vec![plain(flash.clone())],
            );
            (sparks, flash)
        };
        let ring = |r: f32, from: f32, sweep: f32| Stroke {
            points: arc(r, from, sweep, (80.0 * sweep.abs()) as usize),
            part: if r > INNER { Part::Outer } else { Part::Inner },
            cap: 0.0,
        };
        let star = |from: usize, to: usize| Stroke {
            points: (from..=to).map(star_point).collect(),
            part: Part::Inner,
            cap: CAP,
        };
        let mut tracers = Vec::new();
        let mut tracer = |strokes: Vec<Stroke>, start: f32, end: f32| {
            let (sparks, flash) = tip(&mut nodes);
            tracers.push(Tracer::new(strokes, start, end, sparks, flash));
        };
        // The outer rings: two tracers from the top, one each way, each half of the first ring then half of the second.
        for sign in [-1.0, 1.0] {
            tracer(
                vec![
                    ring(OUTER, PI / 2.0, sign * PI),
                    ring(OUTER_2, -PI / 2.0, sign * PI),
                ],
                TRACE_START,
                6.5,
            );
        }
        // The inner ring: two tracers from the bottom.
        for sign in [-1.0, 1.0] {
            tracer(vec![ring(INNER, -PI / 2.0, sign * PI)], 1.5, 7.0);
        }
        // The star: its seven edges in two runs.
        tracer(vec![star(0, 4)], 2.5, 8.5);
        tracer(vec![star(4, 7)], 2.5, 8.5);
        // The runes: two tracers, six each.
        for half in 0..2 {
            let strokes = (half * RUNES / 2..(half + 1) * RUNES / 2)
                .flat_map(rune)
                .collect();
            tracer(strokes, 3.0, TRACE_END);
        }
        // The heart: its ring, then the triangle.
        tracer(
            vec![
                Stroke {
                    points: arc(HEART, PI / 2.0, -TAU, 64),
                    part: Part::Heart,
                    cap: 0.0,
                },
                Stroke {
                    points: arc(TRIANGLE, -PI / 2.0, TAU, 3),
                    part: Part::Heart,
                    cap: CAP,
                },
            ],
            6.0,
            9.3,
        );
        let figure = Tracer::new(
            tracers
                .iter()
                .flat_map(|t| {
                    t.strokes.iter().map(|s| Stroke {
                        points: s.points.clone(),
                        part: s.part,
                        cap: s.cap,
                    })
                })
                .collect(),
            0.0,
            1.0,
            0,
            0,
        );
        let rise = one(
            &mut nodes,
            EmitterData {
                velocity: 0.4,
                velocity_variance: 0.25,
                theta_max: 20.0,
                ..emitter()
            },
            vec![(
                rise,
                ParticleExtras {
                    turbulence: 0.7,
                    turbulence_scale: 0.7,
                    ..ParticleExtras::default()
                },
            )],
        );
        let rim = one(
            &mut nodes,
            EmitterData {
                velocity: 2.6,
                velocity_variance: 1.2,
                theta_max: 25.0,
                ..emitter()
            },
            vec![(spark.clone(), streak)],
        );
        let burst = one(
            &mut nodes,
            EmitterData {
                velocity: 4.0,
                velocity_variance: 2.0,
                theta_max: 20.0,
                ..emitter()
            },
            vec![(
                ParticleData {
                    lifetime_ms: 1100,
                    lifetime_variance_ms: 400,
                    gravity: 0.5,
                    ..spark
                },
                streak,
            )],
        );
        Circle {
            t0: 0.0,
            nodes,
            tracers,
            figure,
            rise,
            rim,
            burst,
            flare,
            flares: Vec::new(),
            rim_due: 0.0,
            flare_due: 0.6,
            burst_done: false,
        }
    }

    /// How much of the figure is drawn at `t` (0..1).
    fn completion(&self, t: f32) -> f32 {
        let drawn: f32 = self.tracers.iter().map(|tr| tr.drawn(t)).sum();
        drawn / self.figure.total
    }

    /// The figure's glow at `t` (1: as traced): a flash when it closes, then a slow pulse.
    fn level(t: f32) -> f32 {
        if t < TRACE_END {
            return 0.9;
        }
        let after = t - TRACE_END;
        let pulse = 0.5 + 0.5 * (after * TAU * 0.5 - PI / 2.0).sin();
        0.85 + 0.2 * pulse + 1.3 * (-after * 2.2).exp()
    }

    /// A rune's extra glow at `t` from its flares.
    fn flare(&self, part: Part, t: f32) -> f32 {
        let Part::Rune(k) = part else { return 1.0 };
        1.0 + self
            .flares
            .iter()
            .filter(|(r, _)| *r == k)
            .map(|(_, t0)| 2.2 * (-(t - t0) / 0.9).exp())
            .sum::<f32>()
    }

    /// Rune `k`'s center in the world at `t`.
    fn rune_center(k: usize, t: f32) -> Vec3 {
        let a = TAU * k as f32 / RUNES as f32 + TAU / (2 * RUNES) as f32;
        let r = 0.5 * (BAND.start + BAND.end);
        Part::Rune(k).world(Vec2::new(a.cos(), a.sin()) * r, t)
    }
}

impl Effect for Circle {
    fn setting(&self) -> Setting {
        Setting::new([0.0, -6.8, 4.5], [0.0, 0.3, 0.0], 55)
    }

    fn step(&mut self, t: f32, ms: u32, rng: &mut Rng) {
        if t - self.t0 > TRACE_END + 14.0 {
            self.trigger(t, rng);
        }
        let t = t - self.t0;
        let dt = ms as f32 / 1000.0;
        for tr in &mut self.tracers {
            match tr.tip(t) {
                Some((at, along, stroke)) => {
                    // The pen jumped to another stroke: the sparks' path breaks there.
                    if tr.last_stroke.is_some_and(|k| k != stroke) {
                        self.nodes[tr.sparks].emitter.break_path();
                    }
                    tr.last_stroke = Some(stroke);
                    let axis = (along + Vec3::Z * 0.8).normalize();
                    self.nodes[tr.sparks].path(at, axis, ms, rng);
                    self.nodes[tr.flash].path(at + Vec3::Z * 0.03, Vec3::Z, ms, rng);
                }
                None => {
                    self.nodes[tr.sparks].emitter.break_path();
                    self.nodes[tr.flash].emitter.break_path();
                    if t >= tr.end && !tr.done {
                        // The tracer arrives: a last spurt straight up.
                        tr.done = true;
                        let (p, _, k) = tr.at(tr.total);
                        let at = tr.strokes[k].part.world(p, t);
                        self.nodes[self.burst].burst(at, Vec3::Z, 18, rng);
                    }
                }
            }
        }
        if t < TRACE_END {
            return;
        }
        if !self.burst_done {
            self.burst_done = true;
            for k in 0..600 {
                let a = TAU * k as f32 / 600.0;
                let out = Vec3::new(a.cos(), a.sin(), 0.0);
                self.nodes[self.burst].burst(
                    CENTER + out * OUTER_2,
                    (out * 0.6 + Vec3::Z).normalize(),
                    2,
                    rng,
                );
            }
        }
        // Motes rising from the lines.
        let want = 120.0 * dt;
        for _ in 0..want as u32 + u32::from(rng.f() < want.fract()) {
            let (p, _, k) = self.figure.at(rng.f() * self.figure.total);
            let at = self.figure.strokes[k].part.world(p, t);
            self.nodes[self.rise].burst(at, Vec3::Z, 1, rng);
        }
        // Spurts of sparks at the rim, along it.
        self.rim_due -= dt;
        while self.rim_due <= 0.0 {
            self.rim_due += 0.15 + 0.15 * rng.f();
            let a = rng.f() * TAU;
            let p = Vec2::new(a.cos(), a.sin()) * OUTER_2;
            let along = Vec3::new(-a.sin(), a.cos(), 0.0) * if rng.f() < 0.5 { 1.0 } else { -1.0 };
            self.nodes[self.rim].burst(
                Part::Outer.world(p, t),
                (along + Vec3::Z * 0.7).normalize(),
                12,
                rng,
            );
        }
        // A rune flares, spraying sparks about it.
        self.flare_due -= dt;
        while self.flare_due <= 0.0 {
            self.flare_due += 0.55 + 0.5 * rng.f();
            let k = (self.flares.len() * 7 + 3) % RUNES;
            self.flares.push((k, t));
            self.nodes[self.flare].burst(
                Circle::rune_center(k, t) + Vec3::Z * 0.03,
                Vec3::Z,
                26,
                rng,
            );
        }
        self.flares.retain(|(_, t0)| t - t0 < 4.0);
    }

    fn nodes(&mut self) -> &mut [Node] {
        &mut self.nodes
    }

    fn trigger(&mut self, t: f32, _rng: &mut Rng) {
        *self = Circle::new();
        self.t0 = t;
    }

    fn lights(&self, t: f32, frame: u32) -> Vec<HdLight> {
        let t = t - self.t0;
        let mut out: Vec<HdLight> = self
            .tracers
            .iter()
            .enumerate()
            .filter_map(|(k, tr)| tr.tip(t).map(|tip| (k, tip)))
            .map(|(k, (at, _, _))| {
                light(
                    at + Vec3::Z * 0.2,
                    [0.6, 1.4, 2.4],
                    0.6 + 0.25 * hash(frame + k as u32 * 97),
                    2.5,
                )
            })
            .collect();
        if t > TRACE_START {
            let k = if t < TRACE_END {
                0.1 + 0.4 * self.completion(t)
            } else {
                0.5 * Circle::level(t)
            };
            out.push(light(CENTER + Vec3::Z * 0.9, [0.4, 0.9, 1.8], k, 7.0));
        }
        out
    }

    fn decals(&self, t: f32, textures: &mut TextureSet<'_>) -> Vec<(f32, Draw)> {
        let t = t - self.t0;
        let level = Circle::level(t);
        let mut lines = Vec::new();
        let mut runes = Vec::new();
        let flare = |part: Part| self.flare(part, t);
        for tr in &self.tracers {
            tr.mesh(t, level * 0.55, &flare, &mut lines, &mut runes);
        }
        // A pool of light under the figure, growing with it.
        let pool = 0.5 * self.completion(t) * level;
        let c = [
            (0.1 * pool * 255.0) as u8,
            (0.3 * pool * 255.0) as u8,
            (0.8 * pool * 255.0).min(255.0) as u8,
            255,
        ];
        let r = 2.9;
        let corner = |x: f32, y: f32| {
            vertex(
                CENTER + Vec3::new(x * r, y * r, -0.01),
                Vec3::Z,
                c,
                [0.5 + 0.5 * x, 0.5 - 0.5 * y],
            )
        };
        let (a, b, cc, d) = (
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        );
        let (line, glyph, disc) = (textures.get(LINE), textures.get(GLYPH), textures.get(DISC));
        vec![
            (0.0, scenery::decal("figure", lines, line)),
            (0.0, scenery::decal("runes", runes, glyph)),
            (0.0, scenery::decal("pool", vec![a, b, cc, a, cc, d], disc)),
        ]
    }
}
