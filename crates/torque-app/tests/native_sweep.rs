//! The natives a server of the default plugins adds to the script runtime's, called from script with hostile arguments
//! (the runtime's own sweep, `torque-ts-vm/tests/fuzz.rs`, covers its natives): each call ends without a panic and in time
//! in proportion to its steps. The default run calls a fixed eighth of them; `TORQUE_FUZZ_VM=<n>` calls them all with
//! 1 MiB arguments.

use crate::sweep;

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};

use sweep::{argument_lists, call_from_script, hammer, hostile_values, natives, rounds, seed};
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

fn server() -> GameServer {
    let files = MemFiles::new();
    files.insert("server.cs", b"function startNothing() {}");
    GameServer::start(
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals: &[],
            start: "startNothing",
            args: &[],
        },
    )
    .expect("the server starts")
}

#[test]
fn the_servers_natives_survive_hostile_arguments() {
    let (_, long_run) = rounds(0);
    let long = if long_run { 1 << 20 } else { 64 << 10 };
    let values = hostile_values(long);
    let own: HashSet<String> = natives(&Vm::new())
        .into_iter()
        .map(|n| format!("{:?}::{}", n.namespace, n.name))
        .collect();
    let game: Vec<_> = natives(server().vm())
        .into_iter()
        .filter(|n| !own.contains(&format!("{:?}::{}", n.namespace, n.name)))
        .collect();
    assert!(game.len() > 50, "{} natives of the game's", game.len());
    let pick = seed() as usize % 8;
    for (n, native) in game.iter().enumerate() {
        if !long_run && n % 8 != pick {
            continue;
        }
        let mut server = server();
        for (i, args) in argument_lists(native, &values).iter().enumerate() {
            let what = || format!("{native:?} call {i}");
            hammer(&what, server.vm_mut(), 2_000_000, |case| {
                call_from_script(case, native, args);
            });
        }
    }
}
