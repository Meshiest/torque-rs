//! Each cap of [`TcpLimits`] refusing past its limit, on the loopback.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use common::{closed_port, listener, pump, pump_with, ticks, vm_limited, vm_with};
use torque_plugin_tcp::{TcpLimits, TcpPlugin};
use torque_ts_vm::testing::{eval, global};

/// Accepts every connection on a fresh port and keeps them open; the count accepted so far.
fn sink() -> (u16, Arc<Mutex<Vec<TcpStream>>>) {
    let (peer, port) = listener();
    let held = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&held);
    thread::spawn(move || {
        for stream in peer.incoming().flatten() {
            keep.lock().expect("the list").push(stream);
        }
    });
    (port, held)
}

fn log_connects(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| {
            format!(
                r#"function {n}::onConnected(%this) {{ $log = $log @ "{n}:connected;"; }}
                   function {n}::onConnectFailed(%this) {{ $log = $log @ "{n}:failed;"; }}
                   new TCPObject({n});"#
            )
        })
        .collect()
}

#[test]
fn a_connect_past_the_socket_cap_warns_and_fails_on_the_next_tick() {
    let (port, _held) = sink();
    let mut vm = vm_limited(TcpLimits {
        sockets: 2,
        ..TcpLimits::default()
    });
    eval(&mut vm, &log_connects(&["A", "B", "C"]));
    eval(
        &mut vm,
        &format!(
            r#"A.connect("127.0.0.1:{port}"); B.connect("127.0.0.1:{port}"); C.connect("127.0.0.1:{port}");"#
        ),
    );
    assert!(
        vm.log_texts()
            .iter()
            .any(|l| l.contains("connect refused: 2 connections are open"))
    );
    assert_eq!(global(&vm, "$log"), "", "no callback runs inside connect");
    assert!(pump(&mut vm, |vm| {
        let log = global(vm, "$log");
        log.contains("A:connected") && log.contains("B:connected")
    }));
    assert!(
        global(&vm, "$log").starts_with("C:failed;"),
        "the refusal comes first: {}",
        global(&vm, "$log")
    );
}

#[test]
fn a_connect_past_the_lookup_cap_is_refused_while_a_lookup_runs() {
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    let plugin = TcpPlugin::default()
        .with_resolver(Arc::new(move |_, port| {
            let _ = wait
                .lock()
                .expect("the gate")
                .recv_timeout(Duration::from_secs(5));
            Ok(vec![std::net::SocketAddr::from(([127, 0, 0, 1], port))])
        }))
        .with_limits(TcpLimits {
            lookups: 1,
            ..TcpLimits::default()
        });
    let mut vm = vm_with(plugin);
    eval(&mut vm, &log_connects(&["Slow", "Next"]));
    eval(
        &mut vm,
        r#"Slow.connect("slow.test:1"); Next.connect("next.test:1");"#,
    );
    assert!(
        vm.log_texts()
            .iter()
            .any(|l| l.contains("connect refused: 1 connects are under way"))
    );
    assert!(pump(&mut vm, |vm| global(vm, "$log") == "Next:failed;"));
    release.send(()).expect("the lookup ends");
}

#[test]
fn a_listen_past_the_listener_cap_is_an_error() {
    let mut vm = vm_limited(TcpLimits {
        listeners: 1,
        ..TcpLimits::default()
    });
    let (first, second) = (closed_port(), closed_port());
    eval(
        &mut vm,
        &format!("new TCPObject(L1); L1.listen({first}); new TCPObject(L2); L2.listen({second});"),
    );
    assert!(
        vm.log_texts()
            .iter()
            .any(|l| l.contains("listen on port") && l.contains("1 listen sockets are open"))
    );
    assert!(
        TcpStream::connect(("127.0.0.1", first)).is_ok(),
        "the first listens"
    );
    // Listening again on the same object replaces its socket, within the cap.
    eval(&mut vm, &format!("L1.listen({second});"));
    assert!(TcpStream::connect(("127.0.0.1", second)).is_ok());
}

