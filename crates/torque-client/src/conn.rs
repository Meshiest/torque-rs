//! The connection to a server: TGE's client `GameConnection` as scripts drive it (`new GameConnection(ServerConnection)`,
//! `setConnectArgs`, `setJoinPassword`, `connect`), over the game's feed ([`Feed`], made by the game's half [`ConnGame`]) on the
//! client's own socket ([`crate::net::ClientNet`]).
//!
//! **What TGE does**: `NetConnection::connect` parses the address with `Net::stringToAddress` (port 28000 by default; an address
//! it cannot parse prints `NetConnection::connect: invalid address - %s`), and sends the challenge request (`Sending Connect
//! challenge Request`); the time-outs send it again after a wait and give up after the game's tries (`onConnectRequestTimedOut`,
//! then the object is deleted). The challenge response prints `Got Connect challenge Response` and `Sending Connect Request`; the
//! accept (`onConnectionEstablished`, client side) prints `Connection established` and calls `%conn.onConnectionAccepted()`; a
//! reject calls `onConnectRequestRejected(reason)` and the object is deleted; a disconnect from the server prints `Connection
//! with server lost.` and calls `onConnectionDropped(reason)`; a silent server `onConnectionTimedOut`. Then, per event of the
//! server's packets, in their order: a net string is mapped into the client's tag table (`Mapping string: %s to index: %d`); a
//! remote command calls `clientCmd<name>` with its tags expanded (`net::remote::resolve`); a datablock
//! `onDataBlockObjectReceived(index, total)`; connection message 3 `onGhostAlwaysStarted(count)`; a ghost-always object
//! `onGhostAlwaysObjectReceived()`; the first control object `%conn.initialControlSet()`; the game's events through the game's
//! half ([`ConnGame::event_steps`]). `connectLocal` returns "" or the refusal's text (`Unknown Error` when there is none).
//!
//! A game changes the handshake's lines and callbacks ([`ConnGame::challenge_sent`], [`ConnGame::challenge_answered`],
//! [`ConnGame::invalid_address`]), when a mapped string is printed ([`ConnGame::print_mapping`]), the view distance's variable
//! ([`ConnGame::view_distance_pref`]), the camera's speed ([`ConnGame::camera_speed`]) and `connectLocal`'s answer
//! ([`ConnGame::local_error`]).
//!
//! **Scripts call** `commandToServer(tag, ...)` (only once the connection to the server is established; the tag test and
//! `Remote Command Error - command must be a tag.`; `net::remote::send_command` over the connection's own string table), and the
//! connection's methods: `isConnected` (accepted), `isLocalConnection` (0), `getAddress`, `getPort`, `getPing`, `delete` (a disconnect
//! with the reason, empty by default, when connected), `getGhostsActive`, `getControlObject` and `resolveGhostID` (the ghosts'
//! script objects, [`crate::ghost_objects`]). Every other `GameConnection` (one a server host made) keeps the server's natives.
//! A game registers its own natives of the connection after these ([`ConnGame::register_natives`]).
//!
//! **The ticks.** Every tick of the client host once accepted: the move of [`crate::net::moves::next_move`] (TGE's `getNextMove`)
//! is queued, and the packet goes out with the unacknowledged moves ([`net::moves::MoveQueue`]) and the control checksum of the
//! client's prediction ([`crate::predict`]).
//!
//! **The game's half** ([`ConnGame`], the type the client state is made over; Torque's is [`tge::TgeConn`]): the feed and its world
//! ([`ConnGame::start`]: the packets, the events and the records), the file gate of `connect` ([`ConnGame::file_permitted`]: a
//! refused `connect` does nothing at all), the local connection's server half ([`ConnGame::local_request`]), the part of each
//! connection the game keeps ([`ConnPart`], dropped with the connection), the game's events and its work each tick, and what is
//! read of its world (the Sky's distances, the Sun's colour). The game's own natives of the connection are registered by the
//! game. The renderer is told of a new connection through the hooks the game installs ([`crate::render::RenderResets`]).
//!
//! **LAN, and the servers the user chose**: a connect to an address the client may not send to sends nothing
//! (`connect: refused <address>: LAN servers only`), counts it ([`ConnSlot::refused`]) and times out as an unanswered connect does. Under
//! [`crate::net::NetPolicy::Internet`] a public address may be connected to once when the user chose it: a game's native armed
//! one permit for it while the user's click or key on a GUI control was handled ([`crate::net::Internet::arm`], with
//! [`ClientState::user_input`] set), used up by the connect it allows and gone after [`crate::net::PERMIT_MS`]
//! ([`crate::net::Internet::take_permit`]). Torque's arms none, so a public connect without a permit is refused. The public
//! server's door lives as long as the connection (`sync_door`).
//!
//! Credit: follows TGE's client connection as Torque3D carries it (MIT: `sim/netConnection.cpp`, `sim/netInterface.h`, each
//! Copyright (c) 2012 GarageGames, LLC; `T3D/gameBase/gameConnection.cpp`, Copyright (c) 2012 GarageGames, LLC; Copyright (C)
//! 2015 Faust Logic, Inc.).

use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gui::Gui;
use net::control::{ClientBlock, ControlData, ControlSection};
use net::events::{EventQueue, NetEvent};
use net::moves::MoveQueue;
use net::netstring::{RemoteStrings, SendStrings, mapping_line};
use net::remote::{RemoteCommandEvent, SendError, TagLookup, function_name, resolve, send_command};
use torque_game::dbvalues::DbValues;
use ts_vm::host::{FileHost, FileRead};
use ts_vm::sim::ObjId;
use ts_vm::text::until_nul;
use ts_vm::vm::{Native, NativeCall, Vm, VmError};

use crate::game::{GhostWidths, client_game};
use crate::hud::HudWorld;
use crate::net::ClientNet;
use crate::predict::game::PredictWorld;
use crate::predict::{FileReader, Predictor};
use crate::render::RenderResets;
use crate::state::{ClientState, GameSlot};

pub mod tge;

/// The client's tick: one move and one packet each (TGE's process list ticks every 32 ms; `$pref::Net::PacketRateToServer` is 32).
pub const TICK_MS: u32 = 32;
/// TGE's camera speed between first and third person (`GameConnection::mCameraSpeed`), Torque's [`ConnGame::camera_speed`].
pub const TGE_CAMERA_SPEED: f32 = 10.0;
/// The longest connect argument and password kept.
pub const MAX_ARG: usize = 255;
/// The most events one tick hands to the scripts: the rest wait in the feed, in order, for the next tick (they were dropped,
/// a `Dropped`, `Rejected` or datablock event among them; a feed stops taking datagrams while its own most events wait, so the
/// backlog never grows past a tick's share and a datagram's).
pub const MAX_EVENTS_PER_TICK: usize = 16_384;

/// What `setConnectArgs` and `setJoinPassword` left on a connection object before its `connect`.
#[derive(Debug, Clone, Default)]
pub struct ConnArgs {
    pub args: Vec<Vec<u8>>,
    pub password: Vec<u8>,
}

/// The world of a game's feed.
pub type WorldOf<G> = <<G as ConnGame>::Feed as Feed>::World;

/// The connection to a server.
pub struct ClientConn<G: ConnGame> {
    pub obj: ObjId,
    pub server: SocketAddrV4,
    pub feed: G::Feed,
    pub world: WorldOf<G>,
    pub inbox: Rc<RefCell<VecDeque<Vec<u8>>>>,
    pub accepted: bool,
    /// The server's net strings, as the client's tag ids (`NetStringEvent::process`).
    pub remote: RemoteStrings,
    /// The client's own (`commandToServer`'s tags).
    pub send: SendStrings,
    pub moves: MoveQueue,
    pub triggers: crate::net::moves::TriggerMemory,
    pub last_tick: Option<u32>,
    pub finished_initial_ghost: bool,
    /// The client's prediction of its control object ([`crate::predict`]).
    pub predict: Predictor<WorldOf<G>>,
    /// Packets sent, and of them the ones whose checksum was the prediction's (not 0).
    pub packets: u64,
    /// A local connection (`connectLocal`): the server half's `LocalClientConnection` object in this VM; the packets go through
    /// memory (the host's transport over [`crate::net::ClientNet`]), `server` is [`LOCAL_ADDR`].
    pub local: Option<ObjId>,
    /// The control object's fov and its update flag ([`crate::zoom`]).
    pub fov: crate::zoom::ConnFov,
    /// The ghosts' script objects ([`crate::ghost_objects`]).
    pub mirrors: crate::net::ghost_objects::Mirrors,
    /// The game's part of the connection ([`ConnPart`], [`ClientConn::part`]).
    pub game: Box<dyn ConnPart>,
    /// The renderer's hooks, told of the connection's start ([`crate::game::ClientGame::renderer`]).
    pub render: Rc<dyn RenderResets>,
}

