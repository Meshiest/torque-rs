//! `ServerTick` hooks on a real server: once for every 32 ms tick, before and after the process list's pass, in the order
//! pushed, with the tick's number and time, with no effect on a server that has none.

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque_engine::{
    Engine, EngineBuilder, Plugin, PluginId, PluginType, ServerTick, TickInfo, VmMut,
};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

use crate::plugins::MinimalPlugins;
use crate::server::{GameServer, Listen, ServerStart};

type Log = Rc<RefCell<Vec<String>>>;

/// A plugin that pushes the hooks `push` makes.
struct Hooks {
    id: &'static str,
    push: Box<dyn Fn(&mut EngineBuilder)>,
}

impl Plugin for Hooks {
    fn build(&self, engine: &mut EngineBuilder) {
        (self.push)(engine);
    }

    fn id(&self) -> Option<&str> {
        Some(self.id)
    }
}

impl PluginType for Hooks {
    const ID: PluginId = PluginId::new("test.hooks");
}

fn plugin(id: &'static str, push: impl Fn(&mut EngineBuilder) + 'static) -> Hooks {
    Hooks {
        id,
        push: Box::new(push),
    }
}

/// Hooks that log `name.before n` and `name.after n` with the script's `$Count` they see.
fn logging(name: &'static str, log: &Log) -> Hooks {
    let log = Rc::clone(log);
    plugin(name, move |engine| {
        let before = Rc::clone(&log);
        engine.push(ServerTick::before(
            move |vm: &mut Vm, _: &Engine, tick: TickInfo| -> Result<(), String> {
                before.borrow_mut().push(format!(
                    "{name}.before {} {} count={}",
                    tick.number,
                    tick.time_ms,
                    count(vm)
                ));
                Ok(())
            },
        ));
        let after = Rc::clone(&log);
        engine.push(ServerTick::after(
            move |vm: &mut Vm, _: &Engine, tick: TickInfo| -> Result<(), String> {
                after.borrow_mut().push(format!(
                    "{name}.after {} {} count={}",
                    tick.number,
                    tick.time_ms,
                    count(vm)
                ));
                Ok(())
            },
        ));
    })
}

fn count(vm: &Vm) -> String {
    vm.global("$Count")
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_default()
}

fn engine(plugins: Vec<Hooks>) -> Engine {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(MinimalPlugins);
    for plugin in plugins {
        builder.add_plugins(plugin);
    }
    builder.build().expect("the engine builds")
}

/// A server whose script counts its own events: one a tick, in the Sim event queue.
fn start(engine: &Engine) -> GameServer {
    let files = MemFiles::new();
    files.insert(
        "server.cs",
        b"$Count = 0;\n\
          function startIt() { bump(); }\n\
          function bump() { $Count++; schedule(32, 0, \"bump\"); }",
    );
    GameServer::start_engine(
        engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "startIt",
            args: &[],
        },
    )
    .expect("the server starts")
}

/// The ticks a server has run after `passes` calls of `tick()`: the first covers its start-up, 32 ticks at once.
fn ticks_after(passes: u64) -> u64 {
    32 + (passes - 1)
}

fn run(server: &mut GameServer, passes: u64) {
    for _ in 0..passes {
        server.tick();
    }
}