/// An object connected to a peer whose stream the test holds.
fn connected(limits: TcpLimits) -> (torque_ts_vm::vm::Vm, TcpStream) {
    let (peer, port) = listener();
    let mut vm = vm_limited(limits);
    eval(
        &mut vm,
        &format!(
            r#"function Pipe::onConnected(%this) {{ $up = 1; }}
               function Pipe::onLine(%this, %line) {{ $lines++; $last = %line; }}
               function Pipe::onDisconnect(%this) {{ $down = 1; }}
               new TCPObject(Pipe); Pipe.connect("127.0.0.1:{port}");"#
        ),
    );
    let (stream, _) = peer.accept().expect("the script connects");
    assert!(pump(&mut vm, |vm| global(vm, "$up") == "1"));
    (vm, stream)
}

#[test]
fn a_send_that_would_overflow_the_queue_is_refused_whole() {
    let (mut vm, mut stream) = connected(TcpLimits {
        send_queue: 16,
        ..TcpLimits::default()
    });
    eval(
        &mut vm,
        r#"Pipe.send("0123456789abcdef0123456789abcdef"); Pipe.send("fits\n");"#,
    );
    assert!(
        vm.log_texts()
            .iter()
            .any(|l| l.contains("send refused: more than 16 bytes"))
    );
    let mut got = [0u8; 5];
    stream.read_exact(&mut got).expect("the second send");
    assert_eq!(&got, b"fits\n");
}

#[test]
fn sending_past_the_tick_allowance_waits_for_later_ticks() {
    let (mut vm, mut stream) = connected(TcpLimits {
        send_per_tick: 1_000,
        ..TcpLimits::default()
    });
    eval(
        &mut vm,
        r#"function big() { for (%i = 0; %i < 500; %i++) %t = %t @ "0123456789"; return %t; } Pipe.send(big());"#,
    );
    let reader = thread::spawn(move || {
        let mut got = vec![0u8; 5_000];
        stream.read_exact(&mut got).expect("every byte");
        got
    });
    let mut most = 0;
    let mut passes = 0;
    assert!(pump_with(
        &mut vm,
        |_| reader.is_finished(),
        |report| {
            most = most.max(report.sent);
            passes += 1;
        }
    ));
    assert!(most <= 1_000, "a tick sent {most}");
    assert!(
        passes >= 4,
        "5,000 bytes take at least four more ticks after the first 1,000"
    );
    assert_eq!(reader.join().expect("the reader").len(), 5_000);
}

#[test]
fn reading_stops_at_the_tick_and_connection_caps_and_loses_nothing() {
    let (mut vm, mut stream) = connected(TcpLimits {
        read_per_tick: 4_096,
        read_per_connection: 2_048,
        ..TcpLimits::default()
    });
    let line = format!("{}\n", "y".repeat(99));
    let writer = thread::spawn(move || {
        for _ in 0..200 {
            stream.write_all(line.as_bytes()).expect("the lines");
        }
        stream
    });
    let mut most = 0;
    assert!(pump_with(
        &mut vm,
        |vm| global(vm, "$lines") == "200",
        |r| most = most.max(r.read)
    ));
    assert!(most <= 2_048, "one connection read {most} in a tick");
    drop(writer.join().expect("the writer"));
}

#[test]
fn a_line_past_the_cap_disconnects_the_peer_and_is_not_delivered() {
    let (mut vm, mut stream) = connected(TcpLimits {
        line: 1_024,
        ..TcpLimits::default()
    });
    stream.write_all(b"short\n").expect("a line");
    stream.write_all(&[b'x'; 2_000]).expect("a long line");
    assert!(pump(&mut vm, |vm| global(vm, "$down") == "1"));
    assert_eq!(
        (global(&vm, "$lines"), global(&vm, "$last")),
        ("1".into(), "short".into())
    );
    assert!(
        vm.log_texts()
            .iter()
            .any(|l| l.contains("a line longer than 1024 bytes"))
    );
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a timeout");
    let mut byte = [0u8; 1];
    assert!(
        matches!(stream.read(&mut byte), Ok(0) | Err(_)),
        "the peer is disconnected"
    );
}

#[test]
fn callbacks_past_the_tick_cap_wait_for_the_next_tick() {
    let (mut vm, mut stream) = connected(TcpLimits {
        callbacks_per_tick: 10,
        ..TcpLimits::default()
    });
    stream.write_all(&b"z\n".repeat(100)).expect("the lines");
    let mut most = 0;
    assert!(pump_with(
        &mut vm,
        |vm| global(vm, "$lines") == "100",
        |r| most = most.max(r.callbacks)
    ));
    assert_eq!(most, 10);
}