impl<G: ConnGame> ClientConn<G> {
    /// The game's part, when it is a `T`.
    pub fn part<T: 'static>(&self) -> Option<&T> {
        self.game.as_any().downcast_ref()
    }

    /// The game's part to change, when it is a `T`.
    pub fn part_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.game.as_any_mut().downcast_mut()
    }

    /// Whether this is a local connection whose server half still exists (TGE's remote-connection pointer).
    pub fn local_live(&self, vm: &Vm) -> bool {
        self.local.is_some_and(|o| vm.objects().contains(o))
    }
}

/// A connect that will never be answered (a refused address): the object times out as an unanswered connect does.
#[derive(Debug, Clone, Copy)]
struct Doomed {
    obj: ObjId,
    due: u32,
}

/// The client's connection, when it has one, and what the connection objects were given.
pub struct ConnSlot<G: ConnGame> {
    pub conn: Option<Box<ClientConn<G>>>,
    pub pending: HashMap<ObjId, ConnArgs>,
    doomed: Vec<Doomed>,
    /// Connects refused: the address is not a LAN one (nothing sent).
    pub refused: u64,
    /// Connects started.
    pub connects: u64,
    /// The connection's record of its datagrams goes here when set (the host's record of the session).
    pub relay: Option<PathBuf>,
}

impl<G: ConnGame> Default for ConnSlot<G> {
    fn default() -> ConnSlot<G> {
        ConnSlot {
            conn: None,
            pending: HashMap::new(),
            doomed: Vec::new(),
            refused: 0,
            connects: 0,
            relay: None,
        }
    }
}

/// The most connection objects remembered before their `connect`.
const MAX_PENDING: usize = 64;

// ---- the install as the connection reads it (the shapes, the game's cache check and wheel counts)

thread_local! {
    /// The client host's files (the install and its overlay) the connection's reads see: the game sets them as it connects
    /// ([`set_install`]).
    static FILES: RefCell<Option<Rc<dyn FileHost>>> = const { RefCell::new(None) };
}

/// The public server's door ([`crate::net::Internet::server`]) follows the live connection: open while a network connection to a
/// public address has not ended ([`Feed::is_gone`]), shut once it is dropped, rejected, timed out or replaced.
fn sync_door<G: ConnGame>(st: &ClientState<G>) {
    let live = st
        .conn
        .conn
        .as_ref()
        .filter(|c| c.local.is_none() && !c.feed.is_gone() && crate::net::is_public(*c.server.ip()))
        .map(|c| c.server);
    st.net.borrow_mut().internet.server = live;
}

/// The files the connection's reads see from now on ([`read_install`]): the game's, set as it connects.
pub fn set_install(files: Option<Rc<dyn FileHost>>) {
    FILES.with(|f| *f.borrow_mut() = files);
}

/// A server-given path the client may read: relative, `/`-separated, no `..`, no drive, at most 1,024 bytes.
pub fn path_ok(path: &[u8]) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && path.first() != Some(&b'/')
        && !path.iter().any(|&b| b == b'\\' || b == b':' || b == 0)
        && path
            .split(|&b| b == b'/')
            .all(|part| !part.is_empty() && part != b"." && part != b"..")
}

pub fn read_install(path: &[u8]) -> Option<Vec<u8>> {
    if !path_ok(path) {
        return None;
    }
    FILES.with(|f| match f.borrow().as_ref()?.read(path) {
        FileRead::Found(bytes) => Some(bytes),
        _ => None,
    })
}

/// Whether the game lets a native gated on the calling file run while `file` is executing ([`ConnGame::file_permitted`]).
fn file_gate<G: ConnGame>(root: &Path, file: Option<&[u8]>) -> bool {
    G::file_permitted(root, file)
}

// ---- the game's half

/// The game's part of a connection: its own state for the connection's life, dropped with it (no VM then: what it lets go of
/// goes through hooks such as [`RenderResets`]).
pub trait ConnPart: Any {
    /// The connection became the client's.
    fn installed(&self) {}

    /// The lines it has for the console since the last tick.
    fn take_notes(&self) -> Vec<String> {
        Vec::new()
    }

    fn as_any(&self) -> &dyn Any;

    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// No part.
#[derive(Debug, Default)]
pub struct NoPart;

impl ConnPart for NoPart {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// The local connection's address (`Net::stringToAddress` of none: no address, port 0): the client half's packets go to the server
/// half from it.
pub const LOCAL_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);

/// A connection's link to its server: the datagrams the feed sends, and the ones the server sent it.
pub trait Link {
    /// Sends one datagram to the server.
    fn send(&mut self, bytes: &[u8]) -> io::Result<()>;

    /// The next datagram the server sent, if one waits (`wait`: how long a blocking link may wait for one).
    fn recv(&mut self, wait: Duration) -> io::Result<Option<Vec<u8>>>;
}

/// What a connect gives the game's feed ([`ConnGame::start`]).
pub struct Connect {
    /// The link to the server: the client's socket, or the local server half's inbox.
    pub link: Box<dyn Link>,
    /// `setConnectArgs`'s arguments, five of them (the missing ones empty).
    pub args: Vec<Vec<u8>>,
    /// `setJoinPassword`'s.
    pub password: Vec<u8>,
    /// The connect sequence (0 for a local connection).
    pub seq: u32,
    /// The connection object (a game keeps its own settings of it).
    pub obj: ObjId,
    /// Where the connection's record of its datagrams goes, when the host keeps one.
    pub relay: Option<Box<dyn io::Write>>,
    /// The server half's accept, for a local connection.
    pub local: Option<LocalAccept>,
    /// The renderer's hooks the connection holds.
    pub render: Rc<dyn RenderResets>,
}

/// The local server half's accept of a local connection: its connection object, the protocol version and the ghost index width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalAccept {
    pub obj: ObjId,
    pub protocol: u32,
    pub ghost_bits: u8,
}

/// What the game's server half answered a local connect ([`ConnGame::local_request`]).
pub enum LocalAnswer {
    /// The client cannot connect locally (no server half, its tables unreadable): printed, then the script's `onConnectionError`.
    Failure(String),
    /// The server half refused (`reason`: its text, empty when it gave none); its callbacks ran (`ran`: their error, when one
    /// stopped).
    Refused {
        reason: Vec<u8>,
        ran: Result<(), VmError>,
    },
    Accepted(LocalAccept),
}

/// A new connection's feed and world, the game's part of it and what the prediction reads through the game.
pub struct Started<G: ConnGame> {
    pub feed: G::Feed,
    pub world: WorldOf<G>,
    pub part: Box<dyn ConnPart>,
    /// The files the prediction's static collision reads (`None`: none).
    pub reader: Option<FileReader>,
}

/// One event of a feed, as the connection runs it (the game's own are the game's: [`ConnGame::event_steps`]).
pub enum FeedEvent<E> {
    /// A challenge request went out ([`ConnGame::challenge_sent`]: TGE's `Sending Connect challenge Request`).
    ChallengeSent,
    /// The challenge response came and the connect request went out.
    ChallengeAnswered,
    /// `Connection established`, `onConnectionAccepted`.
    Accepted,
    /// `onConnectRequestRejected(reason)`.
    Rejected(Vec<u8>),
    /// No answer within the tries (`onConnectRequestTimedOut`).
    HandshakeTimedOut,
    /// The server hung up (its reason) or the link failed.
    Dropped(Vec<u8>),
    /// The server went silent.
    TimedOut,
    /// A net string the server mapped (`NetStringEvent::process`).
    NetString { slot: u8, text: Vec<u8> },
    /// A remote command (its arguments as the wire carried them: the connection expands the tags).
    Remote(RemoteCommandEvent),
    /// `onDataBlockObjectReceived(index, total)`, then the datablock the event carried (the game's unpack reads it:
    /// [`crate::game::ClientGame::unpack_datablock`]).
    DataBlock {
        index: u32,
        total: u32,
        body: Box<dyn Any>,
    },
    /// `onGhostAlwaysStarted(count)`.
    GhostAlwaysStarted(u32),
    /// `onGhostAlwaysObjectReceived()`.
    GhostAlwaysObject,
    /// The first head with a control section: `initialControlSet`.
    ControlSet,
    /// A server head: its move ack and control section, for the client's prediction.
    Head {
        move_ack: u32,
        control: ControlSection<Box<dyn ControlData>>,
    },
    /// A console line.
    Line(Vec<u8>),
    /// An event of the game's own.
    Game(E),
}

