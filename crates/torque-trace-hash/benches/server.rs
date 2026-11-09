//! A whole server tick of a busy game: the trace world's floor, 1,024 props (`PropsPlugin`) and 64 AIPlayers of the trace
//! figure. "Standing" has the Players still among the props, every prop asleep (the walks, the pushes, each Player's collision
//! phase over the props' shapes); "walking" has them walk back and forth on a strip beside the props. The allocations a tick of
//! each are printed ahead of the timings (a counting allocator). Run under the bench lock:
//! `cargo bench -p torque-trace-hash --bench server`.

use std::hint::black_box;
use std::net::{Ipv4Addr, SocketAddr};

use criterion::{Criterion, criterion_group};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use torque_app_server::plugins::MinimalPlugins;
use torque_app_server::server::{GameServer, Listen, ServerStart};
use torque_engine::EngineBuilder;
use torque_plugin_props::PropsPlugin;

#[global_allocator]
static GLOBAL: &StatsAlloc<std::alloc::System> = &INSTRUMENTED_SYSTEM;

const PROPS: u32 = 1024;
const PLAYERS: u32 = 64;

/// The bench's start function, over the trace world's script: props in a 32-wide grid 1.6 apart from the origin, Players
/// standing in the gaps between them or walking from x = -40 to -10 and back on rows 2 apart.
const BENCH: &str = r#"
datablock PropData(BenchCrate) { size = "0.8 0.8 0.8"; mass = 15; };

function TraceFigureData::onReachDestination(%this, %obj)
{
   %at = %obj.getPosition();
   %x = getWord(%at, 0) < -25 ? -10 : -40;
   %obj.setMoveDestination(%x SPC getWord(%at, 1) SPC 0);
}

function startBench(%walk)
{
   startTrace();
   TraceFigure.delete();
   TraceBuggy.delete();
   $Pref::Props::MaxAwake = 256;
   for (%i = 0; %i < 1024; %i++)
      propSpawn(BenchCrate, (%i % 32) * 1.6 SPC mFloor(%i / 32) * 1.6 SPC 0.4);
   for (%i = 0; %i < 64; %i++)
   {
      if (%walk)
         %at = (-40 + (%i % 4) * 8) SPC (mFloor(%i / 4) * 2) SPC 0.1;
      else
         %at = ((%i % 8) * 6.4 + 0.8) SPC (mFloor(%i / 8) * 6.4 + 0.8) SPC 0.1;
      %p = new AIPlayer() { dataBlock = TraceFigureData; position = %at; rotation = "1 0 0 0"; };
      MissionCleanup.add(%p);
      if (%walk)
         %p.setMoveDestination("-10" SPC getWord(%at, 1) SPC 0);
   }
}
"#;

fn server(walk: bool) -> GameServer {
    let engine = EngineBuilder::new()
        .add_plugins(MinimalPlugins)
        .add_plugins(PropsPlugin)
        .build()
        .expect("the engine");
    let files = torque_trace_hash::rig::files();
    let mut script = include_str!("../src/game/server.cs").to_owned();
    script.push_str(BENCH);
    files.insert("server.cs", script.as_bytes());
    let arg = if walk { "1" } else { "0" };
    let mut s = GameServer::start_engine(
        &engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "startBench",
            args: &[arg],
        },
    )
    .expect("the server");
    for _ in 0..160 {
        s.tick();
    }
    let counts = s
        .vm_mut()
        .eval(b"return propCount() SPC propAwakeCount();")
        .expect("the counts");
    eprintln!(
        "{}: props, awake: {}",
        if walk { "walking" } else { "standing" },
        String::from_utf8_lossy(&counts)
    );
    s
}

fn bench(c: &mut Criterion) {
    for walk in [false, true] {
        let mut s = server(walk);
        let name = if walk {
            "walking beside"
        } else {
            "standing among"
        };
        c.bench_function(
            &format!("server tick, {PLAYERS} players {name} {PROPS} props"),
            |b| b.iter(|| s.tick()),
        );
        black_box(&s);
    }
}

criterion_group!(benches, bench);

fn main() {
    for walk in [false, true] {
        let mut s = server(walk);
        let region = Region::new(GLOBAL);
        for _ in 0..32 {
            s.tick();
        }
        eprintln!(
            "{}: {:.0} allocations a tick",
            if walk { "walking" } else { "standing" },
            region.change().allocations as f64 / 32.0
        );
    }
    benches();
    Criterion::default().configure_from_args().final_summary();
}
