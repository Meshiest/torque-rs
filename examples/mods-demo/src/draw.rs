//! A client's world drawn as a window draws it: the world's ghosts (the pitch, the players and the ball, in shapes the client
//! downloaded) and the GUI the client's own script keeps.
//!
//! The drawing reads its files through the client's own script engine (the same file layer `exec` reads), so what the client
//! downloaded and mounted is what is drawn: a client that had not mounted the mod's shapes would draw nothing of the match.

use std::rc::Rc;

use torque::app::WorldView;
use torque::app::client::GameClient;
use torque::app::ghosts::{FrameInput, GhostOptions, GhostScene};
use torque::app::view::Daylight;
use torque::gui::GuiDraws;
use torque::script::vm::host::{FileHost, FileRead, FileStat};
use torque_render_core::Gpu;
use torque_render_scene::Assets;
use torque_render_scene::moving::ShapeSelect;
use torque_render_scene::source::Eye;

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

/// The drawing of one client's world.
pub struct Drawing {
    ghosts: GhostScene,
    files: ClientFiles,
}

impl Drawing {
    /// The drawing of `client`'s world, from its own files. Err: the client's script engine gave no files to share.
    pub fn new(client: &mut GameClient) -> Result<Drawing, String> {
        let mut slot = None;
        client.vm_mut().wrap_file_host(|inner| {
            let shared: Rc<dyn FileHost> = Rc::from(inner);
            slot = Some(Rc::clone(&shared));
            Box::new(Shared(shared))
        });
        let files = slot.ok_or("the client's files could not be shared with the drawing")?;
        Ok(Drawing {
            // Every shape: the pitch's static shapes and the client's own player too, which the view stands apart from.
            ghosts: GhostScene::new(GhostOptions {
                select: ShapeSelect::All,
                ..GhostOptions::default()
            }),
            files: ClientFiles(files),
        })
    }

    /// Whether the client can read the file `path` (mounted content included).
    pub fn has_file(&self, path: &str) -> bool {
        self.files.read(path).is_some()
    }

    /// A frame of `client`'s world, `dt` seconds after the last, seen from `eye`, and its GUI.
    pub fn frame(
        &mut self,
        client: &mut GameClient,
        view: &mut WorldView,
        dt: f32,
        eye: &Eye,
    ) -> GuiDraws {
        view.advance_time(dt);
        {
            let mut st = client.host.client.borrow_mut();
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
                    .objects(view, &self.files, &conn.world, Some(eye));
                view.set_objects(objects);
            } else {
                view.set_objects(Vec::new());
            }
        }
        client.host.gui.0.borrow_mut().world_camera = true;
        client.host.draw_frame()
    }
}

/// The world view the game draws in: a plain floor round the pitch, under a clear sky.
pub fn world_view(gpu: &Gpu) -> WorldView {
    let mut view = WorldView::new(gpu);
    view.set_plain_floor(60.0, [112, 146, 104]);
    view.set_daylight(&Daylight::default());
    view
}