/// A game's feed: what the connection reads from its server into its world, and what it sends back.
pub trait Feed: 'static {
    /// The world the feed fills (the record world and the game's view of it).
    type World: PredictWorld + HudWorld;
    /// An event of the game's own.
    type Event;

    /// The start of a tick: the host's clock (`now`, the client's simulation time) and what the feed reads of the scripts' globals.
    fn tick_start(&mut self, vm: &Vm, now: u32);

    /// Reads the datagrams that came into the world; an error is a section that does not decode (the connection ends).
    fn pump(&mut self, world: &mut Self::World) -> Result<(), String>;

    /// The host's own work for the connection, for the join's load report.
    fn note_host_work(&mut self, work: Duration) {
        let _ = work;
    }

    /// What did not decode, when a pump failed.
    fn decode_summary(&self) -> Option<String> {
        None
    }

    /// Hangs up with `reason` (nothing more is sent or read).
    fn disconnect(&mut self, reason: &str);

    /// The first `max` events in order (the rest wait for the next call).
    fn take_events(&mut self, max: usize) -> Vec<FeedEvent<Self::Event>>;

    /// The load report's lines of the join's phases that ended since the last call.
    fn take_load_lines(&mut self) -> Vec<String> {
        Vec::new()
    }

    /// The connection ended (dropped, rejected, timed out or hung up).
    fn is_gone(&self) -> bool;

    /// The width of a ghost index on this connection.
    fn ghost_bits(&self) -> u8;

    /// The protocol version of the server's accept; 0 before it.
    fn protocol_version(&self) -> u32;

    /// The room the ordered event queue has.
    fn room_ordered(&self) -> usize;

    /// Posts ordered net events (a remote command and its net strings).
    fn post_net(&mut self, events: Vec<NetEvent>);

    /// Sends the tick's packet: the client's control block, the events that wait.
    fn send_tick(&mut self, world: &mut Self::World, block: &ClientBlock);
}

/// The game's half of the client's connection: its feed and what the connection asks the game. The client state is made over it
/// ([`ClientState`]); Torque's is [`tge::TgeConn`].
pub trait ConnGame: Default + 'static {
    /// The game's feed.
    type Feed: Feed;
    /// The game's own parts of the natives' state ([`ClientState::parts`]).
    type Parts: Default + 'static;

    /// How long an unanswered challenge waits before the connect tries again, and the tries.
    const HANDSHAKE_WAIT_MS: u32;
    const HANDSHAKE_TRIES: u32;

    /// Whether a native gated on the calling file (`connect`, the internet permit, the game's own) may run while `file` is
    /// executing (`None`: no script is), for the install at `root`.
    fn file_permitted(root: &Path, file: Option<&[u8]>) -> bool {
        let _ = (root, file);
        true
    }

    /// What the game needs before a network connect starts (the tables it reads the server with); an error is printed and the
    /// connect does nothing more.
    fn ready(vm: &mut Vm, client: &Rc<RefCell<ClientState<Self>>>) -> Result<(), String> {
        let _ = (vm, client);
        Ok(())
    }

    /// Whether the join's phases are printed as they end (the game's performance report is on).
    fn report_load(vm: &Vm) -> bool {
        let _ = vm;
        false
    }

    /// Why a connect to a public server the user did not choose is refused (the end of its `connect: refused <address>: ...`
    /// line): where the user chooses a server, in the game's own words.
    fn refused_internet_reason() -> &'static str {
        "an internet server is joined only when the user chose it"
    }

    /// The names the connection's local test answers to (Torque's `isLocalConnection`; a game may add its own names).
    fn local_test_names() -> &'static [&'static str] {
        &["isLocalConnection"]
    }

    /// The game's own natives of the connection, registered after the engine's (Torque's registers none).
    fn register_natives(vm: &mut Vm, client: &Rc<RefCell<ClientState<Self>>>, gui: &Gui) {
        let _ = (vm, client, gui);
    }

    /// The lines and callbacks as `connect` sends the challenge request (Torque's: TGE's `Sending Connect challenge Request`).
    fn challenge_sent() -> Vec<Step<Self>> {
        vec![Step::Print(b"Sending Connect challenge Request".to_vec())]
    }

    /// The lines and callbacks as `server` (the connection's, when there is one) answers the challenge and the connect request
    /// goes out (Torque's: TGE's `Got Connect challenge Response` and `Sending Connect Request`).
    fn challenge_answered(server: Option<SocketAddrV4>) -> Vec<Step<Self>> {
        let _ = server;
        vec![
            Step::Print(b"Got Connect challenge Response".to_vec()),
            Step::Print(b"Sending Connect Request".to_vec()),
        ]
    }

    /// What `connect` does after printing `NetConnection::connect: invalid address - %s` (Torque's: nothing more).
    fn invalid_address(vm: &mut Vm) -> Result<(), VmError> {
        let _ = vm;
        Ok(())
    }

    /// Whether a net string mapped into the tag table prints `Mapping string: %s to index: %d` (Torque's: always, as TGE's
    /// `NetStringEvent::process`).
    fn print_mapping(vm: &Vm) -> bool {
        let _ = vm;
        true
    }

    /// The script variable's cap on the view distance, as read this frame (Torque's: none, the Sky's distances only).
    fn view_distance_pref(vm: &Vm) -> Option<f32> {
        let _ = vm;
        None
    }

    /// How fast the camera moves between first and third person, in its distance a second (Torque's: TGE's
    /// `GameConnection::mCameraSpeed`, 10).
    fn camera_speed(vm: &Vm) -> f32 {
        let _ = vm;
        TGE_CAMERA_SPEED
    }

    /// What `connectLocal` returns when the server half refused (`reason`: the refusal's text, empty when it gave none), or
    /// the engine could not connect. Torque's: TGE's, the reason, else `Unknown Error`.
    fn local_error(reason: &[u8]) -> Vec<u8> {
        if reason.is_empty() {
            UNKNOWN_ERROR.to_vec()
        } else {
            reason.to_vec()
        }
    }

    /// A new connection's feed and world (`connect.local`: a local connection's, its server half accepted).
    fn start(
        vm: &mut Vm,
        client: &Rc<RefCell<ClientState<Self>>>,
        connect: Connect,
    ) -> Result<Started<Self>, String>;

    /// A local connection's request to the game's server half in this VM (`obj`: the client half's object).
    fn local_request(
        vm: &mut Vm,
        client: &Rc<RefCell<ClientState<Self>>>,
        obj: ObjId,
        args: &ConnArgs,
    ) -> LocalAnswer {
        let _ = (vm, client, obj, args);
        LocalAnswer::Failure("the client has no server half".to_string())
    }

    /// The server half's own `onConnectionEstablished` (after the client half's), for its connection object `server`.
    fn local_establish(
        vm: &mut Vm,
        client: &Rc<RefCell<ClientState<Self>>>,
        server: ObjId,
    ) -> Result<(), VmError> {
        let _ = (vm, client, server);
        Ok(())
    }

    /// A new connection is about to become the client's.
    fn installing(st: &mut ClientState<Self>) {
        let _ = st;
    }

    /// The connection's world goes (a new one replaces it, or it ended): what the client keeps of it is let go.
    fn world_gone(st: &mut ClientState<Self>) {
        let _ = st;
    }

    /// The game's work at the start of a tick's events.
    fn tick(vm: &mut Vm, client: &Rc<RefCell<ClientState<Self>>>) {
        let _ = (vm, client);
    }

    /// The game's work at the end of a tick (the world's sounds).
    fn ticked(vm: &mut Vm, client: &Rc<RefCell<ClientState<Self>>>) {
        let _ = (vm, client);
    }

    /// The steps of an event of the game's own (none: it is dropped).
    fn event_steps(
        vm: &mut Vm,
        client: &Rc<RefCell<ClientState<Self>>>,
        ev: <Self::Feed as Feed>::Event,
    ) -> Vec<Step<Self>> {
        let _ = (vm, client, ev);
        Vec::new()
    }

    /// The datablocks' computed values a datablock the client makes reads (none: it reads none).
    fn db_values(st: &ClientState<Self>) -> Option<DbValues> {
        let _ = st;
        None
    }

    /// The world before the tick's moves (`finished_initial_ghost`: the scripts said the initial ghosting is done).
    fn moving(world: &mut WorldOf<Self>, finished_initial_ghost: bool) {
        let _ = (world, finished_initial_ghost);
    }

    /// The world before the frame draws it, beside the engine's frame state.
    fn frame(game: &GameSlot, world: &mut WorldOf<Self>) {
        let _ = (game, world);
    }

    /// The Sky's distance fields as bits (a script's cap, [`ClientState::max_view_distance`], holds until they change); none
    /// without a Sky.
    fn sky_distance(world: &WorldOf<Self>) -> Option<[u32; 2]> {
        let _ = world;
        None
    }

    /// The client's Sun's colour (for the game's natives); none without a Sun.
    fn sun_color(world: &WorldOf<Self>) -> Option<[f32; 3]> {
        let _ = world;
        None
    }
}

// ---- the link: the client's socket, the connection's own datagrams