#[test]
fn an_accepted_connection_no_script_takes_is_closed() {
    let port = closed_port();
    let mut vm = vm_limited(TcpLimits {
        adopt_within: Duration::from_millis(100),
        ..TcpLimits::default()
    });
    eval(
        &mut vm,
        &format!(
            "function Door::onConnectRequest(%this, %a, %id) {{ $offered++; }} new TCPObject(Door); Door.listen({port});"
        ),
    );
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("the script listens");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a timeout");
    assert!(pump(&mut vm, |vm| global(vm, "$offered") == "1"));
    ticks(&mut vm, 1);
    thread::sleep(Duration::from_millis(150));
    ticks(&mut vm, 1);
    let mut byte = [0u8; 1];
    assert!(
        matches!(stream.read(&mut byte), Ok(0) | Err(_)),
        "the connection is closed"
    );
}

#[test]
fn accepted_connections_past_their_cap_are_closed_and_leave_the_scripts_own_alone() {
    let (out, _held) = sink();
    let port = closed_port();
    let mut vm = vm_limited(TcpLimits {
        sockets: 2,
        unadopted: 1,
        ..TcpLimits::default()
    });
    eval(
        &mut vm,
        &format!(
            "function Door::onConnectRequest(%this, %a, %id) {{ $offered++; }} new TCPObject(Door); Door.listen({port});"
        ),
    );
    let first = TcpStream::connect(("127.0.0.1", port)).expect("the first");
    assert!(pump(&mut vm, |vm| global(vm, "$offered") == "1"));
    let mut second =
        TcpStream::connect(("127.0.0.1", port)).expect("the second reaches the backlog");
    second
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a timeout");
    ticks(&mut vm, 5);
    let mut byte = [0u8; 1];
    assert!(
        matches!(second.read(&mut byte), Ok(0) | Err(_)),
        "the second is closed"
    );
    assert_eq!(global(&vm, "$offered"), "1");
    // The connection waiting to be taken does not count against the script's own two.
    eval(&mut vm, &log_connects(&["A", "B"]));
    eval(
        &mut vm,
        &format!(r#"A.connect("127.0.0.1:{out}"); B.connect("127.0.0.1:{out}");"#),
    );
    assert!(pump(&mut vm, |vm| {
        let log = global(vm, "$log");
        log.contains("A:connected") && log.contains("B:connected")
    }));
    assert!(!global(&vm, "$log").contains("failed"));
    drop(first);
}

#[test]
fn refused_connects_keep_one_callback_and_a_later_connect_drops_it() {
    let mut vm = vm_limited(TcpLimits {
        sockets: 0,
        ..TcpLimits::default()
    });
    eval(&mut vm, &log_connects(&["Loop"]));
    eval(
        &mut vm,
        r#"function hammer() { for (%i = 0; %i < 10000; %i++) Loop.connect("127.0.0.1:1"); } hammer();"#,
    );
    ticks(&mut vm, 5);
    assert_eq!(
        global(&vm, "$log"),
        "Loop:failed;",
        "one callback for 10,000 refusals"
    );

    // A refusal, then a connect that is not refused in the same tick: no stale onConnectFailed.
    let (out, _held) = sink();
    let mut vm = vm_limited(TcpLimits {
        sockets: 1,
        ..TcpLimits::default()
    });
    eval(&mut vm, &log_connects(&["A", "B"]));
    eval(
        &mut vm,
        &format!(
            r#"A.connect("127.0.0.1:{out}"); B.connect("127.0.0.1:{out}"); A.disconnect(); B.connect("127.0.0.1:{out}");"#
        ),
    );
    assert!(pump(&mut vm, |vm| global(vm, "$log") == "B:connected;"));
    ticks(&mut vm, 3);
    assert_eq!(global(&vm, "$log"), "B:connected;");
}

#[test]
fn lookups_that_hang_hold_the_cap_after_a_disconnect() {
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    let plugin = TcpPlugin::default()
        .with_resolver(Arc::new(move |_, port| {
            let _ = wait
                .lock()
                .expect("the gate")
                .recv_timeout(Duration::from_secs(5));
            Ok(vec![std::net::SocketAddr::from(([127, 0, 0, 1], port))])
        }))
        .with_limits(TcpLimits {
            lookups: 2,
            ..TcpLimits::default()
        });
    let mut vm = vm_with(plugin);
    eval(&mut vm, &log_connects(&["Spin"]));
    eval(
        &mut vm,
        r#"function spin() { for (%i = 0; %i < 20; %i++) { Spin.connect("hang.test:1"); Spin.disconnect(); } } spin();"#,
    );
    let refused = vm
        .log_texts()
        .iter()
        .filter(|l| l.contains("connect refused: 2 connects are under way"))
        .count();
    assert_eq!(
        refused, 18,
        "two threads hang, every later connect is refused"
    );
    for _ in 0..2 {
        let _ = release.send(());
    }
}

#[test]
fn adopting_past_the_socket_cap_closes_the_connection_instead() {
    let port = closed_port();
    let limit = 3;
    let mut vm = vm_limited(TcpLimits {
        sockets: limit,
        ..TcpLimits::default()
    });
    // The script feeds its own listen socket: every tick it connects out once more and adopts what comes in.
    eval(
        &mut vm,
        &format!(
            r#"function Door::onConnectRequest(%this, %a, %id) {{ new TCPObject(Taken, %id); $taken++; }}
               new TCPObject(Door); Door.listen({port});
               function more() {{ %c = new TCPObject(); %c.connect("127.0.0.1:{port}"); }}"#
        ),
    );
    let mut most = 0;
    for _ in 0..60 {
        eval(&mut vm, "more();");
        let report = torque_plugin_tcp::poll(&mut vm);
        most = most.max(report.sockets);
        thread::sleep(Duration::from_millis(2));
    }
    assert!(most <= limit, "{most} connections were open");
    assert!(
        global(&vm, "$taken").parse::<u32>().unwrap_or(0) > 0,
        "some were offered"
    );
}

#[test]
fn an_adoption_once_the_script_filled_the_cap_closes_the_waiting_connection() {
    let (out, _held) = sink();
    let port = closed_port();
    let mut vm = vm_limited(TcpLimits {
        sockets: 3,
        ..TcpLimits::default()
    });
    eval(
        &mut vm,
        &format!(
            r#"function Door::onConnectRequest(%this, %a, %id) {{ $waiting = %id; }}
               new TCPObject(Door); Door.listen({port});"#
        ),
    );
    let mut peer = TcpStream::connect(("127.0.0.1", port)).expect("a peer");
    peer.set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a timeout");
    assert!(pump(&mut vm, |vm| !global(vm, "$waiting").is_empty()));
    // The script's own three connections fill the cap while the peer waits; taking it then is refused.
    eval(&mut vm, &log_connects(&["A", "B", "C"]));
    eval(
        &mut vm,
        &format!(
            r#"A.connect("127.0.0.1:{out}"); B.connect("127.0.0.1:{out}"); C.connect("127.0.0.1:{out}");"#
        ),
    );
    assert!(pump(&mut vm, |vm| global(vm, "$log")
        .matches("connected")
        .count()
        == 3));
    eval(&mut vm, "$late = new TCPObject(Late, $waiting);");
    assert!(
        vm.log_texts()
            .iter()
            .any(|l| l.contains("accepted connection refused: 3 connections are open"))
    );
    assert_eq!(
        eval(&mut vm, "return isObject($late);"),
        "1",
        "the object exists, unconnected"
    );
    let mut byte = [0u8; 1];
    assert!(
        matches!(peer.read(&mut byte), Ok(0) | Err(_)),
        "the peer is closed"
    );
}

#[test]
fn peers_a_script_adopts_at_once_never_pass_the_socket_cap() {
    let port = closed_port();
    let limit = TcpLimits::default().sockets;
    let mut vm = vm_limited(TcpLimits::default());
    eval(
        &mut vm,
        &format!(
            r#"function Door::onConnectRequest(%this, %a, %id) {{ new TCPObject(Taken, %id); }}
               new TCPObject(Door); Door.listen({port});"#
        ),
    );
    let mut peers = Vec::new();
    let mut most = 0;
    for round in 0..100 {
        if round < 50 {
            peers.push(TcpStream::connect(("127.0.0.1", port)).expect("a peer"));
        }
        let report = torque_plugin_tcp::poll(&mut vm);
        most = most.max(report.sockets + report.waiting);
        thread::sleep(Duration::from_millis(2));
    }
    assert!(most <= limit, "{most} connections were held");
    assert_eq!(most, limit, "the cap is reached");
    drop(peers);
}
