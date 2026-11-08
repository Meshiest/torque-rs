//! An [`App`] without a window: every frame drawn offscreen as the window would draw it, and pictures of it written as PNGs.
//!
//! A [`Seat`] is one game with its world view and its target; [`run`] runs the clocks of several seats side by side, a frame
//! of each every [`FRAME_MS`]; [`Seat::picture`] holds the clock while the renderer settles on the scene, then reads it back;
//! [`Shots`] writes the pictures into a folder.

use std::io;
use std::path::{Path, PathBuf};

use torque_render_core::{Frame, Gpu, Offscreen};
use torque_render_scene::source::Eye;

use super::App;
use super::view::WorldView;

/// The clock's step between frames (ms).
pub const FRAME_MS: u32 = 16;
/// The frames drawn from a still scene before its picture (the renderer's history settles).
pub const SETTLE_FRAMES: u32 = 24;

/// One game and what its frames draw into.
pub struct Seat<G> {
    pub game: G,
    pub view: WorldView,
    pub target: Offscreen,
    /// An eye in place of the game's (none: the game's).
    pub eye: Option<Eye>,
    /// The real clock the frames are counted by, as a window counts its own (the readout of a game's frame rate).
    clock: web_time::Instant,
}

impl<G: App> Seat<G> {
    /// `game` drawn at `size` on `gpu`.
    pub fn new(mut game: G, gpu: &Gpu, size: (u32, u32)) -> Result<Seat<G>, String> {
        game.host().surface_size(size);
        let mut view = game.view(gpu);
        view.attach_to(game.host());
        // The emitter kinds run by the frame's step, not by how long the machine took to draw it.
        view.set_effect_frame_seconds(Some(FRAME_MS as f32 / 1000.0));
        // Nor does what the renderer draws wait on the clock: its uploads and the clouds' noise are not held to a time.
        view.renderer().set_unbudgeted();
        Ok(Seat {
            view,
            target: gpu.offscreen(size.0, size.1).map_err(|e| e.to_string())?,
            eye: None,
            clock: web_time::Instant::now(),
            game,
        })
    }

    /// Draws one frame.
    pub fn frame(&mut self, gpu: &Gpu) -> Result<(), String> {
        // Names over the shapes (GuiShapeNameHud) stand where the game's own camera sees them: none from another eye.
        if self.eye.is_some() {
            self.game.host().gui.0.borrow_mut().hud.names.clear();
        }
        let (gui, eye) = self.game.frame(&mut self.view);
        let eye = self.eye.unwrap_or(eye);
        // A frame ages the debug gizmos by its step, not by how long the machine took to draw it.
        #[cfg(feature = "gizmos")]
        self.view
            .set_gizmo_frame_seconds(Some(FRAME_MS as f32 / 1000.0));
        self.view.draw(gpu, &self.target.color, &gui, eye)?;
        let real_ms = self.clock.elapsed().as_secs_f64() * 1000.0;
        self.game.host().frame_drawn(real_ms);
        self.view.report_warnings(self.game.host());
        Ok(())
    }

    /// Runs `ms` of the clock, a frame every [`FRAME_MS`].
    pub fn run(&mut self, gpu: &Gpu, ms: u32) -> Result<(), String> {
        run(gpu, &mut [self], ms)
    }

    /// A picture of the scene as it stands (the clock held), after the renderer settled on it.
    pub fn picture(&mut self, gpu: &Gpu) -> Result<Frame, String> {
        self.view.reset_history();
        for _ in 0..SETTLE_FRAMES {
            self.frame(gpu)?;
        }
        gpu.read_back(&self.target).map_err(|e| e.to_string())
    }

    /// A picture of the world alone as it stands (the clock held), over the whole target, after the renderer settled on it:
    /// no GUI drawn over it (no HUD, hints or menus), whatever rectangle the game's canvas gives the world.
    pub fn world_picture(&mut self, gpu: &Gpu) -> Result<Frame, String> {
        let rect = [0, 0, self.target.width, self.target.height];
        self.view.reset_history();
        for _ in 0..SETTLE_FRAMES {
            let (_, eye) = self.game.frame(&mut self.view);
            let eye = self.eye.unwrap_or(eye);
            #[cfg(feature = "gizmos")]
            self.view
                .set_gizmo_frame_seconds(Some(FRAME_MS as f32 / 1000.0));
            self.view.draw_world(&self.target.color, rect, eye)?;
        }
        gpu.read_back(&self.target).map_err(|e| e.to_string())
    }
}

/// The seats' clocks run `ms`, a frame of each every [`FRAME_MS`].
pub fn run<G: App>(gpu: &Gpu, seats: &mut [&mut Seat<G>], ms: u32) -> Result<(), String> {
    for _ in 0..ms.div_ceil(FRAME_MS) {
        for s in seats.iter_mut() {
            s.game.advance(FRAME_MS);
            s.frame(gpu)?;
        }
    }
    Ok(())
}

/// A folder of pictures and the paths written into it.
pub struct Shots {
    dir: PathBuf,
    pub paths: Vec<PathBuf>,
}

impl Shots {
    /// The folder `dir`, made if missing.
    pub fn new(dir: &Path) -> Result<Shots, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        Ok(Shots {
            dir: dir.to_path_buf(),
            paths: Vec::new(),
        })
    }

    /// Writes `frame` as `<name>.png`.
    pub fn save(&mut self, frame: &Frame, name: &str) -> Result<(), String> {
        let path = self.dir.join(format!("{name}.png"));
        write_png(frame, &path).map_err(|e| e.to_string())?;
        self.paths.push(path);
        Ok(())
    }
}

/// Writes `frame` as a PNG (its color, no alpha).
pub fn write_png(frame: &Frame, path: &Path) -> io::Result<()> {
    let width = frame.width;
    let bytes = png_rgb(frame.width, frame.height, |x, y| {
        let i = 4 * (y * width + x) as usize;
        [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
    })?;
    std::fs::write(path, bytes)
}

/// An image of `width` x `height` whose pixel at column `x`, row `y` (row 0 at the top) is `pixel(x, y)`, as a PNG.
pub fn png_rgb(
    width: u32,
    height: u32,
    pixel: impl Fn(u32, u32) -> [u8; 3],
) -> io::Result<Vec<u8>> {
    let mut data = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            data.extend(pixel(x, y));
        }
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(io::Error::other)?;
    writer.write_image_data(&data).map_err(io::Error::other)?;
    writer.finish().map_err(io::Error::other)?;
    Ok(out)
}
