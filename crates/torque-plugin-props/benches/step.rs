//! The cost of the server's props each tick: the world alone (the step, the writeback, sleep and the cap) and a whole server
//! tick of a game server with the plugin (the world, the record fields, the scripts' callbacks), for 256 props awake and for
//! 1,024 asleep. Run under the bench lock: `cargo bench -p torque-plugin-props --bench step`.

use std::hint::black_box;
use std::net::{Ipv4Addr, SocketAddr};

use criterion::{Criterion, criterion_group, criterion_main};
use torque_app_server::plugins::MinimalPlugins;
use torque_app_server::server::{GameServer, Listen, ServerStart};
use torque_engine::EngineBuilder;
use torque_plugin_props::PropsPlugin;
use torque_plugin_props::data::{PropNumbers, PropParams};
use torque_plugin_props::world::{PropLimits, PropWorld, StaticPiece};
use torque_ts_vm::host::MemFiles;

/// A crate that never sleeps (`awake`) or one that does.
fn params(awake: bool) -> PropParams {
    let (sleep_linear, sleep_angular) = if awake { (0.0, 0.0) } else { (0.1, 0.2) };
    PropNumbers {
        size: [0.8; 3],
        mass: 15.0,
        sleep_linear,
        sleep_angular,
        ..PropNumbers::default()
    }
    .held()
}

/// `n` crates in a grid on a floor, each a little above it, ticked until they rest (and, when they may, sleep).
fn world(n: u32, awake: bool) -> PropWorld {
    let mut w = PropWorld::new(
        4,
        PropLimits {
            awake: 1024,
            ..PropLimits::default()
        },
    );
    w.sync_statics(vec![(
        1,
        StaticPiece::Box {
            position: [0.0, 0.0, -1.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            half: [500.0, 500.0, 1.0],
        },
    )]);
    let side = (n as f32).sqrt().ceil() as u32;
    for k in 0..n {
        let at = [(k % side) as f32 * 1.2, (k / side) as f32 * 1.2, 0.45];
        w.spawn(
            100 + k,
            9,
            params(awake),
            (at, [0.0, 0.0, 0.0, 1.0]),
            None,
            0,
        )
        .expect("a crate");
    }
    for _ in 0..120 {
        w.tick(&[], &|_| 0.0);
    }
    w
}

/// A game server with the plugin and `n` crates spawned by its script on a frozen floor prop, run until they rest.
fn server(n: u32, awake: bool) -> GameServer {
    let engine = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .add_plugins(PropsPlugin)
        .build()
        .expect("the engine");
    let files = MemFiles::new();
    let sleep = if awake {
        "sleepLinear = 0; sleepAngular = 0;"
    } else {
        ""
    };
    let side = (n as f32).sqrt().ceil() as u32;
    let script = format!(
        "datablock PropData(BenchCrate) {{ size = \"0.8 0.8 0.8\"; mass = 15; {sleep} }};
         datablock PropData(BenchFloor) {{ size = \"128 128 2\"; }};
         function startBench() {{
            $Pref::Props::MaxAwake = 1024;
            propFreeze(propSpawn(BenchFloor, \"16 16 -1\"));
            for (%i = 0; %i < {n}; %i++) propSpawn(BenchCrate, (%i % {side}) * 1.2 SPC mFloor(%i / {side}) * 1.2 SPC 0.4);
         }}"
    );
    files.insert("server.cs", script.as_bytes());
    let mut s = GameServer::start_engine(
        &engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "startBench",
            args: &[],
        },
    )
    .expect("the server");
    for _ in 0..120 {
        s.tick();
    }
    let counts = s
        .vm_mut()
        .eval(b"return propAwakeCount() SPC propCount();")
        .expect("the counts");
    let awake_now = if awake { n } else { 0 };
    assert_eq!(
        String::from_utf8_lossy(&counts),
        format!("{awake_now} {}", n + 1)
    );
    s
}

fn bench(c: &mut Criterion) {
    let mut w = world(256, true);
    assert_eq!(w.awake(), 256);
    c.bench_function("world tick, 256 awake", |b| {
        b.iter(|| black_box(w.tick(&[], &|_| 0.0)))
    });
    let mut w = world(1024, false);
    assert_eq!(w.awake(), 0);
    c.bench_function("world tick, 1024 asleep", |b| {
        b.iter(|| black_box(w.tick(&[], &|_| 0.0)))
    });
    let mut s = server(256, true);
    c.bench_function("server tick, 256 awake", |b| b.iter(|| s.tick()));
    // The floor is a prop too: 1,023 crates and the floor make the server's 1,024.
    let mut s = server(1023, false);
    c.bench_function("server tick, 1024 asleep", |b| b.iter(|| s.tick()));
}

criterion_group!(benches, bench);
criterion_main!(benches);
