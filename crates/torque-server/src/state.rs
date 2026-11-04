//! The server's engine state ([`ServerState`]): what the engine natives and the network host share (the connections'
//! settings, the datablocks' keys and computed values, the ghost hub, the process list, the time scale, the ghost limit) and
//! one slot for a game's own state. The network host's parts are a game's types until the host itself is one of the
//! engine's ([`NetParts`]).

use std::fmt::Debug;
use std::rc::Rc;

use torque_game::dbvalues::DbValues;
use torque_game::sim::ProcessHandle;
use torque_net::profile::DEFAULT_GHOST_LIMIT;

/// The network host's parts the server state holds: the command sender, the datablock keys and the ghost hub, each as the
/// host that runs the server makes them.
pub trait NetParts: 'static {
    /// What the command natives send through (the default: no network host).
    type Commands: Default + Debug;
    /// The datablocks' `modifiedKey`s (a fresh table: the counter at 0).
    type DatablockKeys: Default + Debug;
    /// The ghost hub (the state holds one only while a network host is attached).
    type Ghosts: Clone + Debug;
    /// The ghost-id width of a server before a host sets its ghost limit.
    const START_GHOST_BITS: u8 = START_GHOST_BITS;
}

/// The ghost-id width of a server before a host sets its ghost limit: TGE's `GhostIdBitSize`, the width of a table of
/// [`DEFAULT_GHOST_LIMIT`] objects.
pub const START_GHOST_BITS: u8 = DEFAULT_GHOST_LIMIT.trailing_zeros() as u8;

/// The parts of a server with no network host.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoNet;

impl NetParts for NoNet {
    type Commands = ();
    type DatablockKeys = ();
    type Ghosts = ();
}

/// The engine's state the natives share with the host (connections, datablocks, ghosts, the process list, the time scale), and
/// one slot for the game's ([`ServerState::game`]).
#[derive(Debug)]
pub struct ServerState<N: NetParts = NoNet> {
    /// `allowConnections`; true at start (`NetInterface`'s constructor).
    pub allow_connections: bool,
    /// The ghost limit a host set last.
    pub ghost_limit: Option<i32>,
    /// A host's least ghost limit (none by default): the ghost limit a host sets is at least this. A host may set the floor
    /// before its scripts run.
    pub ghost_limit_floor: Option<i32>,
    /// The ghost table's size, `1 << ghost_bits`.
    pub ghost_size: u32,
    /// The ghost-id width the accept advertises.
    pub ghost_bits: u8,
    /// The port number `setNetPort` used last (bound or not).
    pub net_port: Option<u16>,
    /// A failed bind does not run the bind-failure callback the host's scripts define (false by default): a server that serves
    /// clients on the socket its start-up bound keeps it, where that callback might move it to another port.
    pub keep_first_socket: bool,
    /// The network host the command natives send through (the host's attach sets it; none without a host).
    pub commands: N::Commands,
    /// The datablocks' `modifiedKey`s (`deleteDataBlocks` resets the counter; the network host's attach shares its own
    /// table here).
    pub datablock_keys: N::DatablockKeys,
    /// The values the datablocks compute at add and preload time (the shape loader's table; the network host shares it
    /// with its packers).
    pub db_values: DbValues,
    /// The ghost hub: the registry of ghostable objects, the dirty list and the connections' ghost managers. `None`
    /// without a network host, so that every hook is a `None` test.
    pub ghost: Option<N::Ghosts>,
    /// The `$timeScale` value (1.0 until set).
    pub time_scale: f32,
    /// The server's process list: `GameBase` objects enter it when they are added.
    pub process: ProcessHandle,
    /// The game's state, one value of the game's own type (`()` until the game puts its own there): the engine never reads it.
    pub game: Rc<dyn std::any::Any>,
}

impl<N: NetParts> Default for ServerState<N> {
    fn default() -> ServerState<N> {
        ServerState {
            allow_connections: true,
            ghost_limit: None,
            ghost_limit_floor: None,
            ghost_size: 1 << N::START_GHOST_BITS,
            ghost_bits: N::START_GHOST_BITS,
            net_port: None,
            keep_first_socket: false,
            commands: N::Commands::default(),
            datablock_keys: N::DatablockKeys::default(),
            db_values: DbValues::new(),
            ghost: None,
            time_scale: 1.0,
            process: ProcessHandle::new(),
            game: Rc::new(()),
        }
    }
}

impl<N: NetParts> ServerState<N> {
    /// The game's state, if it is a `T`.
    pub fn game<T: 'static>(&self) -> Option<Rc<T>> {
        Rc::clone(&self.game).downcast::<T>().ok()
    }
}