#[test]
fn hooks_run_once_a_tick_in_order_over_600_passes_and_see_the_ticks_time() {
    let log: Log = Log::default();
    let engine = engine(vec![logging("a", &log), logging("b", &log)]);
    let mut server = start(&engine);
    run(&mut server, 600);
    let log = log.borrow();
    let total = ticks_after(600);
    assert_eq!(log.len() as u64, total * 4, "four hooks, once a tick");
    // The first pass is 32 ticks at once: every `before`, the process list, every `after`; each in registration order.
    let first: Vec<String> = (1..=32)
        .flat_map(|n| [format!("a.before {n}"), format!("b.before {n}")])
        .chain((1..=32).flat_map(|n| [format!("a.after {n}"), format!("b.after {n}")]))
        .collect();
    let seen: Vec<String> = log[..128]
        .iter()
        .map(|line| line.split(" count").next().unwrap().to_string())
        .map(|line| {
            let mut parts = line.split(' ');
            format!("{} {}", parts.next().unwrap(), parts.next().unwrap())
        })
        .collect();
    assert_eq!(seen, first);
    // After it, each tick is a before pair, then an after pair.
    for (i, chunk) in log[128..].chunks(4).enumerate() {
        let n = 33 + i as u64;
        let time = n * 32;
        let names: Vec<&str> = chunk.iter().map(|l| l.split(' ').next().unwrap()).collect();
        assert_eq!(
            names,
            ["a.before", "b.before", "a.after", "b.after"],
            "{chunk:?}"
        );
        for line in chunk {
            let mut parts = line.split(' ');
            let (_, number, ms) = (parts.next(), parts.next(), parts.next());
            assert_eq!(number, Some(n.to_string().as_str()), "{line}");
            assert_eq!(ms, Some(time.to_string().as_str()), "{line}");
        }
    }
    // Hooks run before the Sim events of their pass: within a tick both sides see the same count, and it grows by one a
    // tick (the script's event runs once the hooks have).
    let counts: Vec<i64> = log[128..]
        .chunks(4)
        .map(|chunk| {
            let all: Vec<&str> = chunk
                .iter()
                .map(|l| l.rsplit("count=").next().unwrap())
                .collect();
            assert!(all.iter().all(|c| *c == all[0]), "{chunk:?}");
            all[0].parse().unwrap()
        })
        .collect();
    assert!(
        counts.windows(2).all(|w| w[1] == w[0] + 1),
        "{:?}",
        &counts[..8]
    );
}

#[test]
fn a_plugin_ticks_with_no_script_function_to_call() {
    // A hook that owns its state in the script engine's globals and counts ticks itself: the game's script defines nothing
    // for it.
    let engine = engine(vec![plugin("test.counter", |engine| {
        engine.push(ServerTick::after(|mut vm: VmMut, tick: TickInfo| {
            vm.set_global("$Plugin::Ticks", &tick.number.to_string());
        }));
    })]);
    let mut server = start(&engine);
    run(&mut server, 100);
    assert_eq!(
        server.vm().global("$Plugin::Ticks").as_deref(),
        Some(ticks_after(100).to_string().as_bytes())
    );
}

#[test]
fn the_same_engine_makes_servers_with_the_same_ticks() {
    let log: Log = Log::default();
    let engine = engine(vec![logging("a", &log)]);
    let mut first = start(&engine);
    run(&mut first, 200);
    let first_log = std::mem::take(&mut *log.borrow_mut());
    let mut second = start(&engine);
    run(&mut second, 200);
    assert_eq!(first_log, *log.borrow());
    assert_eq!(count(first.vm()), count(second.vm()));
}

#[test]
fn a_server_with_hooks_runs_its_script_as_one_without() {
    let plain = engine(vec![plugin("test.none", |_| {})]);
    let log: Log = Log::default();
    let hooked = engine(vec![logging("a", &log)]);
    let (mut plain, mut hooked) = (start(&plain), start(&hooked));
    run(&mut plain, 300);
    run(&mut hooked, 300);
    assert_eq!(count(plain.vm()), count(hooked.vm()));
    assert_eq!(plain.vm().sim_time(), hooked.vm().sim_time());
    assert_eq!(plain.drain_log(), hooked.drain_log());
}

#[test]
fn a_failing_hook_says_so_once_and_the_others_go_on() {
    let log: Log = Log::default();
    let engine = engine(vec![
        plugin("test.fails", |engine| {
            engine.push(ServerTick::before(|_: &mut Vm, _: &Engine, _: TickInfo| {
                Err::<(), String>("no good".to_string())
            }));
        }),
        logging("a", &log),
    ]);
    let mut server = start(&engine);
    run(&mut server, 50);
    let lines = server.drain_log();
    let said: Vec<&String> = lines.iter().filter(|l| l.contains("no good")).collect();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("hook 0"), "{}", said[0]);
    assert_eq!(log.borrow().len() as u64, ticks_after(50) * 2);
}
