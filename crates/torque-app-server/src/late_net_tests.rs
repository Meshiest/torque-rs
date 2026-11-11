//! A server started on `Listen::Late`: no network host until `GameServer::attach_net`, which attaches the same host a server
//! started on a transport has, or refuses with the server's rule, its passes attached last so a failed attach reads nothing
//! and a retry can succeed; and `start_with_net_files` over a file host of the test's own.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque_engine::permissions::RoleDef;
use torque_engine::{
    Engine, EngineBuilder, Plugin, PluginGroup, PluginType, Plugins, ProtocolKind,
};
use torque_server::game::GameParts;
use torque_server::net::torque::TorqueNet;
use torque_server::net::transport::{
    BindError, PlainEngineRefusal, Recv, SharedTransport, Transport,
};
use torque_ts_vm::host::{FileHost, FileRead, MemFiles};
use torque_ts_vm::vm::Vm;

use crate::net_game::{NetGameFactory, NetGamePlugin, TorqueNetFactory};
use crate::plugins::{MinimalPlugins, NetHostPlugin, set_protocol};
use crate::server::{AttachError, AttachNetError, GameServer, Listen, ServerLink, ServerStart};

const AT: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);
const PROTOCOLS: [ProtocolKind; 2] = [ProtocolKind::Tge, ProtocolKind::Engine];

fn engine_with<M>(kind: ProtocolKind, plugins: impl Plugins<M>) -> Engine {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(plugins);
    set_protocol(&mut builder, kind);
    builder.build().expect("the engine builds")
}

fn engine(kind: ProtocolKind) -> Engine {
    engine_with(kind, MinimalPlugins)
}

fn files(script: &str) -> MemFiles {
    let files = MemFiles::new();
    files.insert("server.cs", script.as_bytes());
    files
}

fn start_on(engine: &Engine, files: MemFiles, listen: Listen) -> GameServer {
    GameServer::start_engine(
        engine,
        files,
        &ServerStart {
            listen,
            globals: &[],
            start: "startNothing",
            args: &[],
        },
    )
    .expect("the server starts")
}

fn quiet() -> MemFiles {
    files("function startNothing() {}")
}

fn memory() -> ServerLink {
    ServerLink::memory(AT).expect("the link binds")
}

#[test]
fn late_net_a_server_started_late_has_none_of_the_hosts_natives() {
    for kind in PROTOCOLS {
        let engine = engine(kind);
        let late = start_on(&engine, quiet(), Listen::Late);
        let normal = start_on(&engine, quiet(), Listen::Memory(AT));
        assert!(late.shared().is_none() && late.server.net_host().is_none());
        assert!(late.address().is_none() && late.memory_end().is_none());
        let late_natives = late.vm().native_listing();
        let normal_natives = normal.vm().native_listing();
        let late_lines: BTreeSet<&str> = late_natives.lines().collect();
        let normal_lines: BTreeSet<&str> = normal_natives.lines().collect();
        assert!(
            late_lines.is_subset(&normal_lines),
            "{kind:?}: every native of the late server is a normal server's"
        );
        let host: Vec<&str> = normal_lines.difference(&late_lines).copied().collect();
        for native in [
            "GameConnection::getIdentity ",
            "commandToClient ",
            "definePermissionRole ",
        ] {
            assert!(
                host.iter().any(|line| line.starts_with(native)),
                "{kind:?}: the host's {native} natives are missing before the attach: {host:?}"
            );
        }
    }
}

#[test]
fn late_net_after_the_attach_the_natives_are_a_normal_servers() {
    for kind in PROTOCOLS {
        let engine = engine(kind);
        let mut late = start_on(&engine, quiet(), Listen::Late);
        late.attach_net(memory()).expect("the host attaches");
        let normal = start_on(&engine, quiet(), Listen::Memory(AT));
        assert_eq!(
            late.vm().native_listing(),
            normal.vm().native_listing(),
            "{kind:?}"
        );
        assert!(late.shared().is_some() && late.server.net_host().is_some());
        assert_eq!(late.address(), Some(AT));
        assert!(late.memory_end().is_some());
    }
}

#[test]
fn late_net_the_attach_keeps_the_connections_as_the_start_left_them() {
    let engine = engine(ProtocolKind::Tge);
    let mut late = start_on(&engine, quiet(), Listen::Late);
    assert!(late.server.state.borrow().allow_connections);
    late.attach_net(memory()).expect("the host attaches");
    assert!(late.server.state.borrow().allow_connections);
}

