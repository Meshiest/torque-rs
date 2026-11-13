//! Hostile peers on the loopback never stall a tick: a line that never ends, a stream that never stops, and bytes that drip
//! in one at a time. Each pass is timed; the bound is loose (a debug build on a busy machine) but far below what a blocked
//! read or an unbounded pass would take.

use crate::common;

use std::io::Write;
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

use common::{listener, pump, vm_limited};
use torque_plugin_tcp::{TcpLimits, poll};
use torque_ts_vm::testing::{eval, global};
use torque_ts_vm::vm::Vm;

/// The longest one pass may take.
const MAX_PASS: Duration = Duration::from_millis(250);

/// An object connected to a peer that runs `peer` on its end of the connection.
fn hostile(limits: TcpLimits, peer: impl FnOnce(TcpStream) + Send + 'static) -> Vm {
    let (listen, port) = listener();
    thread::spawn(move || {
        if let Ok((stream, _)) = listen.accept() {
            peer(stream);
        }
    });
    let mut vm = vm_limited(limits);
    eval(
        &mut vm,
        &format!(
            r#"function Victim::onLine(%this, %line) {{ $lines++; }}
               function Victim::onDisconnect(%this) {{ $down = 1; }}
               new TCPObject(Victim); Victim.connect("127.0.0.1:{port}");"#
        ),
    );
    vm
}

/// Runs `n` passes a millisecond apart; the longest one, and the most a pass read.
fn timed(vm: &mut Vm, n: usize) -> (Duration, usize) {
    let mut longest = Duration::ZERO;
    let mut most = 0;
    for _ in 0..n {
        let start = Instant::now();
        let report = poll(vm);
        longest = longest.max(start.elapsed());
        most = most.max(report.read);
        thread::sleep(Duration::from_millis(1));
    }
    (longest, most)
}

#[test]
fn a_line_that_never_ends_is_cut_off_at_the_cap() {
    let mut vm = hostile(TcpLimits::default(), |mut stream| {
        let block = [b'x'; 8_192];
        while stream.write_all(&block).is_ok() {}
    });
    let start = Instant::now();
    let mut longest = Duration::ZERO;
    let done = pump(&mut vm, |vm| {
        longest = longest.max(start.elapsed());
        global(vm, "$down") == "1"
    });
    assert!(done, "the peer is disconnected");
    assert_eq!(
        global(&vm, "$lines"),
        "",
        "no part of the line is delivered"
    );
    assert!(
        vm.log_texts()
            .iter()
            .any(|l| l.contains("a line longer than"))
    );
    let (longest, _) = timed(&mut vm, 5);
    assert!(longest < MAX_PASS, "a pass took {longest:?}");
}

#[test]
fn a_stream_that_never_stops_is_read_a_capped_slice_per_tick() {
    let limits = TcpLimits::default();
    let cap = limits.read_per_connection;
    let mut vm = hostile(limits, |mut stream| {
        let block = b"spam\n".repeat(2_000);
        while stream.write_all(&block).is_ok() {}
    });
    assert!(pump(&mut vm, |vm| !global(vm, "$lines").is_empty()));
    let (longest, most) = timed(&mut vm, 100);
    assert!(longest < MAX_PASS, "a pass took {longest:?}");
    assert!(most <= cap, "a pass read {most}");
    assert!(global(&vm, "$lines").parse::<u64>().expect("a count") > 1_000);
    // The script can still hang up on it.
    eval(&mut vm, "Victim.disconnect();");
    let (longest, most) = timed(&mut vm, 5);
    assert!(longest < MAX_PASS);
    assert_eq!(most, 0);
}

#[test]
fn bytes_that_drip_in_never_make_a_tick_wait() {
    let mut vm = hostile(TcpLimits::default(), |mut stream| {
        for &b in b"drip\nslow\n" {
            if stream.write_all(&[b]).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
    let mut longest = Duration::ZERO;
    let start = Instant::now();
    while global(&vm, "$lines") != "2" && start.elapsed() < common::WAIT {
        let pass = Instant::now();
        poll(&mut vm);
        longest = longest.max(pass.elapsed());
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(global(&vm, "$lines"), "2");
    assert!(
        longest < Duration::from_millis(50),
        "a pass waited {longest:?}"
    );
}

#[test]
fn many_short_lines_are_bounded_by_the_callback_cap() {
    let limits = TcpLimits::default();
    let callbacks = limits.callbacks_per_tick;
    let mut vm = hostile(limits, |mut stream| {
        let block = b"\n".repeat(16_384);
        while stream.write_all(&block).is_ok() {}
    });
    assert!(pump(&mut vm, |vm| !global(vm, "$lines").is_empty()));
    let mut most = 0;
    let mut longest = Duration::ZERO;
    for _ in 0..50 {
        let start = Instant::now();
        let report = poll(&mut vm);
        longest = longest.max(start.elapsed());
        most = most.max(report.callbacks);
    }
    assert!(most <= callbacks, "a pass made {most} callbacks");
    assert!(longest < MAX_PASS * 4, "a pass took {longest:?}");
}
