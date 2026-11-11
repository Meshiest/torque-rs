//! The net game, codecs and server game slots: a server of a net game of its own starts from the slot, a server of another
//! type is refused, a second setter fails the build, and the dedicated runner starts the slot's net game.

use std::cell::{Cell, RefCell};
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque_engine::testing::ClosurePlugin;
use torque_engine::{BuildError, Engine, EngineBuilder};
use torque_net::handshake::ServerConfig;
use torque_net::huffman::Table;
use torque_server::game::GameParts;
use torque_server::net::NetGame;
use torque_server::net::codecs::NetCodecs;
use torque_server::net::torque::{NoGameEvents, TgeGhostGame, TorqueNet};
use torque_ts_vm::host::MemFiles;

use crate::net_game::{
    NetCodecsPlugin, NetGameFactory, NetGameParts, NetGamePlugin, plugin_class_warning,
    start_net_game,
};
use crate::plugins::MinimalPlugins;
use crate::run::{Dedicated, EngineDedicated};
use crate::server::{GameServer, Listen, ServerStart};
use crate::slots::ServerSlots;
use crate::testing::TestNet;

/// Another net game, for the refusals.
#[derive(Debug, Default)]
struct OtherNet(TorqueNet);

impl NetGame for OtherNet {
    type Events = NoGameEvents;
    type Ghosts = TgeGhostGame;
    type Conn = ();

    fn string_table(&self) -> Rc<Table> {
        self.0.string_table()
    }
}

fn test_parts() -> NetGameParts<TestNet> {
    NetGameParts {
        game: TestNet::default(),
        ghosts: TgeGhostGame,
        static_writes: None,
        payload: None,
    }
}

fn files() -> MemFiles {
    let files = MemFiles::new();
    files.insert(
        "server.cs",
        b"function startIt() { echo(\"started\"); onTick(); }\n\
          function onTick() { $Ticks++; if ($Ticks == 60) echo(\"sixty\"); schedule(32, 0, \"onTick\"); }",
    );
    files
}

fn start() -> ServerStart<'static> {
    ServerStart {
        listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
        globals: &[],
        start: "startIt",
        args: &[],
    }
}

fn engine_with(plugins: impl FnOnce(&mut EngineBuilder)) -> Engine {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(MinimalPlugins);
    plugins(&mut builder);
    builder.build().expect("the engine builds")
}

fn build_error(builder: &mut EngineBuilder) -> BuildError {
    let Err(error) = builder.build() else {
        panic!("the build fails");
    };
    error
}

#[test]
fn net_game_a_server_of_the_slots_net_game_is_made_by_its_factory_over_the_server_game() {
    let made = Rc::new(Cell::new(0));
    let wrapped = Rc::new(Cell::new(false));
    let container = Rc::new(RefCell::new(None));
    let engine = engine_with(|builder| {
        let (made, wrapped, container) = (made.clone(), wrapped.clone(), container.clone());
        builder.add_plugins(NetGamePlugin::new(move |_vm: &mut _, game: &GameParts| {
            made.set(made.get() + 1);
            *container.borrow_mut() = Some(Rc::clone(&game.container));
            let wrapped = Rc::clone(&wrapped);
            Ok(NetGameParts {
                payload: Some(Box::new(move |torque| {
                    wrapped.set(true);
                    torque
                })),
                ..test_parts()
            })
        }));
    });
    let mut server =
        GameServer::<TestNet>::start_with_net(&engine, files(), &start()).expect("starts");
    server.tick();
    assert_eq!(made.get(), 1);
    assert!(wrapped.get(), "the payload layer was handed Torque's");
    let seen = container
        .borrow()
        .clone()
        .expect("the factory saw the parts");
    assert!(Rc::ptr_eq(&seen, &server.game.container));
    assert!(server.drain_log().iter().any(|l| l.contains("started")));
}

