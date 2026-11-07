//! The arena as plugins on the engine: [`ArenaPlugin`] (the client's prediction over the kit, the server's shapes), the
//! join screen's natives ([`JoinScreenPlugin`]), and [`arena_engine`], which gathers them with the engine's own.
//!
//! The server and every client are made from one engine: a client replays the client recipes, a server the server ones. Where
//! a server binds, its name and its map are arguments of each start, not plugins.

use std::rc::Rc;

use torque::app::hosting::register_join_natives;
use torque::app::plugins::{ClientGamePlugin, ClientWiring, DefaultPlugins, GuiPlugin};
use torque::engine::{ClientStage, EngineBuilder, Plugin, PluginGroup, PluginId, ServerStage};
use torque::script::vm::host::MemFiles;
use torque_example_common::prediction::CubeClient;

use crate::files::GameFiles;
use crate::server::{DEFAULT_PORT, shape_files};

/// What makes the arena the arena: the client's half of its prediction (the cube figures over the kit's files), the port its
/// scripts ask the LAN list on (`$Arena::Port`), and on a server the kit's shape files (`$Arena::Shapes`).
#[derive(Clone, PluginId)]
pub struct ArenaPlugin {
    files: MemFiles,
    port: u16,
}

impl ArenaPlugin {
    /// The arena over `files`, its LAN list asking on [`DEFAULT_PORT`].
    pub fn new(files: &GameFiles) -> ArenaPlugin {
        ArenaPlugin {
            files: files.mem_files(),
            port: DEFAULT_PORT,
        }
    }

    /// The LAN list asks on `port` (0 when hosting: any free one).
    pub fn with_port(mut self, port: u16) -> ArenaPlugin {
        self.port = port;
        self
    }
}

impl Plugin for ArenaPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_plugins(ClientGamePlugin::new(Rc::new(CubeClient::new(
            self.files.clone(),
        ))));
        let port = self.port.to_string();
        engine.client(ClientStage::Natives, move |vm, _| {
            vm.set_global("$Arena::Port", &port);
            Ok(())
        });
        engine.server(ServerStage::World, |vm, _| {
            vm.set_global("$Arena::Shapes", shape_files());
            Ok(())
        });
    }
}

/// The join screen's natives (`checkJoinAddress` and the rest): what the menu's address field asks.
#[derive(Clone, Copy, Default, PluginId)]
pub struct JoinScreenPlugin;

impl Plugin for JoinScreenPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client(ClientStage::Natives, |vm, _| {
            let wiring = vm
                .extension::<ClientWiring>()
                .ok_or("the join screen runs only when a game client is made")?;
            register_join_natives(vm, wiring.state());
            Ok(())
        });
    }
}

/// The arena's engine, not yet built: the engine's own plugins (the GUI measuring text with the fonts it draws with), the
/// arena's, and the join screen's. Add what runs it (`app`, `run`) or build it for a client ([`ArenaGame::from_engine`]) or a
/// server ([`ArenaServer`](crate::server::ArenaServer)).
///
/// [`ArenaGame::from_engine`]: crate::client::ArenaGame::from_engine
pub fn arena_engine(arena: ArenaPlugin) -> EngineBuilder {
    let mut engine = EngineBuilder::new();
    engine.add_plugins((
        DefaultPlugins.set(GuiPlugin { layout_fonts: true }),
        arena,
        JoinScreenPlugin,
    ));
    engine
}