#[test]
fn late_net_a_server_with_a_host_refuses_another() {
    let engine = engine(ProtocolKind::Tge);
    let mut normal = start_on(&engine, quiet(), Listen::Memory(AT));
    assert_eq!(
        normal.attach_net(memory()),
        Err(AttachNetError::Refused(AttachError::AlreadyAttached))
    );
    let mut late = start_on(&engine, quiet(), Listen::Late);
    late.attach_net(memory()).expect("the host attaches");
    assert_eq!(
        late.attach_net(memory()),
        Err(AttachNetError::Refused(AttachError::AlreadyAttached))
    );
}

#[test]
fn late_net_a_datablock_made_before_the_attach_refuses_it() {
    let engine = engine(ProtocolKind::Tge);
    let script = "datablock StaticShapeData(EarlyBlock) {};\nfunction startNothing() {}";
    let mut late = start_on(&engine, files(script), Listen::Late);
    let error = late.attach_net(memory()).expect_err("refused");
    assert_eq!(
        error,
        AttachNetError::Refused(AttachError::DatablocksAlreadyExist)
    );
    assert!(
        error.to_string().contains("datablocks already exist"),
        "{error}"
    );
    assert!(late.shared().is_none());
}

/// A transport bound at `at` that counts what the server asks of it.
struct Socket {
    at: SocketAddr,
    real: bool,
    calls: Rc<Cell<u32>>,
}

impl Socket {
    fn shared(at: SocketAddr, real: bool) -> (SharedTransport, Rc<Cell<u32>>) {
        let calls = Rc::new(Cell::new(0));
        let socket = Socket {
            at,
            real,
            calls: Rc::clone(&calls),
        };
        (Rc::new(RefCell::new(socket)), calls)
    }

    fn count(&self) {
        self.calls.set(self.calls.get() + 1);
    }
}

impl Transport for Socket {
    fn bind(&mut self, _: IpAddr, _: u16) -> Result<(), BindError> {
        self.count();
        Ok(())
    }

    fn recv(&mut self, _: &mut [u8]) -> Recv {
        self.count();
        Recv::Empty
    }

    fn send_to(&mut self, _: SocketAddr, _: &[u8]) -> bool {
        self.count();
        true
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        Some(self.at)
    }

    fn is_real(&self) -> bool {
        self.real
    }
}

fn real_socket() -> SharedTransport {
    Socket::shared(AT, true).0
}

#[test]
fn late_net_a_real_socket_needs_the_system_clock() {
    let engine = engine(ProtocolKind::Tge);
    let mut late = start_on(&engine, quiet(), Listen::Late);
    let error = late.attach_net(real_socket()).expect_err("refused");
    assert_eq!(
        error,
        AttachNetError::Refused(AttachError::RealSocketNeedsSystemClock)
    );
    assert!(error.to_string().contains("system clock"), "{error}");
    late.server.use_system_clock();
    late.attach_net(real_socket())
        .expect("the host attaches on the system clock");
    assert_eq!(late.address(), Some(AT));
}

#[test]
fn late_net_an_engine_without_the_network_host_fails_the_attach() {
    let engine = engine_with(ProtocolKind::Tge, MinimalPlugins.disable::<NetHostPlugin>());
    let mut late = start_on(&engine, quiet(), Listen::Late);
    let Err(AttachNetError::Failed(error)) = late.attach_net(memory()) else {
        panic!("the attach fails");
    };
    assert!(error.contains("NetHostPlugin"), "{error}");
    let Err(AttachNetError::Failed(again)) = late.attach_net(memory()) else {
        panic!("a second attach fails too");
    };
    assert!(again.contains("NetHostPlugin"), "{again}");
}

/// A plugin that defines a role no server can hold, which only the permissions made for a server's host refuse.
#[derive(torque_engine::PluginId)]
#[plugin(id = "test.bad-role")]
struct BadRole;

impl Plugin for BadRole {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.push(RoleDef {
            plugin: BadRole::ID,
            name: "not a role name".to_string(),
            grants: Vec::new(),
        });
    }
}

