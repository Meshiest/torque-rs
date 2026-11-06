//! The client's state as its natives keep it ([`ClientState`]): the engine's parts, and one slot for a game's own state
//! ([`GameSlot`], which the engine never reads) and the game's parts of the natives' state ([`ConnGame::Parts`]).

use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use ts_vm::sim::ObjId;
use ts_vm::vm::VmError;

use crate::conn::{ConnGame, ConnSlot};
use crate::{actionmap, screen};

/// The most callbacks waiting for the next tick.
pub const MAX_QUEUED: usize = 4_096;

/// A script callback for the next tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queued {
    pub object: ObjId,
    pub method: Vec<u8>,
    pub args: Vec<Vec<u8>>,
}

/// What opens an http(s) URL `gotoWebPage` passed: the windowed client's, the OS's URL handler.
pub type WebOpener = Rc<dyn Fn(&str) -> Result<(), String>>;
/// The client natives' state: the engine's parts and the game's slot ([`ClientState::game`]).
#[derive(Default)]
pub struct ClientState<G: ConnGame> {
    /// The resource manager's mod paths (`setModPaths`, read back by `getModPaths`: the valid folders, `;`-joined).
    pub mod_paths: Vec<u8>,
    /// Every ActionMap's device maps and nodes ([`actionmap`]), and the keys held down (the break table, shared by the maps as
    /// TGE's static one is).
    pub binds: HashMap<ObjId, Vec<actionmap::DeviceMap>>,
    pub breaks: Vec<actionmap::BreakEntry>,
    pub queue: VecDeque<Queued>,
    /// Callbacks dropped because the queue was full.
    pub dropped: u64,
    /// What `gotoWebPage` was asked to open (printed; opened only through [`ClientState::web_opener`]).
    pub web_pages: Vec<Vec<u8>>,
    /// The display device the client reports (`getDisplayDeviceList`, `getResolutionList`, `setDisplayDevice`): empty for
    /// [`screen::DISPLAY_DEVICE`] (every headless run); the windowed client names the backend its renderer really uses.
    pub display_device: String,
    /// Every display device the windowed client can draw with (its usable renderer backends, the one in use among them):
    /// `getDisplayDeviceList` lists them and `setDisplayDevice` takes any of them for the next start. Empty in every headless run.
    pub display_devices: Vec<String>,
    /// A device `setDisplayDevice` chose that is not the one in use (the window starts with it next time).
    pub display_next: Option<String>,
    /// The windowed client's URL opener: None in every headless run, which opens nothing.
    pub web_opener: Option<WebOpener>,
    /// The errors of queued callbacks that stopped (at most [`MAX_QUEUED`]).
    pub errors: Vec<VmError>,
    /// The client's UDP socket (`setNetPort`, [`crate::net`]), shared with the platform's bind.
    pub net: Rc<RefCell<crate::net::ClientNet>>,
    /// The server query's client half ([`crate::query`]).
    pub query: crate::net::query::QueryState,
    /// The user's own input is being handled (a window message's routing, [`crate::input::post`]): a game's native may arm an
    /// internet join permit for an address its scripts ask about ([`crate::net::Internet::arm`]). Nothing else sets it.
    pub user_input: bool,
    /// The connection to a server, while there is one ([`crate::conn`]).
    pub conn: ConnSlot<G>,
    /// The install the client runs over (read only).
    pub root: std::path::PathBuf,
    /// `lockMouse`'s state: the window keeps the mouse (the window client grabs it; headless, nothing).
    pub mouse_locked: bool,
    /// The server's datablocks the client could not make ([`crate::datablocks::Received::Refused`]).
    pub datablocks_refused: u64,
    /// The named-shape list ([`crate::hud::NameList`]).
    pub names: crate::hud::NameList,
    /// `setScreenMode`'s current mode and the request the windowed client takes ([`screen`]).
    pub screen: screen::ScreenState,
    /// The zoom ([`crate::zoom`]): `setFov`'s target and the camera's fov.
    pub zoom: crate::zoom::Zoom,
    /// The frame rate ([`crate::fps`]).
    pub fps: crate::fps::Fps,
    /// The frames of each quarter second, for a game's readout ([`crate::fps::FrameWindow`]).
    pub frame_window: crate::fps::FrameWindow,
    /// The client's work by part for the window's frame breakdown ([`crate::live_perf`]).
    pub live_perf: crate::live_perf::LivePerf,
    /// What `setVerticalSync` asked for since the windowed client last looked.
    pub vsync_request: Option<bool>,
    /// Whether a look the scripts asked for is still being built (the windowed client's).
    pub graphics_pending: bool,
    /// The audio driver the Options dialog shows in place of "OpenAL" (the windowed client's), when set.
    pub audio_driver: Option<String>,
    /// The `console.log` the console's lines go to (the windowed client's, [`crate::console_log`]); None in memory-only runs.
    pub console_log: Option<crate::console_log::SharedLog>,
    /// A script's cap on the view distance (a game's native sets it) and the Sky's distance fields when it was set
    /// ([`crate::conn::view_cap`]): it caps the view until the Sky's distances change. None until one is set.
    pub max_view_distance: Option<(f32, Option<[u32; 2]>)>,
    /// The game's own state ([`GameSlot`]): the engine never reads it.
    pub game: GameSlot,
    /// The game's own parts of the natives' state ([`ConnGame::Parts`]), reached through this state as its own fields are
    /// (`Deref`): the engine never reads them.
    pub parts: G::Parts,
}

impl<G: ConnGame> Deref for ClientState<G> {
    type Target = G::Parts;

    fn deref(&self) -> &G::Parts {
        &self.parts
    }
}

impl<G: ConnGame> DerefMut for ClientState<G> {
    fn deref_mut(&mut self) -> &mut G::Parts {
        &mut self.parts
    }
}

impl<G: ConnGame> ClientState<G> {
    pub fn queue_callback(&mut self, call: Queued) {
        if self.queue.len() < MAX_QUEUED {
            self.queue.push_back(call);
        } else {
            self.dropped += 1;
        }
    }
}

/// The game's state beside the engine's: one value of the game's own type (`()` until the game puts its own there).
#[derive(Clone)]
pub struct GameSlot(pub Rc<dyn Any>);

impl Default for GameSlot {
    fn default() -> GameSlot {
        GameSlot(Rc::new(()))
    }
}

impl GameSlot {
    /// The game's state, if it is a `T`.
    pub fn get<T: 'static>(&self) -> Option<Rc<T>> {
        Rc::clone(&self.0).downcast::<T>().ok()
    }
}
