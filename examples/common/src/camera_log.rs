//! The camera's motion frame by frame, for the examples' camera clips: where each frame drew the eye, how fast it moved and
//! turned, and how far it was from the eye its mode asked for.
//!
//! [`FrameClock`] hands out the whole milliseconds of a frame rate the way a window's clock does (60 a second: 17, 17, 16, ...);
//! [`CameraLog`] keeps a [`CameraFrame`] a frame and writes them as CSV, a line a frame.

use std::fmt::Write as _;
use std::path::Path;

use torque_render_scene::source::Eye;

/// The milliseconds of each frame at a frame rate, carried so they add up to the rate's time.
#[derive(Debug, Clone, Copy)]
pub struct FrameClock {
    step: f64,
    done: f64,
    ms: u64,
}

impl FrameClock {
    /// Frames `fps` a second.
    pub fn new(fps: f32) -> FrameClock {
        FrameClock {
            step: 1000.0 / f64::from(fps.max(1.0)),
            done: 0.0,
            ms: 0,
        }
    }

    /// The next frame's milliseconds.
    pub fn next_ms(&mut self) -> u32 {
        self.done += self.step;
        let to = self.done.round() as u64;
        let ms = to - self.ms;
        self.ms = to;
        ms as u32
    }

    /// The clock's time so far (ms).
    pub fn now_ms(&self) -> u64 {
        self.ms
    }
}

/// One frame of the camera.
#[derive(Debug, Clone)]
pub struct CameraFrame {
    /// The clock (seconds) and the frame's time.
    pub t: f32,
    pub dt: f32,
    /// The eye drawn, and the eye the mode asked for (when the game says).
    pub drawn: Eye,
    pub target: Option<Eye>,
    /// The own figure's feet, when there is one.
    pub figure: Option<[f32; 3]>,
    /// What happened on this frame (a key, say), or empty.
    pub mark: String,
}

/// The unit direction an eye looks along (yaw about z from +y, pitch positive looking down).
pub fn look_direction(e: &Eye) -> [f32; 3] {
    let (sy, cy) = e.yaw.sin_cos();
    let (sp, cp) = e.pitch.sin_cos();
    [sy * cp, cy * cp, -sp]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn angle(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] * b[0] + a[1] * b[1] + a[2] * b[2])
        .clamp(-1.0, 1.0)
        .acos()
}

/// The frames of a run.
#[derive(Debug, Clone, Default)]
pub struct CameraLog {
    pub frames: Vec<CameraFrame>,
}

impl CameraLog {
    pub fn push(&mut self, frame: CameraFrame) {
        self.frames.push(frame);
    }

    /// Each frame's velocity (m/s) and the look's turn rate (rad/s), from the frame before (zero on the first).
    pub fn motion(&self) -> Vec<([f32; 3], f32)> {
        let mut out = Vec::with_capacity(self.frames.len());
        for (i, f) in self.frames.iter().enumerate() {
            let Some(prev) = i.checked_sub(1).map(|j| &self.frames[j]) else {
                out.push(([0.0; 3], 0.0));
                continue;
            };
            let dt = f.dt.max(1e-6);
            let v = sub(f.drawn.position, prev.drawn.position).map(|x| x / dt);
            let w = angle(look_direction(&prev.drawn), look_direction(&f.drawn)) / dt;
            out.push((v, w));
        }
        out
    }

    /// The figure's velocity each frame (m/s), from the frame before.
    pub fn figure_speed(&self) -> Vec<f32> {
        let mut out = vec![0.0];
        out.extend(
            self.frames
                .windows(2)
                .map(|w| match (w[0].figure, w[1].figure) {
                    (Some(a), Some(b)) => length(sub(b, a)) / w[1].dt.max(1e-6),
                    _ => 0.0,
                }),
        );
        out.truncate(self.frames.len());
        out
    }

    /// The CSV: a header, then a line a frame: time, frame time, the drawn eye (position, yaw, pitch, field of view), its
    /// velocity and speed, the look's turn rate, the asked eye's position and how far the drawn one is from it (the lag), the
    /// figure's feet and speed, and the frame's mark.
    pub fn csv(&self) -> String {
        let mut s = String::from(
            "t,dt,x,y,z,yaw,pitch,fov,vx,vy,vz,speed,turn_rate,tx,ty,tz,lag,fx,fy,fz,figure_speed,mark\n",
        );
        let motion = self.motion();
        let figure = self.figure_speed();
        for (i, f) in self.frames.iter().enumerate() {
            let (v, w) = motion[i];
            let p = f.drawn.position;
            let (t3, lag) = match &f.target {
                Some(t) => (
                    format!(
                        "{:.5},{:.5},{:.5}",
                        t.position[0], t.position[1], t.position[2]
                    ),
                    format!("{:.5}", length(sub(p, t.position))),
                ),
                None => (",,".to_string(), String::new()),
            };
            let f3 = f.figure.map_or(",,".to_string(), |q| {
                format!("{:.5},{:.5},{:.5}", q[0], q[1], q[2])
            });
            let _ = writeln!(
                s,
                "{:.5},{:.5},{:.5},{:.5},{:.5},{:.5},{:.5},{},{:.4},{:.4},{:.4},{:.4},{:.4},{t3},{lag},{f3},{:.4},{}",
                f.t,
                f.dt,
                p[0],
                p[1],
                p[2],
                f.drawn.yaw,
                f.drawn.pitch,
                f.drawn.fov.map_or(String::new(), |x| x.to_string()),
                v[0],
                v[1],
                v[2],
                length(v),
                w,
                figure[i],
                f.mark
            );
        }
        s
    }

    /// Writes [`CameraLog::csv`] into `path`.
    pub fn write_csv(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.csv())
    }
}