#[test]
fn net_game_start_engine_on_an_engine_set_for_another_net_game_is_refused_naming_both() {
    let engine = engine_with(|builder| {
        builder.add_plugins(NetGamePlugin::<TestNet>::new(|_: &mut _, _: &_| {
            Ok(test_parts())
        }));
    });
    let Err(error) = GameServer::start_engine(&engine, files(), &start()) else {
        panic!("refused");
    };
    assert!(
        error.contains("TestNet") && error.contains("TorqueNet"),
        "{error}"
    );
    let Err(error) = GameServer::<OtherNet>::start_with_net(&engine, files(), &start()) else {
        panic!("refused");
    };
    assert!(
        error.contains("TestNet") && error.contains("OtherNet"),
        "{error}"
    );
}

#[test]
fn net_game_an_engine_with_no_slot_starts_only_torques_net_game() {
    let engine = engine_with(|_| {});
    let Err(error) = GameServer::<TestNet>::start_with_net(&engine, files(), &start()) else {
        panic!("refused");
    };
    assert!(
        error.contains("TestNet") && error.contains("TorqueNet"),
        "{error}"
    );
    let server = GameServer::<TorqueNet>::start_with_net(&engine, files(), &start());
    assert!(server.is_ok(), "{:?}", server.err());
}

#[test]
fn net_game_a_second_net_game_fails_the_build_naming_both() {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(MinimalPlugins);
    builder.add_plugins(ClosurePlugin::new("test.first", |engine| {
        let factory: Rc<dyn NetGameFactory<TestNet>> = Rc::new(|_: &mut _, _: &_| Ok(test_parts()));
        engine.net_game(factory);
    }));
    builder.add_plugins(NetGamePlugin::<TestNet>::new(|_: &mut _, _: &_| {
        Ok(test_parts())
    }));
    let error = build_error(&mut builder);
    let text = error.to_string();
    assert!(matches!(error, BuildError::DuplicateSlot { .. }), "{text}");
    assert!(
        text.contains("test.first") && text.contains("torque.net-game"),
        "{text}"
    );
}

#[test]
fn net_game_a_second_codecs_setter_fails_the_build_naming_both() {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(MinimalPlugins);
    builder.add_plugins(ClosurePlugin::new("test.first", |engine| {
        engine.net_codecs(|_| Rc::new(Named) as Rc<dyn NetCodecs>);
    }));
    builder.add_plugins(NetCodecsPlugin::new(|_| Rc::new(Named)));
    let error = build_error(&mut builder);
    let text = error.to_string();
    assert!(matches!(error, BuildError::DuplicateSlot { .. }), "{text}");
    assert!(
        text.contains("test.first") && text.contains("torque.net-codecs"),
        "{text}"
    );
}

/// Torque's codecs under another game string.
#[derive(Debug)]
struct Named;

impl NetCodecs for Named {
    fn server_config(&self, max_players: usize) -> ServerConfig {
        let mut config = ServerConfig::new(max_players);
        config.profile.game = b"Another Game".to_vec();
        config
    }
}

#[test]
fn net_game_the_codecs_from_the_slot_are_the_hosts() {
    let engine = engine_with(|builder| {
        builder.add_plugins(NetCodecsPlugin::new(|_| Rc::new(Named)));
    });
    let server = GameServer::start_engine(&engine, files(), &start()).expect("starts");
    let shared = server.shared().expect("the network host").borrow();
    assert_eq!(shared.hs.config().profile.game, b"Another Game");
    assert_eq!(format!("{:?}", shared.codecs), "Named");
}

#[test]
fn net_game_the_dedicated_runner_starts_the_slots_net_game() {
    let made = Rc::new(Cell::new(0));
    let lines = Rc::new(RefCell::new(Vec::<String>::new()));
    let mut server = Dedicated::new(files(), start().listen, "startIt");
    server.ticks = Some(64);
    server.print = Some({
        let lines = Rc::clone(&lines);
        Rc::new(move |line: &str| lines.borrow_mut().push(line.to_string()))
    });
    let factory_made = Rc::clone(&made);
    EngineBuilder::new()
        .add_plugins((
            MinimalPlugins,
            NetGamePlugin::<TestNet>::new(move |_: &mut _, _: &_| {
                factory_made.set(factory_made.get() + 1);
                Ok(test_parts())
            }),
        ))
        .dedicated(server)
        .run()
        .expect("the server runs");
    assert_eq!(made.get(), 1);
    assert!(
        lines.borrow().iter().any(|l| l == "sixty"),
        "{:?}",
        lines.borrow()
    );
}

