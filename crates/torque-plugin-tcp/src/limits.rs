//! [`TcpLimits`]: what one script engine's TCPObjects may hold and move in a tick.

use std::time::Duration;

/// The caps on one script engine's TCPObjects and HTTPObjects (each server and each client has its own). A script that asks
/// for more is refused, never stalled: a connect past a cap prints a warning and reports `onConnectFailed` on the next tick, a
/// listen past one prints an error, a send that would overflow a connection's queue prints a warning and sends nothing, and
/// what a tick may not read stays in the socket for the next tick. The defaults suit a game's handful of web requests and
/// chat bridges; a game sets its own with [`TcpPlugin::with_limits`](crate::TcpPlugin::with_limits).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpLimits {
    /// The script's own connections at once: connecting or open, accepted ones a script took included. Default 32.
    pub sockets: usize,
    /// Accepted connections no script has taken yet (`onConnectRequest` offered them): past it a new one is closed at once,
    /// so peers that connect and wait cannot use up [`sockets`](Self::sockets); none is accepted either once these and the
    /// script's own connections reach `sockets`, and an adoption past `sockets` closes the connection. Default 8.
    pub unadopted: usize,
    /// Listen sockets at once. Default 4.
    pub listeners: usize,
    /// Connects whose name lookup, connect or TLS handshake is still running, a disconnected connect's thread that still
    /// waits on the system included. Default 8.
    pub lookups: usize,
    /// Bytes read from all connections in one tick. Default 256 KiB.
    pub read_per_tick: usize,
    /// Bytes read from one connection in one tick. Default 64 KiB.
    pub read_per_connection: usize,
    /// Bytes written to all connections in one tick (a send past it waits in the connection's queue). Default 256 KiB.
    pub send_per_tick: usize,
    /// Bytes one connection's send queue holds. Default 1 MiB.
    pub send_queue: usize,
    /// The longest line: a peer that sends a longer one is disconnected. Default 256 KiB.
    pub line: usize,
    /// Script callbacks in one tick (`onLine` and the rest). Default 4,096.
    pub callbacks_per_tick: usize,
    /// Connections a listen socket accepts in one tick. Default 8.
    pub accepts_per_tick: usize,
    /// How long an accepted connection waits for a script to take it (`new TCPObject(name, id)`) before it is closed.
    /// Default 10 s.
    pub adopt_within: Duration,
    /// How long a name lookup, connect and TLS handshake may take together. Default 20 s.
    pub connect_within: Duration,
}

impl Default for TcpLimits {
    fn default() -> TcpLimits {
        TcpLimits {
            sockets: 32,
            unadopted: 8,
            listeners: 4,
            lookups: 8,
            read_per_tick: 256 << 10,
            read_per_connection: 64 << 10,
            send_per_tick: 256 << 10,
            send_queue: 1 << 20,
            line: 256 << 10,
            callbacks_per_tick: 4_096,
            accepts_per_tick: 8,
            adopt_within: Duration::from_secs(10),
            connect_within: Duration::from_secs(20),
        }
    }
}

/// The work one tick's pass may do outside the scripts it calls (`Vm::charge_work` units: a byte read or split is one).
pub const PASS_WORK: u64 = 1 << 22;

/// The steps the callbacks of one tick's pass may take together.
pub const PASS_STEPS: u64 = 4_000_000;

/// The connect threads alive at once in the process, for every script engine together: a lookup the system holds keeps its
/// thread until the system answers, so a script that connects and disconnects in a loop cannot pile threads up.
pub const MAX_CONNECT_THREADS: usize = 64;
