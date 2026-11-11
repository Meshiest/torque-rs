//! What the loopback tests share: a script engine with the plugin, a pump that ticks it until a condition holds, and peers on
//! 127.0.0.1 at ports the system picks. No test reaches past the loopback.
//! tests/app (`torque-test-app`) also compiles this file through `#[path]`: a new dev-dependency here must be added there too.

#![allow(dead_code, reason = "each test file uses its own part of the helpers")]

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use torque_engine::{EngineBuilder, ServerStage};
use torque_plugin_tcp::{PassReport, Resolver, TcpLimits, TcpPlugin, poll};
use torque_ts_vm::vm::Vm;

/// How long a test waits for the loopback before it fails.
pub const WAIT: Duration = Duration::from_secs(10);

/// A server script engine made by an engine of `plugin` alone, binding listen sockets to the loopback.
pub fn vm_with(plugin: TcpPlugin) -> Vm {
    let engine = EngineBuilder::new()
        .add_plugins(plugin)
        .build()
        .expect("the engine builds");
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).expect("the recipes run");
    }
    vm.set_global_text(b"$Pref::Net::BindAddress", b"127.0.0.1");
    vm
}

/// [`vm_with`] the default plugin, names looked up by [`test_resolver`].
pub fn vm() -> Vm {
    vm_with(TcpPlugin::default().with_resolver(test_resolver()))
}

/// [`vm_with`] these caps, names looked up by [`test_resolver`].
pub fn vm_limited(limits: TcpLimits) -> Vm {
    vm_with(
        TcpPlugin::default()
            .with_resolver(test_resolver())
            .with_limits(limits),
    )
}

/// Names that end in `.test` are 127.0.0.1 and `localhost` is too; anything else is not found. No system lookup.
pub fn test_resolver() -> Resolver {
    Arc::new(|name, port| {
        if name.ends_with(".test") || name == "localhost" {
            Ok(vec![SocketAddr::from(([127, 0, 0, 1], port))])
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "no such name"))
        }
    })
}

/// Ticks `vm` (a pass, then a millisecond's sleep) until `done` holds; false when [`WAIT`] passed first. Every pass's report
/// goes to `each`.
pub fn pump_with(
    vm: &mut Vm,
    mut done: impl FnMut(&Vm) -> bool,
    mut each: impl FnMut(&PassReport),
) -> bool {
    let start = Instant::now();
    while start.elapsed() < WAIT {
        let report = poll(vm);
        assert!(
            report.errors.is_empty(),
            "callback errors: {:?}",
            report.errors
        );
        each(&report);
        if done(vm) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    false
}

/// [`pump_with`] with no look at the reports.
pub fn pump(vm: &mut Vm, done: impl FnMut(&Vm) -> bool) -> bool {
    pump_with(vm, done, |_| {})
}

/// Ticks `vm` `n` times, a millisecond apart.
pub fn ticks(vm: &mut Vm, n: usize) {
    for _ in 0..n {
        poll(vm);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// A listener on 127.0.0.1 at a port the system picks.
pub fn listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("a loopback port");
    let port = listener.local_addr().expect("its address").port();
    (listener, port)
}

/// A port on 127.0.0.1 nothing listens on (picked by the system, then let go).
pub fn closed_port() -> u16 {
    listener().1
}

/// Reads `stream` up to and with the first `\r\n\r\n`, then the `Content-Length` bytes that follow: an HTTP request.
pub fn read_request(stream: &mut TcpStream) -> String {
    let mut got = Vec::new();
    let mut byte = [0u8; 1];
    while !got.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).expect("the request") == 0 {
            break;
        }
        got.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&got).into_owned();
    let length = head
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length: "))
        .and_then(|n| n.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).expect("the body");
    head + &String::from_utf8_lossy(&body)
}

/// Writes `data` and closes the connection.
pub fn answer(mut stream: TcpStream, data: &[u8]) {
    stream.write_all(data).expect("the answer");
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

/// Callbacks that log what they were told into `$log`, for the object `name`.
pub fn logging(name: &str) -> String {
    format!(
        r#"
        function {name}::onDNSResolved(%this) {{ $log = $log @ "dns;"; }}
        function {name}::onDNSFailed(%this) {{ $log = $log @ "dnsfailed;"; $done = 1; }}
        function {name}::onConnected(%this) {{ $log = $log @ "connected;"; }}
        function {name}::onConnectFailed(%this) {{ $log = $log @ "failed;"; $done = 1; }}
        function {name}::onLine(%this, %line) {{ $log = $log @ "line:" @ %line @ ";"; }}
        function {name}::onDisconnect(%this) {{ $log = $log @ "disconnect;"; $done = 1; }}
        "#
    )
}