#[test]
fn net_game_start_net_game_names_the_slots_net_game() {
    let engine = engine_with(|builder| {
        builder.add_plugins(NetGamePlugin::<TestNet>::new(|_: &mut _, _: &_| {
            Ok(test_parts())
        }));
    });
    let mut server = start_net_game(&engine, files(), &start()).expect("starts");
    server.tick();
    assert!(
        server.net_game().ends_with("TestNet"),
        "{}",
        server.net_game()
    );
    let plain = start_net_game(&engine_with(|_| {}), files(), &start()).expect("starts");
    assert!(
        plain.net_game().ends_with("TorqueNet"),
        "{}",
        plain.net_game()
    );
}

#[test]
fn net_game_a_factory_error_stops_the_start_with_its_message() {
    let engine = engine_with(|builder| {
        builder.add_plugins(NetGamePlugin::<TestNet>::new(|_: &mut _, _: &_| {
            Err("no map table".to_string())
        }));
    });
    let Err(error) = GameServer::<TestNet>::start_with_net(&engine, files(), &start()) else {
        panic!("refused");
    };
    assert!(error.contains("no map table"), "{error}");
}

#[test]
fn net_game_the_server_game_slot_gives_every_server_its_own_parts() {
    let container = Rc::new(RefCell::new(None));
    let engine = engine_with(|builder| {
        let container = Rc::clone(&container);
        builder.add_plugins(ClosurePlugin::new("test.game", move |engine| {
            let container = Rc::clone(&container);
            engine.server_game(move |vm, wiring| {
                // The state is the server's net game's; another type is refused.
                assert!(wiring.state::<OtherNet>().is_err());
                let parts = torque_server::game::install(vm, wiring.state::<TestNet>()?);
                *container.borrow_mut() = Some(Rc::clone(&parts.container));
                vm.set_global("$OwnGame", "1");
                Ok(parts)
            });
        }));
        builder.add_plugins(NetGamePlugin::<TestNet>::new(|_: &mut _, _: &_| {
            Ok(test_parts())
        }));
    });
    let server = GameServer::<TestNet>::start_with_net(&engine, files(), &start()).expect("starts");
    let made = container.borrow().clone().expect("the slot made the parts");
    assert!(Rc::ptr_eq(&made, &server.game.container));
    assert_eq!(server.vm().global("$OwnGame").as_deref(), Some(&b"1"[..]));
}

#[test]
fn net_game_a_second_server_game_setter_fails_the_build_naming_both() {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(MinimalPlugins);
    for id in ["test.first", "test.second"] {
        builder.add_plugins(ClosurePlugin::new(id, |engine| {
            engine.server_game(|vm, wiring| Ok(wiring.tge_game(vm)));
        }));
    }
    let error = build_error(&mut builder);
    let text = error.to_string();
    assert!(
        text.contains("test.first") && text.contains("test.second"),
        "{text}"
    );
}

#[test]
fn net_game_the_warning_names_the_classes_and_what_does_not_carry_them() {
    assert_eq!(plugin_class_warning("G", &[], false, false), None);
    assert_eq!(plugin_class_warning("G", &["Rope"], true, true), None);
    let ghosts = plugin_class_warning("G", &["Rope", "RopeData"], false, true).expect("warns");
    assert!(ghosts.contains("Rope, RopeData") && ghosts.contains("its ghost game does not"));
    let codecs = plugin_class_warning("G", &["Rope"], true, false).expect("warns");
    assert!(codecs.contains("its codecs do not"), "{codecs}");
}
