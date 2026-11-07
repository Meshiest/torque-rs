//! Recipes: what plugins record to run on every new script engine, filed by [`Stage`].

use std::fmt;

use torque_ts_vm::vm::Vm;

use crate::{Engine, PluginId};

/// A point in making a server's script engine at which recipes run. The host making the engine replays each stage in
/// this order, at the moment the stage's parts exist.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum ServerStage {
    /// The game classes' console classes, first of all.
    Classes,
    /// The game's own preparations that the server game reads when it is installed (its simulation classes).
    Prepare,
    /// The server game: its simulation and natives.
    Game,
    /// The world's objects: terrain, water.
    World,
    /// The network host and its passes.
    Net,
}

impl ServerStage {
    /// Every server stage, in replay order.
    pub const ALL: &'static [ServerStage] = &[
        ServerStage::Classes,
        ServerStage::Prepare,
        ServerStage::Game,
        ServerStage::World,
        ServerStage::Net,
    ];
}

/// A point in making a client's script engine at which recipes run. The host making the engine replays each stage in
/// this order, at the moment the stage's parts exist.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum ClientStage {
    /// The game classes' console classes, when the client has them.
    Classes,
    /// The client game: what draws and predicts the game's objects.
    Game,
    /// The GUI's classes and natives.
    Gui,
    /// Input: action maps and their natives.
    Input,
    /// The screen: resolution, full screen and the canvas's natives.
    Screen,
    /// The connection to a server, when the client has one.
    Conn,
    /// The server query (the server browser's natives).
    Query,
    /// The game's own natives and globals, just before its client script runs.
    Natives,
    /// The client host that runs the frames.
    Host,
    /// Once the window exists.
    Window,
}

impl ClientStage {
    /// Every client stage, in replay order.
    pub const ALL: &'static [ClientStage] = &[
        ClientStage::Classes,
        ClientStage::Game,
        ClientStage::Gui,
        ClientStage::Input,
        ClientStage::Screen,
        ClientStage::Conn,
        ClientStage::Query,
        ClientStage::Natives,
        ClientStage::Host,
        ClientStage::Window,
    ];
}

/// A server or a client stage: what [`Engine::replay`] takes (both stage types convert into it).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Stage {
    /// A stage of a server's script engine.
    Server(ServerStage),
    /// A stage of a client's script engine.
    Client(ClientStage),
}

impl From<ServerStage> for Stage {
    fn from(stage: ServerStage) -> Stage {
        Stage::Server(stage)
    }
}

impl From<ClientStage> for Stage {
    fn from(stage: ClientStage) -> Stage {
        Stage::Client(stage)
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Stage::Server(stage) => write!(f, "server stage {stage:?}"),
            Stage::Client(stage) => write!(f, "client stage {stage:?}"),
        }
    }
}

/// The function a recipe runs: it gets the new script engine and the engine it was built from (for its resources and
/// slots; clone the [`Engine`] to keep it in a native). An `Err` stops the replay.
pub(crate) type RecipeFn = Box<dyn Fn(&mut Vm, &Engine) -> Result<(), String>>;

pub(crate) struct Recipe {
    pub(crate) stage: Stage,
    pub(crate) plugin: PluginId,
    pub(crate) run: RecipeFn,
}

/// A recipe that failed while [`Engine::replay`] ran it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RecipeError {
    /// The plugin that recorded the recipe.
    pub plugin: PluginId,
    /// The stage being replayed.
    pub stage: Stage,
    /// What the recipe returned.
    pub message: String,
}

impl fmt::Display for RecipeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "plugin `{}` failed at the {}: {}",
            self.plugin, self.stage, self.message
        )
    }
}

impl std::error::Error for RecipeError {}