struct InboxLink {
    net: Rc<RefCell<ClientNet>>,
    server: SocketAddrV4,
    inbox: Rc<RefCell<VecDeque<Vec<u8>>>>,
}

impl Link for InboxLink {
    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        // A send the socket refused (none, or not a LAN address) is counted by the socket; the packet layer resends.
        self.net.borrow_mut().send(self.server, bytes);
        Ok(())
    }

    fn recv(&mut self, _wait: Duration) -> io::Result<Option<Vec<u8>>> {
        Ok(self.inbox.borrow_mut().pop_front())
    }
}

/// A datagram for the connection: kept when it comes from the connection's server (a local connection takes none), else given back.
pub fn take_datagram<G: ConnGame>(
    client: &Rc<RefCell<ClientState<G>>>,
    from: SocketAddrV4,
    data: Vec<u8>,
) -> Result<(), Vec<u8>> {
    let st = client.borrow();
    let Some(conn) = st.conn.conn.as_ref() else {
        return Err(data);
    };
    if conn.local.is_some() || conn.server != from {
        return Err(data);
    }
    let mut inbox = conn.inbox.borrow_mut();
    if inbox.len() < crate::net::MAX_INBOX {
        inbox.push_back(data);
    }
    Ok(())
}

// ---- the tags

struct VmTags<'a>(&'a Vm);

impl TagLookup for VmTags<'_> {
    fn tag_text(&self, id: u32) -> Option<Vec<u8>> {
        self.0.tags().lookup(id).map(<[u8]>::to_vec)
    }

    fn table_size(&self) -> u32 {
        self.0.tags().size()
    }
}

fn release(vm: &mut Vm, id: u32) {
    if let Some(message) = vm.tags_mut().remove_string(id, false) {
        vm.error(message);
    }
}

// ---- running the connection

/// The game's work a step runs on the VM and the client.
pub type GameRun<G> = Box<dyn FnOnce(&mut Vm, &Rc<RefCell<ClientState<G>>>)>;