#[test]
fn late_net_a_step_that_fails_after_the_hosts_attach_leaves_the_transport_unread() {
    let engine = engine_with(ProtocolKind::Tge, (MinimalPlugins, BadRole));
    let mut late = start_on(&engine, quiet(), Listen::Late);
    let (socket, calls) = Socket::shared(AT, false);
    let Err(AttachNetError::Failed(error)) = late.attach_net(Rc::clone(&socket)) else {
        panic!("the attach fails");
    };
    assert!(error.contains("permissions"), "{error}");
    assert!(late.server.net_host().is_none() && late.shared().is_none());
    assert!(late.server.state.borrow().allow_connections);
    for _ in 0..4 {
        late.tick();
    }
    assert_eq!(calls.get(), 0, "no pass read or wrote the transport");
    let Err(AttachNetError::Failed(again)) = late.attach_net(socket) else {
        panic!("a retry makes the host again, and fails the same way");
    };
    assert!(again.contains("permissions"), "{again}");
    late.tick();
    assert_eq!(calls.get(), 0);
}

#[test]
fn late_net_a_retry_after_a_failed_factory_attaches_the_host() {
    let made = Rc::new(Cell::new(0u32));
    let factory = {
        let made = Rc::clone(&made);
        move |vm: &mut Vm, game: &GameParts| {
            made.set(made.get() + 1);
            if made.get() == 1 {
                return Err("not yet".to_string());
            }
            TorqueNetFactory.make(vm, game)
        }
    };
    let engine = engine_with(
        ProtocolKind::Tge,
        (MinimalPlugins, NetGamePlugin::<TorqueNet>::new(factory)),
    );
    let mut late = start_on(&engine, quiet(), Listen::Late);
    let Err(AttachNetError::Failed(error)) = late.attach_net(memory()) else {
        panic!("the first attach fails");
    };
    assert!(error.contains("not yet"), "{error}");
    late.attach_net(memory()).expect("the retry attaches");
    let normal = start_on(&engine, quiet(), Listen::Memory(AT));
    assert_eq!(late.vm().native_listing(), normal.vm().native_listing());
    assert!(late.server.net_host().is_some());
}

#[test]
fn late_net_a_plain_engine_socket_outside_the_private_addresses_is_refused() {
    let every = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 28_000));
    let engine = engine(ProtocolKind::Engine);
    let mut late = start_on(&engine, quiet(), Listen::Late);
    late.server.use_system_clock();
    let (public, calls) = Socket::shared(every, true);
    let error = late.attach_net(public).expect_err("refused");
    assert_eq!(
        error,
        AttachNetError::PlainEngineBind(PlainEngineRefusal(every.ip()))
    );
    assert!(
        error.to_string().contains("private addresses only"),
        "{error}"
    );
    late.tick();
    assert_eq!(calls.get(), 0);
    late.attach_net(real_socket())
        .expect("a private address attaches after the refusal");
    // Torque's protocol keeps its own rule for a public socket: the bind policy's.
    let tge = engine_with(ProtocolKind::Tge, MinimalPlugins);
    let mut late = start_on(&tge, quiet(), Listen::Late);
    late.server.use_system_clock();
    late.attach_net(Socket::shared(every, true).0)
        .expect("Torque's protocol attaches on every interface");
}

/// A file host of the test's own over `files`, counting its reads.
struct Counting {
    files: MemFiles,
    reads: Rc<Cell<u32>>,
}

impl FileHost for Counting {
    fn read(&self, path: &[u8]) -> FileRead {
        self.reads.set(self.reads.get() + 1);
        self.files.read(path)
    }
}

#[test]
fn late_net_start_with_net_files_reads_through_the_games_host() {
    let reads = Rc::new(Cell::new(0));
    let host = Counting {
        files: files("$Entry = 1;\nfunction startNothing() { $Started = 1; }"),
        reads: Rc::clone(&reads),
    };
    let server = GameServer::<TorqueNet>::start_with_net_files(
        &engine(ProtocolKind::Tge),
        host,
        &ServerStart {
            listen: Listen::Memory(AT),
            globals: &[],
            start: "startNothing",
            args: &[],
        },
    )
    .expect("the server starts");
    assert!(reads.get() > 0, "server.cs was read through the host");
    assert_eq!(
        server.vm().global("$Entry").as_deref(),
        Some(b"1".as_slice())
    );
    assert_eq!(
        server.vm().global("$Started").as_deref(),
        Some(b"1".as_slice())
    );
    assert!(server.shared().is_some());
}
