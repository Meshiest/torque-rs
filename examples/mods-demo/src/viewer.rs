//! A client of a [`Session`] drawn offscreen, as a window would draw it: the world's ghosts (the runners and orbs, in shapes the
//! client downloaded), the ropes the rope plugin draws, and the board the client's own script keeps.
//!
//! The drawing reads its files through the client's own script engine (the same file layer `exec` reads), so what the client
//! downloaded and mounted is what is drawn: a client that had not mounted the mod's shapes would draw nothing for the orbs.

use std::rc::Rc;

use torque::app::ghosts::{FrameInput, GhostOptions, GhostScene};
use torque::app::shots::Seat;
use torque::app::view::Daylight;
use torque::app::window::offscreen_gpu;
use torque::app::{App, Host, WorldView};
use torque::gui::GuiDraws;
use torque::script::vm::host::{FileHost, FileRead, FileStat};
use torque_render_core::{Frame, Gpu};
use torque_render_scene::Assets;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::Eye;

use crate::session::Session;

/// The picture's size.
pub const SIZE: (u32, u32) = (1280, 720);

/// A script engine's files, shared: the same layer, read by the script engine and by the drawing.
struct Shared(Rc<dyn FileHost>);

impl FileHost for Shared {
    fn read(&self, path: &[u8]) -> FileRead {
        self.0.read(path)
    }

    fn stat(&self, path: &[u8]) -> FileStat {
        self.0.stat(path)
    }

    fn stat_without_look_up(&self, path: &[u8]) -> FileStat {
        self.0.stat_without_look_up(path)
    }

    fn is_archive_entry(&self, path: &[u8]) -> bool {
        self.0.is_archive_entry(path)
    }

    fn real_path(&self, path: &[u8]) -> Option<Vec<u8>> {
        self.0.real_path(path)
    }

    fn modified(&self, path: &[u8]) -> Option<u64> {
        self.0.modified(path)
    }

    fn files(&self) -> Rc<Vec<Vec<u8>>> {
        self.0.files()
    }

    fn remove(&self, path: &[u8]) -> bool {
        self.0.remove(path)
    }

    fn unlist(&self, path: &[u8]) {
        self.0.unlist(path);
    }

    fn copy(&self, from: &[u8], to: &[u8]) -> bool {
        self.0.copy(from, to)
    }

    fn write(&self, path: &[u8], contents: &[u8], append: bool) -> bool {
        self.0.write(path, contents, append)
    }

    fn work_done(&self) -> u64 {
        self.0.work_done()
    }

    fn add_mod_path(&self, path: &[u8]) -> usize {
        self.0.add_mod_path(path)
    }
}

/// What the drawing reads: the client's files.
struct ClientFiles(Rc<dyn FileHost>);

impl Assets for ClientFiles {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        match self.0.read(path.as_bytes()) {
            FileRead::Found(bytes) => Some(bytes),
            _ => None,
        }
    }
}

/// A session whose client `watch` is drawn.
pub struct Viewer {
    pub session: Session,
    pub watch: usize,
    ghosts: GhostScene,
    files: ClientFiles,
    /// The eye the picture is drawn from.
    pub eye: Eye,
    /// The time the session has not stepped yet (ms).
    owed: u32,
}

/// The default view: from the south and above, the whole field.
pub fn overview() -> Eye {
    look_at([0.0, -12.5, 9.5], [0.0, 0.5, 0.3])
}

impl Viewer {
    /// Draws client `watch` of `session`.
    pub fn new(mut session: Session, watch: usize) -> Viewer {
        let shared: Rc<dyn FileHost> = {
            let mut slot = None;
            session.clients[watch].vm_mut().wrap_file_host(|inner| {
                let shared: Rc<dyn FileHost> = Rc::from(inner);
                slot = Some(Rc::clone(&shared));
                Box::new(Shared(shared))
            });
            slot.expect("the files were wrapped")
        };
        Viewer {
            session,
            watch,
            ghosts: GhostScene::new(GhostOptions::default()),
            files: ClientFiles(shared),
            eye: overview(),
            owed: 0,
        }
    }

    /// Whether the client can read the file `path` (mounted content included).
    pub fn has_file(&self, path: &str) -> bool {
        self.files.read(path).is_some()
    }
}

impl App for Viewer {
    fn host(&mut self) -> &mut Host {
        &mut self.session.clients[self.watch].host
    }

    fn host_ref(&self) -> &Host {
        &self.session.clients[self.watch].host
    }

    fn title(&self) -> String {
        "torque-example-mods-demo".to_string()
    }

    fn log_name(&self) -> &str {
        "example-mods-demo"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        let mut view = WorldView::new(gpu);
        view.set_plain_floor(60.0, [112, 146, 104]);
        view.set_daylight(&Daylight::default());
        view
    }

    fn advance(&mut self, ms: u32) {
        self.owed += ms;
        while self.owed >= crate::session::TICK_MS {
            self.owed -= crate::session::TICK_MS;
            self.session.step();
        }
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        let dt = 0.016;
        // The ropes run with the view's clock.
        view.advance_time(dt);
        {
            let host = &self.session.clients[self.watch].host;
            let mut st = host.client.borrow_mut();
            if let Some(conn) = st.conn.conn.as_mut() {
                let input = FrameInput {
                    ghosts: None,
                    fov: None,
                    dt,
                    surfaces: &|_, _, _| Vec::new(),
                };
                self.ghosts.update(&self.files, &mut conn.world, &input);
                let objects = self
                    .ghosts
                    .objects(view, &self.files, &conn.world, Some(&self.eye));
                view.set_objects(objects);
            }
        }
        let host = &mut self.session.clients[self.watch].host;
        host.gui.0.borrow_mut().world_camera = true;
        (host.draw_frame(), self.eye)
    }
}

/// A seat that draws client `watch` of `session` at [`SIZE`], and the GPU it draws on. Err: no GPU (the test says so and
/// skips).
pub fn seat(session: Session, watch: usize) -> Result<(Seat<Viewer>, Gpu), String> {
    let gpu = offscreen_gpu().map_err(|e| e.to_string())?;
    let seat = Seat::new(Viewer::new(session, watch), &gpu, SIZE)?;
    Ok((seat, gpu))
}

/// A picture of what the seat shows now.
pub fn picture(seat: &mut Seat<Viewer>, gpu: &Gpu) -> Result<Frame, String> {
    seat.picture(gpu)
}

/// The pictures of a round in progress, as client 0 draws it, written into `dir`: an early moment of the round, the round with a leader
/// and the scores after it. The session must hold two clients that have joined. Returns the paths.
pub fn write_shots(
    session: Session,
    dir: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>, String> {
    let (mut seat, gpu) = seat(session, 0)?;
    let mut shots = torque::app::shots::Shots::new(dir)?;
    let phase = |seat: &Seat<Viewer>| seat.game.session.server_global("$Orbs::Phase");
    seat.run(&gpu, 3_000)?;
    shots.save(&picture(&mut seat, &gpu)?, "round-early")?;
    seat.run(&gpu, 5_000)?;
    shots.save(&picture(&mut seat, &gpu)?, "round-leader")?;
    let mut waited = 0;
    while phase(&seat) != "break" && waited < 30_000 {
        seat.run(&gpu, 500)?;
        waited += 500;
    }
    seat.run(&gpu, 500)?;
    shots.save(&picture(&mut seat, &gpu)?, "round-over")?;
    Ok(shots.paths)
}