/// What a step of the connection makes the scripts do once no borrow is held.
pub enum Step<G: ConnGame> {
    Print(Vec<u8>),
    Call(Vec<u8>, Vec<Vec<u8>>),
    Method(&'static str, Vec<Vec<u8>>),
    /// The connection is over: the object is deleted after the callbacks before it.
    Delete,
    /// The game's work on the VM and the client (an event of its own, [`ConnGame::event_steps`]).
    Run(GameRun<G>),
    /// A global set without a script call (engine-only).
    Global(&'static str, Vec<u8>),
    /// A datablock made in the client's Vm ([`crate::datablocks`]) from the event the game's feed read.
    Datablock(Box<dyn Any>),
}

/// One tick of the connection (the host's, before its Sim tick): the datagrams it got, its events as script calls, its time-outs,
/// then the move and the packet.
pub fn tick<G: ConnGame>(vm: &mut Vm, client: &Rc<RefCell<ClientState<G>>>) {
    let now = vm.sim_time();
    run_doomed(vm, client, now);
    sync_door(&client.borrow());
    let obj = match client.borrow().conn.conn.as_ref() {
        Some(c) => c.obj,
        None => return,
    };
    // A connection whose object a script deleted is over: hang up (TGE's `onRemove` sends the disconnect).
    if !vm.objects().contains(obj) {
        drop_conn(client, Some(b""));
        return;
    }
    let (mut events, lines, failed) = {
        let mut st = client.borrow_mut();
        let st = &mut *st;
        let Some(conn) = st.conn.conn.as_mut() else {
            return;
        };
        let c = &mut **conn;
        // The world's clock is the client's simulation time, the frame clock's (`anim_ms`) too.
        c.feed.tick_start(vm, now);
        // The collision shapes' share of this tick (`predict::SHAPE_TICK_WORK`) starts again.
        c.predict.static_world.start_tick();
        let mut failed = None;
        let t_decode = std::time::Instant::now();
        let pumped = c.feed.pump(&mut c.world);
        let decode = t_decode.elapsed();
        st.live_perf.decode += decode;
        c.feed.note_host_work(decode);
        if let Err(e) = pumped {
            // A section that does not decode: the client cannot follow the server any more. A record the engine cannot read
            // has no length to skip it by, so the connection ends, loudly (never a silent wait).
            let what = c
                .feed
                .decode_summary()
                .unwrap_or_else(|| "a packet".to_string());
            c.feed.disconnect(&format!("decode: {e}"));
            failed = Some((
                format!(
                    "ERROR: the engine cannot read {what} from this server ({e}); disconnecting"
                ),
                format!("The game could not read this server's data ({what})."),
            ));
        }
        let mut lines = c.game.take_notes();
        // The static objects the client collides with otherwise than their datablocks declare (a prediction the server corrects).
        lines.extend(c.predict.static_world.take_notes());
        (c.feed.take_events(MAX_EVENTS_PER_TICK), lines, failed)
    };
    for line in lines {
        vm.print(line.as_bytes());
    }
    if let Some((line, reason)) = failed {
        vm.error(line);
        events.push(FeedEvent::Dropped(reason.into_bytes()));
    }
    // The game's work before this tick's events.
    let t_scripts = std::time::Instant::now();
    G::tick(vm, client);
    run_events(vm, client, obj, events);
    let t_sync = std::time::Instant::now();
    if vm.objects().contains(obj) {
        crate::net::ghost_objects::sync(vm, client, obj);
    }
    let t_predict = std::time::Instant::now();
    send_tick(vm, client, now);
    let load = {
        let mut st = client.borrow_mut();
        let p = &mut st.live_perf;
        p.net_scripts += t_sync.duration_since(t_scripts);
        p.ghost_sync += t_predict.duration_since(t_sync);
        p.predict += t_predict.elapsed();
        st.conn.conn.as_mut().map_or_else(Vec::new, |c| {
            c.feed.note_host_work(t_sync.duration_since(t_scripts));
            c.feed.take_load_lines()
        })
    };
    // The join's phases as they end, one line a phase, while the game's performance report is on.
    if G::report_load(vm) {
        for line in load {
            vm.print(line.as_bytes());
        }
    }
    // `$pref::Player::renderMyItems` (an engine variable): read by Player::renderMountedImage every frame.
    let render_my_items = vm
        .global("$pref::Player::renderMyItems")
        .is_none_or(|v| ts_vm::fields::atob(&v));
    // The view distance's cap on the scene's: the game's variable ([`ConnGame::view_distance_pref`], read as the Sky updates),
    // or a script's cap ([`ClientState::max_view_distance`]) until the Sky's distances change (the next Sky update sets the
    // scene's distances again).
    let pref_max = G::view_distance_pref(vm);
    let mut st = client.borrow_mut();
    let called = st.max_view_distance;
    if let Some(c) = st.conn.conn.as_mut() {
        let c = &mut **c;
        let sky = G::sky_distance(&c.world);
        let (cap, kept) = view_cap(pref_max, called, sky);
        c.world.max_view_distance = cap;
        if !kept {
            st.max_view_distance = None;
        }
    }
    {
        let st = &mut *st;
        if let Some(c) = st.conn.conn.as_mut() {
            let c = &mut **c;
            // The frame's clock first: the eye is posed by it.
            c.world.animate = true;
            c.world.anim_ms = vm.sim_time();
            c.predict.update_view(&mut c.world);
            c.world.hide_own_items = !render_my_items;
            G::frame(&st.game, &mut c.world);
        }
    }
    // The client state is let go first: the game's work at the tick's end borrows it itself.
    drop(st);
    sync_door(&client.borrow());
    // The game's work at the tick's end (the world's sounds and the listener at the eye).
    let t_sounds = std::time::Instant::now();
    G::ticked(vm, client);
    client.borrow_mut().live_perf.sounds += t_sounds.elapsed();
}

/// Whether the connection gives GameTSCtrl a camera (`processCameraQuery`: a control object with an eye).
pub fn has_camera<G: ConnGame>(client: &Rc<RefCell<ClientState<G>>>) -> bool {
    client
        .borrow()
        .conn
        .conn
        .as_ref()
        .is_some_and(|c| c.accepted && c.world.eye().is_some())
}

fn run_doomed<G: ConnGame>(vm: &mut Vm, client: &Rc<RefCell<ClientState<G>>>, now: u32) {
    let due: Vec<ObjId> = {
        let mut st = client.borrow_mut();
        let (due, keep): (Vec<Doomed>, Vec<Doomed>) =
            st.conn.doomed.drain(..).partition(|d| now >= d.due);
        st.conn.doomed = keep;
        due.into_iter().map(|d| d.obj).collect()
    };
    for obj in due {
        if !vm.objects().contains(obj) {
            continue;
        }
        if let Err(e) = vm.call_method(obj, b"onConnectRequestTimedOut", &[]) {
            crate::net::keep_error(client, e);
        }
        if vm.objects().contains(obj) {
            let _ = vm.delete_object(obj);
        }
    }
}

/// Makes `conn` the client's connection (a new one replaces the old, dropped without a word), and what each new connection starts
/// without: the eyes' shapes and posed eyes ([`crate::eye::clear`]: a new server's datablocks name other shapes and
/// constructors), the renderer's live poses ([`RenderResets::connection_reset`]) and what the game keeps of the old world
/// ([`ConnGame::world_gone`]: its sounds, say, when the new world numbers its effects from 1).
pub fn install_conn<G: ConnGame>(st: &mut ClientState<G>, conn: Box<ClientConn<G>>) {
    st.conn.connects += 1;
    G::installing(st);
    conn.game.installed();
    crate::eye::clear();
    conn.render.connection_reset();
    G::world_gone(st);
    st.conn.conn = Some(conn);
}

/// Ends the connection: a disconnect datagram with `reason` when given (and connected), then nothing more. A local connection sends
/// none (only a network connection does) and leaves the server half to the server (its pointer to this side reads none).
pub fn drop_conn<G: ConnGame>(client: &Rc<RefCell<ClientState<G>>>, reason: Option<&[u8]>) {
    let mut st = client.borrow_mut();
    let Some(mut conn) = st.conn.conn.take() else {
        return;
    };
    G::world_gone(&mut st);
    if conn.local.is_some() {
        st.net.borrow_mut().local_inbox = None;
        return;
    }
    if let Some(reason) = reason
        && conn.accepted
    {
        conn.feed
            .disconnect(&String::from_utf8_lossy(until_nul(reason)));
    }
    drop(conn);
    sync_door(&st);
}

fn run_events<G: ConnGame>(
    vm: &mut Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    obj: ObjId,
    events: Vec<FeedEvent<<G::Feed as Feed>::Event>>,
) {
    for ev in events {
        let steps = event_steps(vm, client, ev);
        for step in steps {
            let called = match &step {
                Step::Call(name, _) => String::from_utf8_lossy(name).into_owned(),
                Step::Method(name, _) => format!("the connection's {name}"),
                Step::Datablock(_) => "a datablock".to_string(),
                _ => "a step".to_string(),
            };
            let result = match step {
                Step::Print(line) => {
                    vm.print(&line);
                    Ok(())
                }
                Step::Call(name, args) => vm.invoke(None, &name, &args).map(|_| ()),
                Step::Method(name, args) => {
                    if vm.objects().contains(obj) {
                        vm.call_method(obj, name.as_bytes(), &args).map(|_| ())
                    } else {
                        Ok(())
                    }
                }
                Step::Run(run) => {
                    run(vm, client);
                    Ok(())
                }
                Step::Global(name, value) => {
                    vm.set_global(name, &String::from_utf8_lossy(&value));
                    Ok(())
                }
                Step::Datablock(body) => {
                    let values = G::db_values(&client.borrow());
                    let unpacked = client_game(vm).unpack_datablock(body.as_ref());
                    let received = match &unpacked {
                        Some(d) => crate::datablocks::receive(vm, d, values.as_ref()),
                        None => Ok(crate::datablocks::Received::Refused),
                    };
                    received.map(|r| match r {
                        crate::datablocks::Received::Refused => {
                            client.borrow_mut().datablocks_refused += 1;
                        }
                        // TGE's `SimDataBlockEvent::process`: a datablock the client made joins its connection (a
                        // group: out of DataBlockGroup), so `getDataBlock` finds a remote client's datablocks there.
                        crate::datablocks::Received::Added(id) => {
                            if vm.objects().contains(obj) {
                                vm.objects_mut().add(obj, id);
                            }
                        }
                        crate::datablocks::Received::Updated(_) => {}
                    })
                }
                Step::Delete => {
                    drop_conn(client, None);
                    if vm.objects().contains(obj) {
                        let _ = vm.delete_object(obj);
                    }
                    return;
                }
            };
            if let Err(e) = result {
                // A callback the engine stopped (its step limit, an error of the engine's): said, never a silent stall.
                vm.error(format!("ERROR: the engine stopped {called}: {e}"));
                crate::net::keep_error(client, e);
            }
            // A callback that deleted the connection's object ends it (no more of its events are run).
            if !vm.objects().contains(obj) {
                drop_conn(client, Some(b""));
                return;
            }
        }
    }
}

pub fn num(n: u32) -> Vec<u8> {
    n.to_string().into_bytes()
}

/// The script calls and lines of one event (TGE's order). Net strings and commands are resolved here, against the connection's
/// tables and the client's tag table.
/// The ghost-index widths of the game's servers.
fn ghost_widths(vm: &Vm) -> GhostWidths {
    GhostWidths::of(&*client_game(vm).codecs())
}

fn event_steps<G: ConnGame>(
    vm: &mut Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    ev: FeedEvent<<G::Feed as Feed>::Event>,
) -> Vec<Step<G>> {
    let call = |name: &str, args: Vec<Vec<u8>>| Step::Call(name.as_bytes().to_vec(), args);
    let print = |s: &str| Step::Print(s.as_bytes().to_vec());
    match ev {
        FeedEvent::ChallengeSent => G::challenge_sent(),
        FeedEvent::ChallengeAnswered => {
            let server = client.borrow().conn.conn.as_ref().map(|c| c.server);
            G::challenge_answered(server)
        }
        FeedEvent::Accepted => {
            if let Some(c) = client.borrow_mut().conn.conn.as_mut() {
                c.accepted = true;
                // The ghost table's size is the accept's (the game's widest at most, an engine server's up to 2^24): indices past it are kept
                // nowhere.
                if c.mirrors.is_empty() {
                    c.mirrors = crate::net::ghost_objects::Mirrors::for_ghost_bits(
                        c.feed.ghost_bits(),
                        ghost_widths(vm),
                    );
                }
            }
            vec![
                print("Connection established"),
                Step::Method("onConnectionAccepted", Vec::new()),
            ]
        }
        FeedEvent::Rejected(reason) => vec![
            Step::Method("onConnectRequestRejected", vec![reason]),
            Step::Delete,
        ],
        FeedEvent::HandshakeTimedOut => {
            vec![
                Step::Method("onConnectRequestTimedOut", Vec::new()),
                Step::Delete,
            ]
        }
        FeedEvent::Dropped(reason) => vec![
            print("Connection with server lost."),
            Step::Method("onConnectionDropped", vec![reason]),
            Step::Delete,
        ],
        FeedEvent::TimedOut => vec![
            Step::Method("onConnectionTimedOut", Vec::new()),
            Step::Delete,
        ],
        FeedEvent::NetString { slot, text } => {
            let mut steps = Vec::new();
            let Some(id) = vm.tags_mut().add_string(&text) else {
                return steps;
            };
            if G::print_mapping(vm) {
                steps.push(Step::Print(mapping_line(&text, slot)));
            }
            let old = client
                .borrow_mut()
                .conn
                .conn
                .as_mut()
                .and_then(|c| c.remote.set(slot, id, &text));
            if let Some(old) = old {
                release(vm, old);
            }
            steps
        }
        FeedEvent::Remote(cmd) => {
            let args = cmd.script_args();
            let st = client.borrow();
            let Some(c) = st.conn.conn.as_ref() else {
                return Vec::new();
            };
            let Some(r) = resolve(&args, &c.remote) else {
                return Vec::new();
            };
            let name = function_name(
                client_game(vm).codecs().command_prefix(cmd.variant, false),
                &r.name,
            );
            vec![Step::Call(name, r.args)]
        }
        FeedEvent::DataBlock { index, total, body } => {
            vec![
                call("onDataBlockObjectReceived", vec![num(index), num(total)]),
                Step::Datablock(body),
            ]
        }
        FeedEvent::GhostAlwaysStarted(n) => vec![call("onGhostAlwaysStarted", vec![num(n)])],
        FeedEvent::GhostAlwaysObject => vec![call("onGhostAlwaysObjectReceived", Vec::new())],
        FeedEvent::ControlSet => vec![Step::Method("initialControlSet", Vec::new())],
        FeedEvent::Head { move_ack, control } => {
            let mut st = client.borrow_mut();
            if let Some(c) = st.conn.conn.as_mut() {
                let c = &mut **c;
                c.moves.on_server_ack(move_ack);
                c.predict.on_head(move_ack, &control, &c.world, &c.moves);
            }
            Vec::new()
        }
        FeedEvent::Line(text) => vec![Step::Print(text)],
        // The game's own events.
        FeedEvent::Game(ev) => G::event_steps(vm, client, ev),
    }
}

/// The tick's move and packet, once the connection is accepted and a tick is due.
fn send_tick<G: ConnGame>(vm: &mut Vm, client: &Rc<RefCell<ClientState<G>>>, now: u32) {
    let pending = {
        let st = client.borrow();
        let Some(c) = st.conn.conn.as_ref() else {
            return;
        };
        if !c.accepted || c.last_tick.is_some_and(|t| now.wrapping_sub(t) < TICK_MS) {
            return;
        }
        c.moves.len()
    };
    let mut triggers = match client.borrow().conn.conn.as_ref() {
        Some(c) => c.triggers,
        None => return,
    };
    let keys = client_game(vm)
        .codecs()
        .control_state()
        .moves
        .format()
        .triggers();
    let mv = crate::net::moves::next_move(vm, &mut triggers, pending, keys);
    let first_person = crate::predict::first_person(vm);
    let speed = G::camera_speed(vm);
    let globals = crate::predict::ClientGlobals::of(vm);
    let mut st = client.borrow_mut();
    let Some(c) = st.conn.conn.as_mut() else {
        return;
    };
    let c = &mut **c;
    c.triggers = triggers;
    c.last_tick = Some(now);
    // The game's world before the moves ([`ConnGame::moving`]).
    G::moving(&mut c.world, c.finished_initial_ghost);
    if let Some(m) = mv {
        c.moves.push(m);
    }
    // The camera distance moves towards first or third person, then the control object runs the moves it has not run (after a
    // correction: every one the server has not acknowledged).
    c.predict.camera_advance(first_person, speed, now);
    c.predict.set_view_globals(vm);
    c.predict.process(&c.moves, &c.world, &globals);
    c.predict.ghost_bits = c.feed.ghost_bits();
    let checksum = c.predict.checksum();
    if checksum != 0 {
        c.predict.stats.predicted_packets += 1;
    }
    let block = ClientBlock {
        camera_flag: c.predict.camera_pos == 0.0,
        checksum,
        moves: c.moves.block(),
        first_person: None,
        fov: c.fov.take(),
    };
    c.packets += 1;
    c.feed.send_tick(&mut c.world, &block);
}

// ---- the natives

type Prev = Rc<dyn Fn(&mut Vm, &NativeCall) -> Native>;

/// Makes `f` the `GameConnection` method `name` (in the class table's namespace for it), keeping the native it replaces for the
/// objects that are not the client's.
pub fn method(
    vm: &mut Vm,
    name: &str,
    f: impl Fn(&mut Vm, &NativeCall, Option<&Prev>) -> Native + 'static,
) {
    let prev = vm.native_of("GameConnection", name).map(|t| t.3);
    let f = Rc::new(f);
    for class in ["GameConnection", "NetConnection"] {
        let (f2, p2) = (Rc::clone(&f), prev.clone());
        if torque_game::methods::register_method(vm, class, name, move |vm, c| {
            f2(vm, c, p2.as_ref())
        }) {
            return;
        }
    }
    vm.register_in("GameConnection", name, 0, 0, "", move |vm, c| {
        f(vm, c, prev.as_ref())
    });
}

pub fn arg(c: &NativeCall, i: usize) -> &[u8] {
    until_nul(c.argv.get(i).map_or(&[][..], Vec::as_slice))
}

fn cut(b: &[u8]) -> Vec<u8> {
    b[..b.len().min(MAX_ARG)].to_vec()
}

/// Whether the client's connection is a local one whose server half still exists.
pub fn local_live<G: ConnGame>(vm: &Vm, client: &Rc<RefCell<ClientState<G>>>) -> bool {
    client
        .borrow()
        .conn
        .conn
        .as_ref()
        .is_some_and(|c| c.local_live(vm))
}

/// The client's connection object `c` runs on, if it is one.
pub fn is_conn<G: ConnGame>(client: &Rc<RefCell<ClientState<G>>>, c: &NativeCall) -> bool {
    c.this_object.is_some_and(|o| {
        client
            .borrow()
            .conn
            .conn
            .as_ref()
            .is_some_and(|conn| conn.obj == o)
    })
}

pub fn fallback(vm: &mut Vm, c: &NativeCall, prev: Option<&Prev>, neutral: Native) -> Native {
    match prev {
        Some(p) => p(vm, c),
        None => neutral,
    }
}

/// Registers the client's connection natives (after the server host's: the ones it replaces stay for the server's objects).
pub fn register<G: ConnGame>(vm: &mut Vm, client: &Rc<RefCell<ClientState<G>>>, gui: &Gui) {
    crate::net::ghost_objects::register(vm, client);
    let st = Rc::clone(client);
    method(vm, "setConnectArgs", move |_, c, _| {
        let Some(obj) = c.this_object else {
            return Native::Void;
        };
        let args: Vec<Vec<u8>> = (2..7).map(|i| cut(arg(c, i))).collect();
        let mut s = st.borrow_mut();
        if s.conn.pending.len() < MAX_PENDING || s.conn.pending.contains_key(&obj) {
            s.conn.pending.entry(obj).or_default().args = args;
        }
        Native::Void
    });
    let st = Rc::clone(client);
    method(vm, "setJoinPassword", move |_, c, _| {
        if let Some(obj) = c.this_object {
            let mut s = st.borrow_mut();
            if s.conn.pending.len() < MAX_PENDING || s.conn.pending.contains_key(&obj) {
                s.conn.pending.entry(obj).or_default().password = cut(arg(c, 2));
            }
        }
        Native::Void
    });
    let st = Rc::clone(client);
    method(vm, "connect", move |vm, c, _| {
        let Some(obj) = c.this_object else {
            return Native::Void;
        };
        let root = st.borrow().root.clone();
        if !file_gate::<G>(&root, c.file.as_deref()) {
            return Native::Void;
        }
        connect(vm, &st, obj, arg(c, 2))
    });
    let st = Rc::clone(client);
    method(vm, "connectLocal", move |vm, c, _| {
        let Some(obj) = c.this_object.filter(|o| vm.objects().contains(*o)) else {
            return Native::Str(UNKNOWN_ERROR.to_vec());
        };
        connect_local(vm, &st, obj)
    });
    let st = Rc::clone(client);
    method(vm, "delete", move |vm, c, prev| {
        if is_conn(&st, c) {
            let reason = c.argv.get(2).map_or(&[][..], |r| until_nul(r)).to_vec();
            drop_conn(&st, Some(&reason));
        }
        if let Some(obj) = c.this_object {
            st.borrow_mut().conn.pending.remove(&obj);
        }
        fallback(vm, c, prev, Native::Void)
    });
    let st = Rc::clone(client);
    method(vm, "isConnected", move |vm, c, prev| {
        if is_conn(&st, c) {
            return Native::Bool(st.borrow().conn.conn.as_ref().is_some_and(|x| x.accepted));
        }
        fallback(vm, c, prev, Native::Bool(false))
    });
    for &name in G::local_test_names() {
        let st = Rc::clone(client);
        method(vm, name, move |vm, c, prev| {
            if is_conn(&st, c) {
                return Native::Bool(local_live(vm, &st));
            }
            fallback(vm, c, prev, Native::Bool(false))
        });
    }
    let st = Rc::clone(client);
    method(vm, "getAddress", move |vm, c, prev| {
        if is_conn(&st, c) {
            if local_live(vm, &st) {
                return Native::Str(b"local".to_vec());
            }
            let a = st.borrow().conn.conn.as_ref().map(|x| {
                if x.local.is_some() {
                    client_game(vm).codecs().local_address_text()
                } else {
                    x.server.to_string()
                }
            });
            return Native::Str(a.unwrap_or_default().into_bytes());
        }
        fallback(vm, c, prev, Native::Str(Vec::new()))
    });
    let st = Rc::clone(client);
    method(vm, "getPort", move |vm, c, prev| {
        if is_conn(&st, c) {
            let p = st
                .borrow()
                .conn
                .conn
                .as_ref()
                .map_or(0, |x| x.server.port());
            return Native::Int(i32::from(p));
        }
        fallback(vm, c, prev, Native::Int(0))
    });
    let st = Rc::clone(client);
    method(vm, "getPing", move |vm, c, prev| {
        if is_conn(&st, c) {
            return Native::Int(0);
        }
        fallback(vm, c, prev, Native::Int(0))
    });
    // `getProtocolVersion()`: the version the server's accept named.
    let st = Rc::clone(client);
    method(vm, "getProtocolVersion", move |vm, c, prev| {
        if is_conn(&st, c) {
            let v = st
                .borrow()
                .conn
                .conn
                .as_ref()
                .map_or(0, |x| x.feed.protocol_version());
            return Native::Int(i32::try_from(v).unwrap_or(0));
        }
        fallback(vm, c, prev, Native::Int(0))
    });
    // `getControlCameraFov()` on the client: the connection's control-object fov ([`crate::zoom::ConnFov`],
    // 90 until the server or `setControlCameraFov` changes it).
    let st = Rc::clone(client);
    method(vm, "getControlCameraFov", move |vm, c, prev| {
        if is_conn(&st, c) {
            let fov = st.borrow().conn.conn.as_ref().map_or(0.0, |x| x.fov.camera);
            return Native::Float(f64::from(fov));
        }
        fallback(vm, c, prev, Native::Float(0.0))
    });
    let st = Rc::clone(client);
    method(vm, "getGhostsActive", move |vm, c, prev| {
        if is_conn(&st, c) {
            let n = st
                .borrow()
                .conn
                .conn
                .as_ref()
                .map_or(0, |x| x.world.ordinary_ghosts());
            return Native::Int(i32::try_from(n).unwrap_or(i32::MAX));
        }
        fallback(vm, c, prev, Native::Int(0))
    });
    // `getControlObject()`: the control object's id (its ghost's script object, [`crate::ghost_objects`]), 0 for none.
    let st = Rc::clone(client);
    method(vm, "getControlObject", move |vm, c, prev| {
        if is_conn(&st, c) {
            let index = st
                .borrow()
                .conn
                .conn
                .as_ref()
                .and_then(|x| x.world.control.object);
            let obj = index.and_then(|i| crate::net::ghost_objects::object_of(&st, i));
            return Native::Int(obj.map_or(0, |o| i32::try_from(o).unwrap_or(0)));
        }
        fallback(vm, c, prev, Native::Int(0))
    });
    // `resolveGhostID(index)`: a negative index is 0; else the id of the ghost's object, 0 for none.
    let st = Rc::clone(client);
    method(vm, "resolveGhostID", move |vm, c, prev| {
        if is_conn(&st, c) {
            let index = ts_vm::text::atoi(arg(c, 2));
            let obj = u32::try_from(index)
                .ok()
                .and_then(|i| crate::net::ghost_objects::object_of(&st, i));
            return Native::Int(obj.map_or(0, |o| i32::try_from(o).unwrap_or(0)));
        }
        fallback(vm, c, prev, Native::Int(0))
    });
    // `getGhostID(obj)` on the client's connection, which ghosts nothing to its server: what is not a
    // NetObject is the error and -1; a NetObject answers its own net index, the ghost index of a ghost's object (-1 for any other).
    let st = Rc::clone(client);
    method(vm, "getGhostID", move |vm, c, prev| {
        if is_conn(&st, c) {
            let objects = vm.objects();
            let net_object = objects.find(arg(c, 2)).filter(|o| {
                let class = objects.classes.find(b"NetObject");
                matches!((class, objects.get(*o)), (Some(k), Some(ob)) if objects.classes.is_a(ob.class(), k))
            });
            let Some(obj) = net_object else {
                vm.error(
                    "NetConnection::serverToGhostID - could not find specified object".to_string(),
                );
                return Native::Int(-1);
            };
            let index = st
                .borrow()
                .conn
                .conn
                .as_ref()
                .and_then(|x| x.mirrors.index_of(obj));
            return Native::Int(index.map_or(-1, |i| i as i32));
        }
        fallback(vm, c, prev, Native::Int(-1))
    });
    let st = Rc::clone(client);
    method(vm, "getPacketLoss", move |vm, c, prev| {
        if is_conn(&st, c) {
            return Native::Int(0);
        }
        fallback(vm, c, prev, Native::Int(0))
    });
    for name in ["isDemoRecording", "isDemoPlaying"] {
        let st = Rc::clone(client);
        method(vm, name, move |vm, c, prev| {
            if is_conn(&st, c) {
                return Native::Bool(false);
            }
            fallback(vm, c, prev, Native::Void)
        });
    }
    // `lockMouse(bool)`: the window keeps the mouse (the window client reads it; headless, nothing).
    let st = Rc::clone(client);
    vm.register(
        "lockMouse",
        2,
        2,
        "(bool isLocked)Lock the mouse (or not, depending on the argument's value) to the window.",
        move |_, c| {
            st.borrow_mut().mouse_locked = ts_vm::fields::atob(arg(c, 1));
            Native::Void
        },
    );
    // commandToServer(tag, ...): over the connection to the server, once there is one.
    let st = Rc::clone(client);
    vm.register("commandToServer", 2, 21, "", move |vm, c| {
        let bytes = c
            .argv
            .iter()
            .skip(1)
            .map(Vec::len)
            .fold(0usize, usize::saturating_add);
        if bytes > 0 && !vm.charge_work(u64::try_from(bytes).unwrap_or(u64::MAX)) {
            return Native::Fail(VmError::StepLimit);
        }
        command_to_server(vm, &st, c.argv.get(1..).unwrap_or(&[]))
    });
    G::register_natives(vm, client, gui);
}

fn command_to_server<G: ConnGame>(
    vm: &mut Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    args: &[Vec<u8>],
) -> Native {
    let result = {
        let mut st = client.borrow_mut();
        let Some(conn) = st.conn.conn.as_mut().filter(|c| c.accepted) else {
            return Native::Void;
        };
        let mut trial = conn.send.clone();
        let mut scratch: EventQueue<NetEvent> = EventQueue::new();
        match send_command(&mut scratch, &mut trial, &VmTags(vm), args) {
            Ok(sent) => {
                let events: Vec<NetEvent> =
                    scratch.release_all().into_iter().map(|s| s.event).collect();
                if events.len() > conn.feed.room_ordered() {
                    Err(SendError::Backpressure)
                } else {
                    conn.send = trial;
                    conn.feed.post_net(events);
                    Ok(sent)
                }
            }
            Err(e) => Err(e),
        }
    };
    match result {
        Ok(sent) => {
            for &tag in &sent.acquired {
                if let Some(text) = vm.tags().lookup(tag).map(<[u8]>::to_vec) {
                    let _ = vm.tags_mut().add_string(&text);
                }
            }
            for &tag in &sent.released {
                release(vm, tag);
            }
        }
        Err(SendError::NotATag) => {
            vm.error("Remote Command Error - command must be a tag.".to_string());
        }
        Err(_) => {}
    }
    Native::Void
}

/// A seeded connect sequence (TGE draws one at random per connection; the engine's is the Sim clock's and the object's).
fn connect_seq(vm: &Vm, obj: ObjId) -> u32 {
    vm.sim_time().wrapping_mul(0x9e37_79b9) ^ obj.wrapping_mul(0x85eb_ca6b) ^ 0x6d31_2065
}

pub fn connect<G: ConnGame>(
    vm: &mut Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    obj: ObjId,
    address: &[u8],
) -> Native {
    let Some(server) = crate::net::query::parse_address(address) else {
        vm.error(format!(
            "NetConnection::connect: invalid address - {}",
            String::from_utf8_lossy(address)
        ));
        return match G::invalid_address(vm) {
            Ok(()) => Native::Void,
            Err(e) => Native::Fail(e),
        };
    };
    let lan = crate::net::is_lan_target(server) && *server.ip() != crate::net::BROADCAST;
    // A public server: only under the internet policy, only one the user chose (a permit an input event armed).
    let chosen =
        !lan && {
            let st = client.borrow();
            let net = st.net.borrow();
            net.policy == crate::net::NetPolicy::Internet && crate::net::is_public(*server.ip())
        } && client
            .borrow()
            .net
            .borrow_mut()
            .internet
            .take_permit(server, vm.sim_time());
    if !lan && !chosen {
        let internet = {
            let st = client.borrow();
            let mut net = st.net.borrow_mut();
            if net.policy == crate::net::NetPolicy::Internet {
                net.internet.unpermitted += 1;
            }
            net.policy == crate::net::NetPolicy::Internet
        };
        let due = vm
            .sim_time()
            .wrapping_add(G::HANDSHAKE_WAIT_MS * (G::HANDSHAKE_TRIES + 1));
        {
            let mut st = client.borrow_mut();
            st.conn.refused += 1;
            st.net.borrow_mut().stats.refused += 1;
            if st.conn.doomed.len() < MAX_PENDING {
                st.conn.doomed.push(Doomed { obj, due });
            }
        }
        let why = if internet {
            G::refused_internet_reason()
        } else {
            "LAN servers only"
        };
        vm.print(
            format!(
                "connect: refused {}: {why}",
                String::from_utf8_lossy(address)
            )
            .as_bytes(),
        );
        return Native::Void;
    }
    if chosen {
        client.borrow().net.borrow_mut().internet.server = Some(server);
    }
    let root = client.borrow().root.clone();
    if let Err(e) = G::ready(vm, client) {
        vm.error(format!("NetConnection::connect: {e}"));
        return Native::Void;
    }
    let args = client
        .borrow_mut()
        .conn
        .pending
        .remove(&obj)
        .unwrap_or_default();
    let mut connect_args = args.args.clone();
    connect_args.resize(5, Vec::new());
    let inbox = Rc::new(RefCell::new(VecDeque::new()));
    let net = Rc::clone(&client.borrow().net);
    let link = InboxLink {
        net,
        server,
        inbox: Rc::clone(&inbox),
    };
    let relay = relay_file(client);
    let render = client_game(vm).renderer();
    let start = Connect {
        link: Box::new(link),
        args: connect_args,
        password: args.password.clone(),
        seq: connect_seq(vm, obj),
        obj,
        relay,
        local: None,
        render: Rc::clone(&render),
    };
    let mut started = match G::start(vm, client, start) {
        Ok(s) => s,
        Err(e) => {
            vm.error(format!("NetConnection::connect: {e}"));
            return Native::Void;
        }
    };
    let events = started.feed.take_events(usize::MAX);
    {
        let mut st = client.borrow_mut();
        // One connection at a time: a new connect replaces the old one (which is dropped without a word).
        install_conn(
            &mut st,
            Box::new(ClientConn {
                obj,
                server,
                feed: started.feed,
                world: started.world,
                inbox,
                accepted: false,
                remote: RemoteStrings::new(),
                send: SendStrings::new(),
                moves: MoveQueue::new(),
                triggers: Default::default(),
                last_tick: None,
                finished_initial_ghost: false,
                predict: {
                    let mut p = Predictor::new(root.clone());
                    p.static_world.reader = started.reader.take();
                    p
                },
                packets: 0,
                local: None,
                fov: Default::default(),
                mirrors: crate::net::ghost_objects::Mirrors::new(ghost_widths(vm)),
                game: started.part,
                render,
            }),
        );
    }
    // `connect` sends the challenge itself: its callback and line run inside the call, as TGE's.
    run_events(vm, client, obj, events);
    Native::Void
}

/// The connection's record file, when the host keeps one ([`ConnSlot::relay`]).
fn relay_file<G: ConnGame>(client: &Rc<RefCell<ClientState<G>>>) -> Option<Box<dyn io::Write>> {
    client
        .borrow()
        .conn
        .relay
        .as_ref()
        .and_then(|p| std::fs::File::create(p).ok())
        .map(|f| Box::new(io::BufWriter::new(f)) as Box<dyn io::Write>)
}

// ---- the local connection

/// The client half's link of a local connection: its packets go to the server half's inbox (from
/// [`LOCAL_ADDR`]); the server half's come into `inbox` (the host's transport puts them there).
pub struct LocalLink {
    pub net: Rc<RefCell<ClientNet>>,
    pub inbox: Rc<RefCell<VecDeque<Vec<u8>>>>,
}

impl Link for LocalLink {
    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.net.borrow_mut().to_server(LOCAL_ADDR, bytes.to_vec());
        Ok(())
    }

    fn recv(&mut self, _wait: Duration) -> io::Result<Option<Vec<u8>>> {
        Ok(self.inbox.borrow_mut().pop_front())
    }
}

/// What `connectLocal` returns on a failure that gave no reason (TGE's).
pub const UNKNOWN_ERROR: &[u8] = b"Unknown Error";

/// `connectLocal()` (the game's server half: [`ConnGame::local_request`]): the server half registered and asked (`onConnectRequest`), the client half
/// connected at once, then the client's `onConnectionEstablished` (`Connection established`, `onConnectionAccepted`) before the server's
/// (`ClientGroup.add`, `onConnect`), the server half named `LocalClientConnection`. Returns "" (the script ignores it). A refusal
/// deletes both halves, each with its `onDrop` (TGE's error path: neither is a connection to a server yet). A failure of the engine's own
/// (no network host, the game's tables unreadable) is printed and reported through the script's `onConnectionError` (back to the
/// main menu with a message box), never a hang.
fn connect_local<G: ConnGame>(
    vm: &mut Vm,
    client: &Rc<RefCell<ClientState<G>>>,
    obj: ObjId,
) -> Native {
    let args = client
        .borrow_mut()
        .conn
        .pending
        .remove(&obj)
        .unwrap_or_default();
    let root = client.borrow().root.clone();
    let accept = match G::local_request(vm, client, obj, &args) {
        LocalAnswer::Accepted(a) => a,
        LocalAnswer::Failure(why) => return engine_failure::<G>(vm, obj, &why),
        LocalAnswer::Refused { reason, ran } => {
            if let Err(e) = ran {
                crate::net::keep_error(client, e);
            }
            // The client half's `onRemove`: not yet a connection to a server, so `onDrop` runs on it too.
            if vm.objects().contains(obj) {
                if let Err(e) = vm.call_method(obj, b"onDrop", &[Vec::new()]) {
                    crate::net::keep_error(client, e);
                }
                if vm.objects().contains(obj) {
                    let _ = vm.delete_object_quiet(obj);
                }
            }
            return Native::Str(G::local_error(&reason));
        }
    };
    let mut connect_args = args.args.clone();
    connect_args.resize(5, Vec::new());
    let inbox = Rc::new(RefCell::new(VecDeque::new()));
    let net = Rc::clone(&client.borrow().net);
    let link = LocalLink {
        net: Rc::clone(&net),
        inbox: Rc::clone(&inbox),
    };
    let relay = relay_file(client);
    let render = client_game(vm).renderer();
    let start = Connect {
        link: Box::new(link),
        args: connect_args,
        password: args.password.clone(),
        seq: 0,
        obj,
        relay,
        local: Some(accept),
        render: Rc::clone(&render),
    };
    let mut started = match G::start(vm, client, start) {
        Ok(s) => s,
        Err(e) => return engine_failure::<G>(vm, obj, &e),
    };
    net.borrow_mut().local_inbox = Some(Rc::clone(&inbox));
    let server = match LOCAL_ADDR {
        SocketAddr::V4(v4) => v4,
        SocketAddr::V6(_) => SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0),
    };
    {
        let mut st = client.borrow_mut();
        install_conn(
            &mut st,
            Box::new(ClientConn {
                obj,
                server,
                feed: started.feed,
                world: started.world,
                inbox,
                accepted: true,
                remote: RemoteStrings::new(),
                send: SendStrings::new(),
                moves: MoveQueue::new(),
                triggers: Default::default(),
                last_tick: None,
                finished_initial_ghost: false,
                predict: {
                    let mut p = Predictor::new(root.clone());
                    p.static_world.reader = started.reader.take();
                    p
                },
                packets: 0,
                local: Some(accept.obj),
                fov: Default::default(),
                mirrors: crate::net::ghost_objects::Mirrors::for_ghost_bits(
                    accept.ghost_bits,
                    ghost_widths(vm),
                ),
                game: started.part,
                render,
            }),
        );
    }
    // The client half's `onConnectionEstablished(true)`, then the server half's.
    vm.print(b"Connection established");
    if vm.objects().contains(obj)
        && let Err(e) = vm.call_method(obj, b"onConnectionAccepted", &[])
    {
        crate::net::keep_error(client, e);
    }
    if let Err(e) = G::local_establish(vm, client, accept.obj) {
        crate::net::keep_error(client, e);
    }
    Native::Str(Vec::new())
}

