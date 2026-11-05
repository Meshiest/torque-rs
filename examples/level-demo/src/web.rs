//! The level demo in a browser: its files, fetched by the page or carried inside the module, and the engine's window as a
//! canvas on the page.
//!
//! **Its files.** Built with the `embed-pack` feature, the module carries them (a pack made by `example-level-demo --web-pack`,
//! its path in `TORQUE_WEB_PACK` when it is built) and the page calls [`start_embedded`]. Without it the page fetches the files
//! its list names (`files.json`, which `example-level-demo --web-files` writes beside the pack) and hands them over:
//! [`read_manifest`], [`add_file`] for each file (taken only when its size and hash are the list's), then [`start_fetched`].
//! Either way the files become the same `GameFiles`.
//!
//! **The page.** `?map=<name>` starts the map `missions/<name>.mis` at once instead of showing the menu. The page's root element
//! carries the game's state for a test to wait on: `data-torque-frames` (the frames drawn), `data-torque-map` (the map being
//! played, empty on the menu), `data-torque-figure` (`1` once the client holds its figure's ghost) and `data-torque-position`
//! (where the client sees its figure, `x y z`).

use std::cell::RefCell;

use torque::app::manifest::Manifest;
use torque::app::view::WorldView;
use torque::app::{App, Host};
use torque::gui::GuiDraws;
use torque_render_core::Gpu;
use torque_render_scene::source::Eye;

use crate::client::LevelDemo;
use crate::files::GameFiles;

/// The files the build carries.
#[cfg(feature = "embed-pack")]
static PACK: &[u8] = include_bytes!(env!("TORQUE_WEB_PACK"));

/// The files fetched so far and the list they were fetched by.
struct Fetched {
    manifest: Manifest,
    files: Vec<(String, Vec<u8>)>,
}

thread_local! {
    static FETCHED: RefCell<Option<Fetched>> = const { RefCell::new(None) };
}

/// Shows a panic in the page's console.
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Whether the module carries its files (the `embed-pack` feature).
pub fn embedded() -> bool {
    cfg!(feature = "embed-pack")
}

/// Starts the game on the files the module carries.
#[cfg(feature = "embed-pack")]
pub fn start_embedded() -> Result<(), String> {
    start(GameFiles::unpack(PACK)?)
}

/// Starts the game on the files the module carries: this build carries none.
#[cfg(not(feature = "embed-pack"))]
pub fn start_embedded() -> Result<(), String> {
    Err("this build carries no files: the page fetches them by its list".into())
}

/// Reads the list of the files to fetch (Err: it is not one, or it breaks a cap), forgetting any files added before.
pub fn read_manifest(text: &str) -> Result<(), String> {
    let manifest = Manifest::parse(text)?;
    FETCHED.with_borrow_mut(|f| {
        *f = Some(Fetched {
            manifest,
            files: Vec::new(),
        })
    });
    Ok(())
}

/// Takes the fetched file `path` (Err, the file left out: the list does not name it, or its size or hash is not the list's).
pub fn add_file(path: &str, bytes: Vec<u8>) -> Result<(), String> {
    FETCHED.with_borrow_mut(|f| {
        let f = f.as_mut().ok_or("no list of files was read")?;
        let entry = f
            .manifest
            .file(path)
            .ok_or_else(|| format!("{path} is not in the list of files"))?;
        entry.check(&bytes)?;
        let path = entry.path.clone();
        f.files.retain(|(p, _)| *p != path);
        f.files.push((path, bytes));
        Ok(())
    })
}

/// Starts the game on the fetched files (Err: one the list names is missing, or a pack is broken).
pub fn start_fetched() -> Result<(), String> {
    let Fetched { manifest, files } = FETCHED.take().ok_or("no list of files was read")?;
    if let Some(missing) = manifest
        .files
        .iter()
        .find(|m| !files.iter().any(|(p, _)| *p == m.path))
    {
        return Err(format!("{} was not fetched", missing.path));
    }
    start(GameFiles::from_fetched(files)?)
}

fn start(files: GameFiles) -> Result<(), String> {
    let mut demo = LevelDemo::new(files)?;
    if let Some(name) = query("map") {
        let want = format!("missions/{name}.mis");
        let index = demo
            .maps
            .iter()
            .position(|m| m.path.eq_ignore_ascii_case(&want))
            .ok_or_else(|| format!("no map {want}"))?;
        demo.start_map(index)?;
    }
    torque::app::window::run(WebDemo { demo, frames: 0 })
}

/// The value of `name` in the page's query string.
fn query(name: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    search
        .trim_start_matches('?')
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// The level demo, its state written on the page as it draws (module docs).
struct WebDemo {
    demo: LevelDemo,
    frames: u64,
}

impl WebDemo {
    fn mark(&self) {
        let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        else {
            return;
        };
        let map = self
            .demo
            .playing
            .and_then(|i| self.demo.maps.get(i))
            .map(|m| m.path.as_str())
            .unwrap_or_default();
        let own = self.demo.figure();
        let figure = if own.is_some() { "1" } else { "0" };
        let position = own
            .map(|f| format!("{} {} {}", f.position[0], f.position[1], f.position[2]))
            .unwrap_or_default();
        let _ = root.set_attribute("data-torque-frames", &self.frames.to_string());
        let _ = root.set_attribute("data-torque-map", map);
        let _ = root.set_attribute("data-torque-figure", figure);
        let _ = root.set_attribute("data-torque-position", &position);
    }
}

impl App for WebDemo {
    fn host(&mut self) -> &mut Host {
        self.demo.host()
    }

    fn host_ref(&self) -> &Host {
        self.demo.host_ref()
    }

    fn title(&self) -> String {
        self.demo.title()
    }

    fn log_name(&self) -> &str {
        self.demo.log_name()
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        self.demo.view(gpu)
    }

    fn advance(&mut self, ms: u32) {
        App::advance(&mut self.demo, ms);
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        let drawn = App::frame(&mut self.demo, view);
        self.frames += 1;
        if self.frames % 15 == 1 {
            self.mark();
        }
        drawn
    }

    /// A map is loading until the client holds its figure.
    fn loading(&self) -> bool {
        self.demo.playing.is_some() && self.demo.figure().is_none()
    }
}
