//! The frame rate, as TGE's `FPS::update` measures it, once a main loop pass after the canvas draws. The real seconds (the
//! system's millisecond tick / 1000) and the virtual seconds (the simulation's milliseconds / 1000) between passes are averaged
//! (`avg = 0.07 dt + 0.93 avg`, from the second pass), and every so often `$fps::real` and `$fps::virtual` are set to `1 / avg` as
//! `%4.1f`: the first time 0.75 s after the first pass, then when the real time is half a second past the next due time (0.25 s
//! after the last, or 0.25 s later when on time). A `metrics(fps)` script shows them (" FPS: " `$fps::real` "  mspf: "
//! `1000 / $fps::real`) in a frame overlay.
//!
//! A game's own counter can read the same average every frame. Only a windowed client runs this: a headless run's frames are not
//! a player's.
//!
//! Credit: follows TGE's frame-rate tracker as Torque3D carries it (MIT: `util/fpsTracker.cpp`, Copyright (c) 2012 GarageGames,
//! LLC).

/// The average's weight, the update step and the wait past it.
pub const ALPHA: f32 = 0.07;
pub const STEP: f32 = 0.25;
pub const LATE: f32 = 0.5;

/// `FPS`'s state.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Fps {
    started: bool,
    frames: f32,
    /// The averaged real and virtual seconds a pass.
    pub real: f32,
    pub virt: f32,
    last_real: f32,
    last_virt: f32,
    next: f32,
}

impl Fps {
    /// One pass at real time `real_s` and virtual time `virt_s` (seconds): Some((`$fps::real`, `$fps::virtual`)) when they are due.
    pub fn update(&mut self, real_s: f32, virt_s: f32) -> Option<(String, String)> {
        if !self.started {
            *self = Fps {
                started: true,
                next: real_s + STEP,
                ..Fps::default()
            };
        }
        self.frames += 1.0;
        if self.frames > 1.0 {
            self.real = (real_s - self.last_real) * ALPHA + self.real * (1.0 - ALPHA);
            self.virt = (virt_s - self.last_virt) * ALPHA + self.virt * (1.0 - ALPHA);
        }
        self.last_real = real_s;
        self.last_virt = virt_s;
        let late = real_s - self.next;
        if late <= LATE {
            return None;
        }
        let out = (
            format!("{:4.1}", 1.0 / self.real),
            format!("{:4.1}", 1.0 / self.virt),
        );
        if late <= STEP {
            self.next += STEP;
        } else {
            self.next = self.last_real + STEP;
        }
        Some(out)
    }

    /// The engine's counter: frames a second and milliseconds a frame from the real average (None before two frames).
    pub fn text(&self) -> Option<String> {
        (self.frames > 1.0 && self.real > 0.0)
            .then(|| format!("{:.0} fps  {:.1} ms", 1.0 / self.real, self.real * 1000.0))
    }
}

#[cfg(test)]
#[path = "fps_tests.rs"]
mod tests;