/// A local connection the engine cannot make: the error printed and the script's `onConnectionError(%msg)` run (its cleanup and
/// message box: the main menu again), then the object deleted.
fn engine_failure<G: ConnGame>(vm: &mut Vm, obj: ObjId, why: &str) -> Native {
    vm.error(format!("connectLocal: {why}"));
    let result = vm.call_method(obj, b"onConnectionError", &[why.as_bytes().to_vec()]);
    if vm.objects().contains(obj) {
        let _ = vm.delete_object_quiet(obj);
    }
    match result {
        Ok(_) => Native::Str(G::local_error(why.as_bytes())),
        Err(e) => Native::Fail(e),
    }
}

/// The frame's view distance cap (`ClientWorld::max_view_distance`) from the game's variable (`pref`, as read:
/// [`ConnGame::view_distance_pref`]), the last script's cap (`called`: its distance and the Sky's distance fields then,
/// [`ClientState::max_view_distance`]) and the Sky's distance fields now (`sky`), and whether the call still holds. A call holds until the Sky's distances change (the Sky's update sets the
/// scene's distances from the Sky and the pref again); otherwise the pref caps, as the Sky's update does. A pref that is not a
/// positive number caps nothing (engine-only: a literal reading would draw nothing past it).
pub fn view_cap(
    pref: Option<f32>,
    called: Option<(f32, Option<[u32; 2]>)>,
    sky: Option<[u32; 2]>,
) -> (Option<f32>, bool) {
    if let Some((v, at)) = called
        && at == sky
    {
        return (Some(v), true);
    }
    let pref = pref.filter(|v| v.is_finite() && *v > 0.0);
    (pref, called.is_none())
}

#[cfg(test)]
#[path = "conn_tests.rs"]
mod tests;
