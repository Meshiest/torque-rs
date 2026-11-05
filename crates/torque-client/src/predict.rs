//! The client's prediction of its control object, as TGE's client does it:
//!
//! - **The move list** (`GameConnection` `mLastMoveAck`, `mLastClientMove`, `mFirstMoveIndex`): every client
//!   tick pushes the move `getNextMove` made, and the process list ticks the control object once for every move from
//!   `mLastClientMove` to the end (`ProcessList::advanceObjects` with `getMoveList`/`clearMoves`), so a normal tick runs the new move
//!   only.
//! - **A server head** (`GameConnection::readPacket`, client branch): the move ack (raised to the first move held), and
//!   `mLastClientMove` raised to it; a control section whose checksum differed sets `mLastClientMove = mLastMoveAck` and calls the
//!   control object's `readPacketData`: the next tick runs again every move the server has not acknowledged, from the
//!   server's state. A matching section changes nothing (its three floats are the stream's compression point).
//! - **`Player::readPacketData`**, of the set's words ([`PlayerWords`]): ShapeBase's `setEnergyLevel` (clamped to the datablock's
//!   maximum), the word a game's packet data adds ([`game::PredictGame::read_packet_flag`]; none in TGE's), when not mounted the
//!   position, the velocity and the contact counter (4 bits), the head's x and z and the rotation's z (x and y 0), `setPosition`.
//!   Everything else of the client's Player stays what its own steps left.
//! - **The step** is `Player::processTick` of a ghost with a controlling client: the ShapeBase tick (energy, damage, the
//!   images), the working set, `updateMove`, the look animation, `updatePos`; the server's animation steps and
//!   every script callback are the server's only. torque-sim's `player` is that step (bit-exact with the server's); here it runs over the
//!   client's world ([`ClientCollision`]: the ghosts as the client holds them) with no callbacks.
//! - **The checksum** the client's packet carries (`GameConnection::writePacket`: the control object's
//!   `getPacketDataChecksum`) is the checksum of its `writePacketData` after every move processed
//!   (`net::control::checksum`).
//! - **The camera distance** (the head's flag `camera distance == 0` and the Player's free-look test) follows `$firstPerson`
//!   at the camera speed a second ([`crate::conn::ConnGame::camera_speed`]; TGE: 10) (`getControlCameraTransform`: towards 0
//!   in first person, 1 otherwise).
//!
//! **Engine limits**: the client's collision world holds the game's static objects, the other Players' boxes and the shapes the
//! container builds; the Player has no skeleton on the client (the animation does not change the words a walk or a jump sends); a
//! control object the game does not predict sends checksum 0 (the server then resends every packet).
//!
//! **The game's part** ([`game::PredictGame`], from [`crate::game::ClientGame::prediction`]; Torque's answers without one):
//! the Player class and its packet word, the moves' triggers, the type masks and working order, the static collision world
//! ([`StaticIndex`]: objects kept apart from the container's and asked first, and objects made of other ghosts), the predicted
//! Camera ([`game::ControlCamera`]), the WheeledVehicle class and steering rules, the shape slot rule, the projectile ghosts
//! ([`game::GhostProjectiles`]), the third-person and vehicle cameras, smooth aim's next move and the game's preferences
//! ([`game::GamePrefs`]).
//!
//! Credit: follows TGE's client-side prediction as Torque3D carries it (MIT: `T3D/gameBase/gameConnection.cpp`,
//! `T3D/player.cpp`, `T3D/shapeBase.cpp`, each Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.;
//! `T3D/camera.cpp`, `T3D/vehicles/vehicle.cpp`, each Copyright (c) 2012 GarageGames, LLC).

use std::any::Any;
use std::fmt::Debug;
use std::path::PathBuf;
use std::rc::Rc;

use collide::convex::{ConvexSource, ObjectView};
use collide::math::{Box3, F3, Matrix, affine_inverse, mul_box, quat_matrix};
use collide::mesh::Hull;
use collide::{Exhausted, ObjectId};
use ghost_world::Ghost;
use ghost_world::effects::Effect;
use ghost_world::world::{Eye, Pose};
use ghosts::Value;
use net::control::{ControlData, ControlSection, checksum};
use net::moves::{Move, MoveQueue};
use sim::player::host::{LiquidStatics, PlayerHost};
use sim::player::{NoSkeleton, Player, PlayerClass, PlayerData, PlayerState, Skeleton};
use torque_game::scene::container::CandidateOrder;
use torque_game::sim_classes::PlayerKind;
use torque_scene::transform;
use ts_vm::vm::Vm;
use vehicle::words::VehicleWords;

use crate::records::{
    ClassKind, EngineDatablock, EngineRecord, EngineSet, EngineTail, FreeMotion, PlayerWords,
    VehicleUpdate,
};
use game::PredictWorld;

/// The id the predicted Player has in the client's collision world (never a ghost's: those are their index + 1).
pub const SELF_ID: ObjectId = ObjectId(u32::MAX - 1);
/// The most jump sounds kept between two views (a stalled window's ticks).
pub const MAX_JUMP_SOUNDS: usize = 8;

/// The work one tick's prediction may do in all (the world's construction and every move it runs, new and replayed): past it the
/// replay stops, the last whole step's state kept ([`PredictStats::budget_stops`]).
pub const TICK_WORK: u64 = 50_000_000;
/// The most moves one tick runs: the list never holds more than `getNextMove`'s limit plus the move it then makes (45 + 1), so a
/// correction replays at most that many.
pub const MAX_REPLAY: usize = crate::net::moves::MAX_PENDING_MOVES + 1;
/// The work units the construction of the client's world takes per ghost looked at, and per Player ghost's datablock read.
pub const GHOST_WORK: u64 = 4;
pub const DATABLOCK_WORK: u64 = 64;
/// The ghost-id width the checksum's stream is written with until the connection's accept gives its own ([`Predictor::ghost_bits`]).
pub const GHOST_BITS: u8 = 18;
/// A Player's type mask (`PlayerObjectType | ShapeBaseObjectType | GameBaseObjectType`).
pub const PLAYER_TYPE_MASK: u32 = 0x4c00;
/// A dead Player's type mask (`Player::updateDamageState`: `CorpseObjectType` in place of `PlayerObjectType` once the damage
/// state is not `Enabled`, on the client as the ghost's damage state arrives).
pub const CORPSE_TYPE_MASK: u32 =
    (PLAYER_TYPE_MASK & !collide::types::PLAYER) | collide::types::CORPSE;

/// The fraction of a tick left when the client draws a frame (the client process list's `((-t) & 31) / 32` of the
/// client's own time, what `interpolateTick` takes): the frame's phase is in no packet, so a fixed phase of 31/32 is taken.
pub const FRAME_DT: f32 = 31.0 / 32.0;

/// What the predictor did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PredictStats {
    /// Server heads whose control section resent the state (a checksum that differed: a correction, or the first state).
    pub resends: u64,
    /// Heads whose checksum matched.
    pub matching: u64,
    /// Moves stepped (new and replayed).
    pub steps: u64,
    /// Packets sent with the prediction's checksum (not 0).
    pub predicted_packets: u64,
    /// Ticks whose prediction ran out of [`TICK_WORK`] (the replay stopped there).
    pub budget_stops: u64,
    /// The resends once the server had matched a checksum (the ones before answer packets sent before the client had a state to
    /// report), by what they answer ([`ResendClass`]): the server's events, their tails, and the corrections (the rest: the client
    /// predicted the server's state wrong).
    pub events: u64,
    pub event_tails: u64,
    pub corrections: u64,
}

/// What a resend after the first match answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResendClass {
    /// The server changed the control object's kind or ghost ([`ControlKind`]): a mount, the vehicle becoming the control object,
    /// a dismount, a death's camera, a respawn. Only the server decides these (its scripts), and the server resends on each
    /// (`setControlObject` forces a mismatch; a mount changes `writePacketData`'s free flag).
    Event,
    /// A resend for a packet the client sent before the last event's resend reached it (its ack at or under the end of the moves
    /// the client had sent then): the client could not know.
    Tail,
    /// The rest: the client's prediction of the server's state was wrong.
    Correction,
}

/// The kind of state a control section carries (the event test): the control object's ghost and what it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    /// A Player on its feet.
    Free(u32),
    /// A mounted Player with no control object of its own.
    Mounted(u32),
    /// A Player whose control object is the ghost given (a driver).
    Controlling(u32, u32),
    /// Any other control object (a Camera, a vehicle).
    Other(u32),
}

impl ControlKind {
    /// The kind of `data`, the packet data of set `S`'s control object `index`.
    pub fn of<S: EngineSet>(index: u32, data: &dyn ControlData) -> ControlKind {
        match S::read_player(data) {
            Some(p) if p.words.free.is_some() => ControlKind::Free(index),
            Some(p) => match p.controlled {
                Some((t, _)) => ControlKind::Controlling(index, t),
                None => ControlKind::Mounted(index),
            },
            None => ControlKind::Other(index),
        }
    }
}

/// The predicted Player.
#[derive(Clone)]
pub struct Predicted {
    /// The control object's ghost index and its datablock.
    pub index: u32,
    pub datablock: u32,
    pub data: PlayerData,
    /// The Player class its datablock makes (the game's, [`game::PredictGame::player_kind`]).
    pub kind: Rc<dyn PlayerKind>,
    pub player: Player,
    /// The vehicle the Player drives (its control object), when it drives one.
    pub vehicle: Option<drive::DrivenVehicle>,
}

/// The client's control object as it predicts it, over the game's world `W`.
#[derive(Clone)]
pub struct Predictor<W: PredictWorld> {
    pub root: PathBuf,
    pub predicted: Option<Predicted>,
    /// TGE's `mLastClientMove`: the index of the next move the control object has not run.
    pub last_client_move: u32,
    /// The camera distance: 0 in first person.
    pub camera_pos: f32,
    /// The virtual time the camera distance last moved at (`GameConnection`).
    pub camera_ms: Option<u32>,
    /// `$mvFreeLook` as last read (a driver's third-person camera reads it, [`VehicleView::free_look`]).
    pub free_look: bool,
    pub stats: PredictStats,
    /// The width of a ghost index the checksum's stream writes (an orbit camera's target, a mount): the connection's, set from
    /// its accept each packet ([`GHOST_BITS`] until then).
    pub ghost_bits: u8,
    statics: LiquidStatics,
    /// The state after each move run, by the move's index (the last [`HISTORY`]): what a correction is compared with.
    history: std::collections::VecDeque<(u32, Box<dyn ControlData>)>,
    /// The corrections: the ack, the server's state and the client's own at that ack (when it still had it).
    pub corrections: Vec<Correction>,
    /// The static collision world for the connection: the game's answers, the install's reader and the game's index.
    pub static_world: StaticWorld<W>,
    /// The shapes (vehicles) read from the install, once a connection.
    pub shapes: drive::ShapeCache,
    /// The control object when it is a Camera (the death's orbit camera): predicted instead of a Player.
    pub camera: Option<Box<dyn game::ControlCamera<W>>>,
    /// The vehicles nobody on this client drives, run forward as TGE's client runs its ghosts (by ghost index), the last vehicle
    /// update read, and the end of the move list at the last tick (the client's ticks are its moves).
    pub ghost_vehicles: std::collections::BTreeMap<u32, drive::GhostVehicle>,
    /// The driven vehicle's updates that carried its state (no driver on the server's side) since the last resend, from the
    /// last with a pose on, in order, with their positions: applied to it when a resend ends the drive.
    pub driven_pending: Vec<(VehicleUpdate, Option<[f32; 3]>)>,
    /// The Projectile ghosts, run forward every tick as TGE's client runs them (the game's [`game::GhostProjectiles`]), and the explosions their own
    /// steps made since the last view.
    pub projectiles: Option<Box<dyn game::GhostProjectiles<W>>>,
    pub vehicle_seq: u64,
    pub last_end: Option<u32>,
    /// The last resent state's kind, and the end of the moves the client had sent when the last event's resend came.
    pub last_kind: Option<ControlKind>,
    pub tail_end: Option<u32>,
    /// How far back between the last tick and the one before the view is drawn (`Player::interpolateTick`'s `dt`, 0 the last
    /// tick's state, 1 the one before); a window sets it each frame, every other caller
    /// leaves it 0.
    pub back: f32,
    /// Smooth aim (an engine option the game turns on, [`game::PredictGame::smooth_aim`]): the control object's view and
    /// body drawn ahead of the latest tick by what the next move makes of [`Self::aim_input`], the move globals a window's frame read
    /// ([`aim`]); without it they are drawn between the last two ticks, a tick behind the mouse. The moves are unchanged. Without a
    /// frame's input (a tick's view, a headless client) the view is the interpolated one.
    pub smooth_aim: bool,
    pub aim_input: Option<aim::Waiting>,
    /// The yaw of the Player the rider controls (a mount, a turret) as this client's moves turn it: the ghost's yaw is the
    /// server's, quantised and some ticks old (the engine's heads write no control tail for a Player), so the client integrates
    /// the moves' turn itself ([`aim`]) and takes the ghost's only to start from and to settle on. `(ghost index,
    /// yaw and pitch, moves since a turn).
    pub mount_pred: Option<MountPred>,
    /// The game's preferences as the frame read them ([`game::GamePrefs`]: the next move's driver split and the cameras read them).
    pub prefs: game::GamePrefs,
    /// The newest move this client ran (a replay after a correction runs older ones again), and the points of the jumps its
    /// new moves made, for [`Predictor::update_view`] to sound.
    pub newest_move: Option<u32>,
    pub jumps: Vec<[f32; 3]>,
    /// The wheels' spins of the vehicles the client runs (advanced each frame by the window's frame view), and the
    /// FlyingVehicle ghosts' warps ([`look`]).
    pub spins: look::WheelSpins,
    /// The steering drawn for the vehicles the client runs, eased toward their ticks' ([`look::SteeringEase`]).
    pub steering: look::SteeringEase,
    /// The springs drawn for the vehicles the client runs, between their last two ticks' ([`look::SpringLooks`]).
    pub springs: look::SpringLooks,
    pub flying: look::FlyingWarps,
    /// The other Players, run on between their updates and warped to each new one ([`ghost_players`]).
    pub ghost_players: ghost_players::GhostPlayers,
    /// The client's virtual time at the last tick or frame view (ms): what the flying warps are drawn at.
    pub sim_ms: u32,
    /// Whether the game's answers were given (else the first `Vm` seen gives its game's).
    game_given: bool,
}

/// How the client reads a file of the install (a static object's own file, a shape, a map's files; `None`: not found or not to be
/// read).
pub type FileReader = Rc<dyn Fn(&[u8]) -> Option<Vec<u8>>>;

/// The most vehicles the client runs on its own, and the most ghost ticks one packet runs (a stall is not caught up further).
pub const MAX_GHOST_VEHICLES: usize = 64;
pub const MAX_GHOST_TICKS: u32 = 8;
/// The speed (units a second) under which a vehicle let go of takes the poses the server sent before ([`Predictor::driven_pending`]).
pub const RELEASE_MOVING: f32 = 1.0;

/// The most updates of the driven vehicle kept for the end of the drive ([`Predictor::driven_pending`]).
pub const MAX_DRIVEN_PENDING: usize = 64;

/// The states the predictor keeps for comparing a correction, and the corrections it keeps.
pub const HISTORY: usize = 128;
pub const MAX_CORRECTIONS: usize = 64;

/// A correction: the server resent the state at `ack` while the client had predicted `client` there.
#[derive(Debug, Clone)]
pub struct Correction {
    pub ack: u32,
    pub class: ResendClass,
    pub server: Box<dyn ControlData>,
    pub client: Option<Box<dyn ControlData>>,
}

impl<W: PredictWorld> Predictor<W> {
    /// A prediction with Torque's answers until a `Vm` with a game's is seen ([`Predictor::set_view_globals`]).
    pub fn new(root: PathBuf) -> Predictor<W> {
        Predictor {
            root,
            predicted: None,
            last_client_move: 0,
            camera_pos: 0.0,
            camera_ms: None,
            free_look: false,
            stats: PredictStats::default(),
            ghost_bits: GHOST_BITS,
            statics: LiquidStatics::default(),
            history: std::collections::VecDeque::new(),
            corrections: Vec::new(),
            static_world: StaticWorld::default(),
            shapes: drive::ShapeCache::default(),
            camera: None,
            ghost_vehicles: std::collections::BTreeMap::new(),
            driven_pending: Vec::new(),
            projectiles: None,
            newest_move: None,
            mount_pred: None,
            jumps: Vec::new(),
            vehicle_seq: 0,
            last_end: None,
            last_kind: None,
            tail_end: None,
            back: 0.0,
            smooth_aim: false,
            aim_input: None,
            prefs: game::GamePrefs::default(),
            spins: look::WheelSpins::default(),
            steering: look::SteeringEase::default(),
            springs: look::SpringLooks::default(),
            flying: look::FlyingWarps::default(),
            ghost_players: ghost_players::GhostPlayers::default(),
            sim_ms: 0,
            game_given: false,
        }
    }

    /// A prediction with `game`'s answers.
    pub fn with_game(root: PathBuf, game: Rc<dyn game::PredictGame<W>>) -> Predictor<W> {
        let mut p = Predictor::new(root);
        p.set_game(game);
        p
    }

    /// What the prediction asks the game from now on.
    pub fn set_game(&mut self, game: Rc<dyn game::PredictGame<W>>) {
        self.shapes = drive::ShapeCache::with_slots(game.shape_slots());
        self.projectiles = game.projectiles();
        self.static_world.set_game(game);
        self.game_given = true;
    }

    /// What the prediction asks the game.
    pub fn game(&self) -> &Rc<dyn game::PredictGame<W>> {
        &self.static_world.game.0
    }

    /// Classifies a resend after the first match (see [`ResendClass`]); `sent_end` is the end of the moves the client has sent.
    pub fn classify(&mut self, ack: u32, kind: ControlKind, sent_end: u32) -> ResendClass {
        let class = if self.last_kind != Some(kind) {
            self.tail_end = Some(sent_end);
            ResendClass::Event
        } else if self
            .tail_end
            .is_some_and(|end| (ack.wrapping_sub(end) as i32) <= 0)
        {
            ResendClass::Tail
        } else {
            ResendClass::Correction
        };
        self.last_kind = Some(kind);
        match class {
            ResendClass::Event => self.stats.events += 1,
            ResendClass::Tail => self.stats.event_tails += 1,
            ResendClass::Correction => self.stats.corrections += 1,
        }
        class
    }

    /// A server head: its ack (the move list already took it: `moves.first_index()`) and control section.
    ///
    /// A resend that leaves the predicted Player driving no vehicle (or another one) hands the vehicle it drove to the vehicles
    /// the client runs on its own (`Predictor::release_vehicle`).
    pub fn on_head(
        &mut self,
        ack: u32,
        control: &ControlSection<Box<dyn ControlData>>,
        world: &W,
        moves: &MoveQueue,
    ) {
        let before = match control {
            ControlSection::Resend { .. } => {
                self.predicted.as_ref().and_then(|p| p.vehicle.clone())
            }
            _ => None,
        };
        self.on_head_control(ack, control, world, moves);
        if matches!(control, ControlSection::Resend { .. }) {
            self.release_vehicle(before, world);
        }
    }

    /// TGE's client predicts the vehicle its Player drives on the vehicle's own ghost; when the Player stops driving it
    /// (`Player::readPacketData` with no control object: `setControlObject(0)`, which only clears the links), the
    /// ghost goes on from the state the client predicted, its warp and interpolation untouched, as an ordinary ghost: the server's
    /// next updates warp it from there (`Vehicle::unpackUpdate`) and a vehicle at rest gets none (the server sets the pose
    /// mask only when it moved). Dropping the driven vehicle would draw it, and collide with it, at the last pose an update had
    /// carried while nobody drove it: where the Player got in. So the driven vehicle becomes the ghost vehicle, then the updates
    /// its ghost got without a driver since the last resend that kept it are applied in order, as TGE's object reads them as they
    /// come; their poses only when it is all but still ([`RELEASE_MOVING`]). `before` is the driven vehicle before the resend.
    fn release_vehicle(&mut self, before: Option<drive::DrivenVehicle>, world: &W) {
        let now = self
            .predicted
            .as_ref()
            .and_then(|p| p.vehicle.as_ref())
            .map(|v| v.index);
        let pending = std::mem::take(&mut self.driven_pending);
        let Some(v) = before else { return };
        if now == Some(v.index)
            || !world.ghosts.contains_key(&v.index)
            || (self.ghost_vehicles.len() >= MAX_GHOST_VEHICLES
                && !self.ghost_vehicles.contains_key(&v.index))
        {
            return;
        }
        let index = v.index;
        let mut g = drive::GhostVehicle::released(v, self.back);
        // A vehicle still moving gets the server's next updates anyway; the poses kept are older than the state the client
        // predicted, and warping to one would pull the vehicle back along its way.
        let v = g.body.v.s.rigid.lin_vel;
        let moving = v.iter().map(|x| x * x).sum::<f32>() > RELEASE_MOVING * RELEASE_MOVING;
        for (rec, position) in pending {
            let rec = if moving {
                VehicleUpdate { pose: None, ..rec }
            } else {
                rec
            };
            g.unpack(&rec, position, true, &mut ClientCollision::empty());
        }
        self.ghost_vehicles.insert(index, g);
    }

    fn on_head_control(
        &mut self,
        ack: u32,
        control: &ControlSection<Box<dyn ControlData>>,
        world: &W,
        moves: &MoveQueue,
    ) {
        let first = moves.first_index();
        let ack = if ack.wrapping_sub(first) as i32 > 0 {
            ack
        } else {
            first
        };
        if (self.last_client_move.wrapping_sub(ack) as i32) < 0 {
            self.last_client_move = ack;
        }
        match control {
            ControlSection::None => {}
            ControlSection::Matching { .. } => self.stats.matching += 1,
            ControlSection::Resend { index, data } => {
                self.stats.resends += 1;
                self.last_client_move = ack;
                let kind = ControlKind::of::<W::Set>(*index, data.as_ref());
                let sent_end = first.wrapping_add(moves.moves().len() as u32);
                let class = if self.stats.matching > 0 {
                    Some(self.classify(ack, kind, sent_end))
                } else {
                    self.last_kind = Some(kind);
                    None
                };
                let player = <W::Set as EngineSet>::read_player(data.as_ref());
                let driven = player.as_ref().and_then(|p| {
                    let (tail, tail_data) = p.controlled?;
                    let v = <W::Set as EngineSet>::read_vehicle(tail_data)?;
                    (world
                        .ghosts
                        .get(&tail)
                        .and_then(|g| g.class)
                        .map(<W::Set as EngineSet>::class_kind)
                        == Some(ClassKind::WheeledVehicle))
                    .then_some((tail, v))
                });
                let game = Rc::clone(self.game());
                if let Some(cam) =
                    game.read_camera(self.camera.take(), *index, data.as_ref(), world)
                {
                    // `Camera::readPacketData`: the control object is a Camera. The Player the client predicted until now (the
                    // death's corpse the camera orbits) is the same object on TGE's client: its ghost goes on from the state the
                    // client had predicted.
                    let seed = self
                        .predicted
                        .take()
                        .map(|p| (p.index, p.player.s.position(), p.player.s.velocity));
                    self.camera = cam;
                    if let (Some(c), Some((pi, pos, vel))) = (self.camera.as_mut(), seed) {
                        c.watch(pi, pos, vel, world.player_move_seq);
                    }
                    return;
                }
                self.camera = None;
                match &player {
                    Some(p) if p.controlled.is_none() || driven.is_some() => {
                        if let Some(class) = class
                            && self.predicted.is_some()
                            && self.corrections.len() < MAX_CORRECTIONS
                        {
                            let client = self
                                .history
                                .iter()
                                .find(|(id, _)| id.wrapping_add(1) == ack)
                                .map(|(_, d)| d.clone());
                            self.corrections.push(Correction {
                                ack,
                                class,
                                server: data.clone(),
                                client,
                            });
                        }
                        self.read_packet_data(*index, &p.words, p.controlled.map(|c| c.0), world);
                        match driven {
                            Some((vi, v)) => self.read_vehicle(vi, &v, world),
                            None => {
                                if let Some(pr) = self.predicted.as_mut() {
                                    pr.vehicle = None;
                                }
                            }
                        }
                    }
                    // A mounted Player driving, a Camera, a vehicle: not predicted.
                    _ => self.predicted = None,
                }
            }
        }
    }

    /// `Player::readPacketData` into the predicted Player (made from its ghost's datablock when it is new): `p` its words,
    /// `controlled` the ghost of its control object.
    fn read_packet_data(
        &mut self,
        index: u32,
        p: &PlayerWords,
        controlled: Option<u32>,
        world: &W,
    ) {
        let Some(db_id) = world.ghosts.get(&index).and_then(|g| g.datablock()) else {
            self.predicted = None;
            return;
        };
        let fresh = self
            .predicted
            .as_ref()
            .is_none_or(|x| x.index != index || x.datablock != db_id);
        if fresh {
            let game = Rc::clone(self.game());
            let Some(data) = player_data(world, db_id, &*game) else {
                self.predicted = None;
                return;
            };
            let pos = p.free.map_or([0.0; 3], |f| f.position);
            let kind = game.player_kind(world, db_id);
            let mut s = kind.new_state(SELF_ID, &data, pos, p.rot_z);
            s.recharge_rate = world_f32(world, db_id, "rechargeRate").unwrap_or(0.0);
            self.predicted = Some(Predicted {
                index,
                datablock: db_id,
                data,
                kind,
                player: Player::new(s),
                vehicle: None,
            });
        }
        let game = Rc::clone(self.game());
        let Some(pr) = self.predicted.as_mut() else {
            return;
        };
        let s = &mut *pr.player.s;
        let e = p.energy;
        let max = pr.data.max_energy;
        s.energy = if e > max {
            max
        } else if 0.0 > e {
            0.0
        } else {
            e
        };
        game.read_packet_flag(s, p.flag);
        // The control object (`setControlObject` of the chain's ghost) and, for a driver, the mount it sits on. A Player
        // control object (a turret the Player rides) writes no tail in the engine's heads: the Player's mount when that is a Player or
        // AIPlayer this connection controls (its ghost's controlled bit), as the server's split has it.
        s.control_object = controlled
            .map(drive::ghost_id)
            .or_else(|| controlled_mount(world, index));
        let pos = match &p.free {
            Some(f) => {
                s.mount = None;
                let pos = f.position;
                s.velocity = f.velocity;
                s.delta_pos = pos;
                s.jump_surface_last = f.contact & 0xf;
                pos
            }
            None => {
                // Not free: the server's Player is mounted (`writePacketData` writes the position only when it is not). On TGE's
                // client the control Player is its ghost, mounted as its ShapeBase updates say; the update that mounts it may come
                // after this head in the same packet (or, with no control object yet, a tick before the vehicle becomes it), so the
                // mount stays pending until the tick looks for it.
                s.mount = s
                    .control_object
                    .or_else(|| ghost_mount(world, index))
                    .or(Some(PENDING_MOUNT));
                s.position()
            }
        };
        s.head[0] = p.head_x;
        s.head[2] = p.head_z;
        let rot = [0.0, 0.0, p.rot_z];
        s.place(pos, rot, None);
        s.delta_head = s.head;
        s.delta_rot = rot;
    }

    /// The control object's `readPacketData` for a WheeledVehicle the predicted Player drives: the vehicle the client already holds
    /// for that ghost, or a new one of its datablock (`None` and nothing predicted when the client cannot build it).
    fn read_vehicle(&mut self, index: u32, v: &VehicleWords, world: &W) {
        let Some(pr) = self.predicted.as_mut() else {
            return;
        };
        let db = world.ghosts.get(&index).and_then(|g| g.datablock());
        let keep = pr
            .vehicle
            .as_ref()
            .is_some_and(|d| d.index == index && Some(d.datablock) == db);
        if !keep {
            let mut work = 0u64;
            pr.vehicle = drive::DrivenVehicle::new(
                world,
                index,
                &mut self.shapes,
                self.static_world.reader.as_ref(),
                &mut work,
                TICK_WORK,
                &*self.static_world.game.0,
            );
            // the ticks its ghost had left to run its last move: the driven vehicle is the same object on TGE's
            // client, and no tick with a move counts them down.
            if let Some(d) = pr.vehicle.as_mut() {
                d.ghost_prediction = self
                    .ghost_vehicles
                    .get(&index)
                    .map_or(0, |g| g.warp.prediction);
            }
        }
        match pr.vehicle.as_mut() {
            Some(d) => d.read_packet_data(v, &mut ClientCollision::empty()),
            None => self.predicted = None,
        }
    }

    /// The vehicles the client runs on its own: each update the world received since the last tick read in order
    /// (`Vehicle::unpackUpdate`), then one tick of each (`processTick(NULL)`: the warp, else the last move) per move the client made
    /// since, each in the client container with the control Player where the client predicts it, then the frame.
    fn advance_ghosts(&mut self, end: u32, world: &W, vm_globals: &ClientGlobals) {
        let driven = self
            .predicted
            .as_ref()
            .and_then(|p| p.vehicle.as_ref())
            .map(|v| v.index);
        self.ghost_vehicles
            .retain(|i, _| world.ghosts.contains_key(i) && Some(*i) != driven);
        // One work budget for the packet's ghosts: their construction (the shapes read and made) and every tick, the containers'
        // construction charged; past it the ghosts wait (counted as a budget stop).
        let mut spent = 0u64;
        let updates: Vec<&ghost_world::world::VehicleUpdate<W::Set>> = world
            .vehicle_updates
            .iter()
            .filter(|u| u.seq > self.vehicle_seq)
            .collect();
        for u in updates {
            self.vehicle_seq = u.seq;
            let rec = u.record.vehicle().filter(|v| !v.flying);
            if Some(u.index) == driven {
                // an update with the vehicle's state (the server no longer counts this client its driver) is kept for the
                // resend that ends the drive; the next resend that keeps the drive supersedes it. One with a pose supersedes
                // the ones before it: a server that sends its driver the state all along would otherwise have the vehicle warp
                // back through the whole drive when it ends.
                if let Some(rec) = rec
                    && !rec.controlled
                {
                    if rec.pose.is_some() {
                        self.driven_pending.clear();
                    }
                    if self.driven_pending.len() < MAX_DRIVEN_PENDING {
                        self.driven_pending.push((rec, u.position));
                    }
                }
                continue;
            }
            let Some(rec) = rec else {
                continue;
            };
            if !self.ghost_vehicles.contains_key(&u.index) {
                if self.ghost_vehicles.len() >= MAX_GHOST_VEHICLES {
                    continue;
                }
                if spent >= TICK_WORK {
                    self.stats.budget_stops += 1;
                    continue;
                }
                let mut work = GHOST_WORK;
                let made = drive::GhostVehicle::new(
                    world,
                    u.index,
                    &mut self.shapes,
                    self.static_world.reader.as_ref(),
                    &mut work,
                    TICK_WORK - spent,
                    &*self.static_world.game.0,
                );
                spent = spent.saturating_add(work);
                if let Some(g) = made {
                    self.ghost_vehicles.insert(u.index, g);
                }
                continue;
            }
            if let Some(g) = self.ghost_vehicles.get_mut(&u.index) {
                g.unpack(&rec, u.position, true, &mut ClientCollision::empty());
            }
        }
        let ticks = match self.last_end {
            Some(e) => end.wrapping_sub(e).min(MAX_GHOST_TICKS),
            None => 0,
        };
        self.last_end = Some(end);
        // The game's projectile ghosts, each tick of each in the client's container with the control Player where the client
        // predicts it, from the shared work `spent`.
        let player_at = self
            .predicted
            .as_ref()
            .map(|p| (p.index, p.player.s.position()));
        if let Some(p) = self.projectiles.as_mut() {
            let (static_world, shapes, ghosts) = (
                &mut self.static_world,
                &mut self.shapes,
                &self.ghost_vehicles,
            );
            let mut container = |near: F3, budget: u64| {
                ClientCollision::new(
                    world,
                    u32::MAX,
                    budget,
                    Some(Ctx {
                        near,
                        static_world: &mut *static_world,
                        shapes: &mut *shapes,
                        driven: None,
                        ghosts: Some(ghosts),
                        disabled: None,
                        player_at,
                    }),
                )
            };
            p.advance(
                ticks,
                world,
                &mut container,
                &mut spent,
                &mut self.stats.budget_stops,
            );
        }
        if self.ghost_vehicles.is_empty() {
            return;
        }
        let player_at = self
            .predicted
            .as_ref()
            .map(|p| (p.index, p.player.s.position()));
        let indices: Vec<u32> = self.ghost_vehicles.keys().copied().collect();
        'ticks: for _ in 0..ticks {
            for &i in &indices {
                // A ghost that neither warps nor runs a move any more changes nothing the client reads.
                if self
                    .ghost_vehicles
                    .get(&i)
                    .is_none_or(|g| g.warp.count >= g.warp.ticks && g.warp.prediction < 1)
                {
                    continue;
                }
                if spent >= TICK_WORK {
                    self.stats.budget_stops += 1;
                    break 'ticks;
                }
                let Some(mut g) = self.ghost_vehicles.remove(&i) else {
                    continue;
                };
                let near = g.body.v.s.position();
                let mut collision = ClientCollision::new(
                    world,
                    u32::MAX,
                    TICK_WORK - spent,
                    Some(Ctx {
                        near,
                        static_world: &mut self.static_world,
                        shapes: &mut self.shapes,
                        driven: None,
                        ghosts: Some(&self.ghost_vehicles),
                        disabled: Some(i),
                        player_at,
                    }),
                );
                // The mover itself, at its own pose (its collision off).
                collision.set_view(drive::ghost_id(i), g.body.v.s.object_view());
                g.tick(&mut collision, vm_globals.sim_time);
                spent = spent.saturating_add(collision.work());
                self.ghost_vehicles.insert(i, g);
            }
        }
        if ticks > 0 {
            for g in self.ghost_vehicles.values_mut() {
                g.frame(FRAME_DT);
            }
        }
    }

    /// The client's tick: every move from `mLastClientMove` to the end of the list run by the control object.
    pub fn process(&mut self, moves: &MoveQueue, world: &W, vm_globals: &ClientGlobals) {
        let first = moves.first_index();
        let list = moves.moves();
        let end = first.wrapping_add(list.len() as u32);
        self.sim_ms = vm_globals.sim_time;
        // The control object ticks first, then the vehicles the client runs on its own: the client's process list runs in creation
        // order (nothing reorders it here), and the control Player's ghost comes before a vehicle spawned after it.
        self.process_control(moves, world, vm_globals);
        self.advance_ghosts(end, world, vm_globals);
    }

    /// [`Predictor::mount_pred`] after the moves run for the first time this tick (`fresh`: each one's turn of the object the
    /// rider controls).
    fn step_mount_pred(&self, world: &W, fresh: &[[f32; 2]]) -> Option<MountPred> {
        let pr = self.predicted.as_ref()?;
        let s = &pr.player.s;
        let m = s.mount.filter(|m| s.control_object == Some(*m))?;
        if pr.vehicle.is_some() || !controls_player(world, Some(m)) {
            return None;
        }
        let index = m.0.checked_sub(1)?;
        let g = world.ghosts.get(&index)?;
        let Some(Pose::Euler {
            yaw, pitch: head, ..
        }) = g.pose()
        else {
            return None;
        };
        let db = player_data(world, g.datablock()?, &**self.game())?;
        let Some(p) = self.mount_pred.as_ref().filter(|p| p.index == index) else {
            return Some(MountPred {
                index,
                yaw,
                pitch: head,
                still: 0,
                data: db,
            });
        };
        if s.damage_state != 0 {
            return Some(MountPred {
                still: 0,
                data: db,
                ..p.clone()
            });
        }
        let turned = fresh.iter().any(|y| *y != [0.0; 2]);
        let mut now = p.clone();
        for y in fresh {
            now.yaw = add_f32(now.yaw, y[0]);
            now.pitch = aim::mount_pitch(now.pitch, y[1], &db);
        }
        now.still = if turned {
            0
        } else {
            p.still.saturating_add(fresh.len() as u32)
        };
        // The ghost's angles are quantised (1/127 turn) and lag the moves: once the turning has stopped for a while they must agree
        // within a step; if they do not (the server turned the object itself) the ghost's are taken.
        if now.still >= MOUNT_SETTLE
            && (sim_turn(yaw - now.yaw).abs() > MOUNT_QUANTUM
                || (head - now.pitch).abs() > MOUNT_QUANTUM)
        {
            now.yaw = yaw;
            now.pitch = head;
        }
        now.data = db;
        Some(now)
    }

    fn process_control(&mut self, moves: &MoveQueue, world: &W, vm_globals: &ClientGlobals) {
        let first = moves.first_index();
        let list = moves.moves();
        let end = first.wrapping_add(list.len() as u32);
        if let Some(cam) = self.camera.as_mut() {
            let start = self.last_client_move.wrapping_sub(first);
            let start = if (start as i32) < 0 {
                0
            } else {
                (start as usize).min(list.len())
            };
            let skip = list.len().saturating_sub(start).saturating_sub(MAX_REPLAY);
            cam.begin(world);
            let near = cam.position();
            let mut collision = ClientCollision::new(
                world,
                cam.index(),
                TICK_WORK,
                Some(Ctx {
                    near,
                    static_world: &mut self.static_world,
                    shapes: &mut self.shapes,
                    driven: None,
                    ghosts: Some(&self.ghost_vehicles),
                    disabled: None,
                    player_at: None,
                }),
            );
            for m in list.iter().skip(start + skip) {
                if collision.exhausted() {
                    self.stats.budget_stops += 1;
                    break;
                }
                cam.step(m, world, &mut collision);
                self.stats.steps += 1;
            }
            self.last_client_move = end;
            return;
        }
        let game = Rc::clone(self.game());
        let Some(pr) = self.predicted.as_mut() else {
            self.last_client_move = end;
            return;
        };
        let controls_player = controls_player(world, pr.player.s.control_object);
        let start = self.last_client_move.wrapping_sub(first);
        let start = if (start as i32) < 0 {
            0
        } else {
            (start as usize).min(list.len())
        };
        let skip = list.len().saturating_sub(start).saturating_sub(MAX_REPLAY);
        // `ShapeBase::unpackUpdate` sets the control object's damage state as its ghost's record carries it (the same object on
        // TGE's client; a disabled Player runs no move of its own).
        if let Some(state) = damage_state(world, pr.index) {
            pr.player.s.damage_state = state;
        }
        if pr.player.s.mount == Some(PENDING_MOUNT)
            && let Some(m) = ghost_mount(world, pr.index)
        {
            pr.player.s.mount = Some(m);
        }
        pr.player.s.client = Some(sim::player::ClientView {
            freelook_value: self.camera_pos,
        });
        game.before_step(&mut pr.player.s, &vm_globals.prefs);
        // One world and one budget for the whole tick: the construction is charged, and every step draws on what is left.
        let near = match &pr.vehicle {
            Some(v) if pr.player.s.control_object.is_some() => v.v.s.position(),
            _ => pr.player.s.position(),
        };
        if let Some(v) = pr.vehicle.as_mut() {
            v.refresh_wheels(world);
        }
        let mut collision = ClientCollision::new(
            world,
            pr.index,
            TICK_WORK,
            Some(Ctx {
                near,
                static_world: &mut self.static_world,
                shapes: &mut self.shapes,
                driven: pr.vehicle.as_ref(),
                ghosts: Some(&self.ghost_vehicles),
                disabled: None,
                player_at: None,
            }),
        );
        let mut fresh = Vec::new();
        for (k, m) in list.iter().enumerate().skip(start + skip) {
            if collision.exhausted() {
                self.stats.budget_stops += 1;
                break;
            }
            let before = (pr.player.clone(), pr.vehicle.clone());
            let mut host = ClientPlayerHost {
                world: &mut collision,
                skeleton: NoSkeleton,
                statics: &mut self.statics,
                sim_time: vm_globals.sim_time,
                vehicle: pr.vehicle.as_mut(),
                controls_player,
                class: pr.kind.class(),
                game: Rc::clone(&game),
            };
            pr.player.s.mask_bits = 0;
            let stepped = pr
                .player
                .process_tick(&pr.data, Some(&sim_move(m, &*game)), &mut host);
            if stepped.is_err() || collision.exhausted() {
                // A step the budget cut short is not kept: the state of the last whole step stays.
                (pr.player, pr.vehicle) = before;
                self.stats.budget_stops += 1;
                break;
            }
            self.stats.steps += 1;
            let id = first.wrapping_add(k as u32);
            // A jump on the client: the control object of the connection to the server plays the set's jump sound
            // ([`EngineSet::jump_sound`]) at its render transform. Only a move run for the first time sounds: a replay after a
            // correction runs the same jump again silently.
            let new = self
                .newest_move
                .is_none_or(|n| (id.wrapping_sub(n) as i32) > 0);
            if new {
                self.newest_move = Some(id);
                fresh.push(game.control_turn(&sim_move(m, &*game), self.camera_pos));
                if pr.player.s.mask_bits & sim::player::net_mask::JUMP != 0
                    && self.jumps.len() < MAX_JUMP_SOUNDS
                {
                    self.jumps.push(pr.player.s.position());
                }
            }
            self.history.retain(|(i, _)| *i != id);
            if self.history.len() >= HISTORY {
                self.history.pop_front();
            }
            if let Some(d) = full_packet_data::<W::Set>(pr) {
                self.history.push_back((id, d));
            }
        }
        self.last_client_move = end;
        self.mount_pred = self.step_mount_pred(world, &fresh);
    }

    /// The control object's checksum for the next packet (0: nothing predicted).
    pub fn checksum(&self) -> u32 {
        let bits = self.ghost_bits;
        if let Some(cam) = &self.camera {
            return cam.checksum(bits);
        }
        self.packet_data().map_or(0, |p| checksum(p.as_ref(), bits))
    }

    /// The predicted Player's `writePacketData` (with its driven vehicle's, when it drives one).
    pub fn packet_data(&self) -> Option<Box<dyn ControlData>> {
        full_packet_data::<W::Set>(self.predicted.as_ref()?)
    }

    /// The camera distance moved by the virtual time since it last moved, as `getControlCameraTransform` moves
    /// it at every call: `dt = (now - last) / 1000` (the last time takes `now`), then `camera_speed * dt` towards first or
    /// third person. TGE calls it from each frame's camera query, so the Tab change moves the camera a frame's worth a frame;
    /// the engine calls it from each frame of a window and from each tick (so a client without frames moves it a tick's worth a tick).
    /// The client globals a driver's camera reads.
    pub fn set_view_globals(&mut self, vm: &ts_vm::vm::Vm) {
        if !self.game_given
            && let Some(g) = crate::game::installed_game(vm)
        {
            self.set_game(game::prediction_for(g.prediction()));
        }
        self.free_look = vm
            .global("$mvFreeLook")
            .is_some_and(|v| ts_vm::fields::atob(&v));
        self.smooth_aim = self.game().smooth_aim(vm);
        self.prefs = ClientGlobals::of(vm).prefs;
    }

    /// A window's frame: the move globals the next move will be made of, for smooth aim to draw ahead by ([`aim`]); `None` with
    /// smooth aim off.
    pub fn set_aim_input(&mut self, vm: &ts_vm::vm::Vm) {
        self.aim_input = self.smooth_aim.then(|| aim::Waiting::of(vm));
    }

    /// The control Player's yaw and head as this frame draws them: with smooth aim and a frame's input, ahead of the latest tick by
    /// the next move ([`aim::frame`]); else TGE's, `back` between the last two ticks (`Player::interpolateTick`: the yaw
    /// less its last change, the render head [`render_head`]).
    fn view_angles(&self, pr: &Predicted, back: f32, controls_player: bool) -> (f32, [f32; 3]) {
        let s = &pr.player.s;
        if let Some(w) = self.aim_input.as_ref().filter(|_| self.smooth_aim) {
            let drive = pr.vehicle.as_ref().map(|v| {
                let id = drive::ghost_id(v.index);
                aim::Steer {
                    controls: s.control_object == Some(id),
                    mounted_on: s.mount == Some(id),
                    steering: v.v.s.steering,
                    max_angle: v.data.base.max_steering_angle,
                    strafe: v.steer.strafe,
                    strafe_rate: v.steer.strafe_rate,
                    auto_return: v.steer.auto_return,
                    return_rate: v.steer.return_rate,
                    return_speed: v.steer.return_speed,
                    client_return: self.game().client_auto_return(&v.v.s),
                }
            });
            let ctx = aim::Ctx {
                camera_pos: self.camera_pos,
                prefs: self.prefs,
                drive,
                controls_player,
            };
            return aim::frame(&**self.game(), s, &pr.data, w, back, &ctx);
        }
        if back <= 0.0 {
            return (s.rot[2], s.head);
        }
        let b = back.min(1.0);
        (s.rot[2] + s.delta_rot_vec[2] * b, render_head(s, back))
    }

    /// A frame at virtual time `now_ms` (the window's frame view): the wheels of the vehicles the client runs spun by the time
    /// since the last frame (`WheeledVehicle::advanceTime`), the flying warps drawn at `now_ms`.
    pub fn frame_vehicles(&mut self, now_ms: u32) {
        self.sim_ms = now_ms;
        let driven = self
            .predicted
            .as_ref()
            .and_then(|p| p.vehicle.as_ref().filter(|_| p.player.s.mount.is_some()));
        let ghosts = self.ghost_vehicles.values().map(|g| &g.body);
        self.spins
            .advance(now_ms, driven.into_iter().chain(ghosts.clone()));
        let networked = ghosts.map(|g| (g, true));
        self.steering.advance(
            now_ms,
            driven.map(|v| (v, false)).into_iter().chain(networked),
        );
    }

    /// Ghost `index`'s object view in the container the control Player's next tick collides with (`None`: not in it).
    pub fn collision_view(&mut self, world: &W, index: u32) -> Option<collide::ObjectView> {
        let pr = self.predicted.as_ref();
        let me = pr.map_or(u32::MAX, |p| p.index);
        let near = pr.map_or([0.0; 3], |p| p.player.s.position());
        let c = ClientCollision::new(
            world,
            me,
            TICK_WORK,
            Some(Ctx {
                near,
                static_world: &mut self.static_world,
                shapes: &mut self.shapes,
                driven: pr.and_then(|p| p.vehicle.as_ref()),
                ghosts: Some(&self.ghost_vehicles),
                disabled: None,
                player_at: None,
            }),
        );
        collide::World::object(&c, drive::ghost_id(index))
    }

    /// The first of `mask`'s objects the segment from `start` to `end` meets in the client's container, built about `start` (the
    /// game's static objects near it and the ghosts; the control Player left out): a game's own camera behind its Player kept in
    /// front of a wall. `None` when it meets nothing (or the container's work budget runs out).
    pub fn cast_ray(&mut self, world: &W, start: F3, end: F3, mask: u32) -> Option<RayHit> {
        let pr = self.predicted.as_ref();
        let me = pr.map_or(u32::MAX, |p| p.index);
        let mut c = ClientCollision::new(
            world,
            me,
            TICK_WORK,
            Some(Ctx {
                near: start,
                static_world: &mut self.static_world,
                shapes: &mut self.shapes,
                driven: pr.and_then(|p| p.vehicle.as_ref()),
                ghosts: Some(&self.ghost_vehicles),
                disabled: None,
                player_at: None,
            }),
        );
        c.cast_ray(start, end, mask)
    }

    pub fn camera_advance(&mut self, first_person: bool, speed: f32, now_ms: u32) {
        let last = self.camera_ms.replace(now_ms);
        let dt = last.map_or(0.0, |t| now_ms.wrapping_sub(t) as f32 / 1000.0);
        if dt > 0.0 {
            self.camera_step(first_person, speed, dt);
        }
    }

    /// The camera distance for a step of `dt` seconds (`getControlCameraTransform`).
    pub fn camera_step(&mut self, first_person: bool, speed: f32, dt: f32) {
        if first_person {
            if self.camera_pos > 0.0 {
                self.camera_pos -= speed * dt;
                if self.camera_pos <= 0.0 {
                    self.camera_pos = 0.0;
                }
            }
        } else if self.camera_pos < 1.0 {
            self.camera_pos += speed * dt;
            if self.camera_pos > 1.0 {
                self.camera_pos = 1.0;
            }
        }
    }

    /// The driver's view. In the first person the Player's render eye on its seat (`Player::getRenderEyeTransform`): the vehicle's
    /// render transform turned by the head (`Rz(head.z) * Rx(head.x)`), at the vehicle's mount node of the driver's seat plus the
    /// Player's eye node (each scaled by its object's scale). In the third person the vehicle's camera
    /// ([`game::PredictGame::vehicle_eye`]: TGE's `Vehicle::getCameraTransform` by default). Between the two, while Tab moves the
    /// camera distance, the two eased ([`tab_blend`]). The source is picked each frame, so a mount or a dismount during the move
    /// carries the distance on to the new one. None when the Player drives nothing or the vehicle has no camera (its datablock
    /// without a `cameraMaxDist`, or its shape not there).
    fn driver_view(&mut self, world: &W, back: f32, rh: [f32; 3]) -> Option<(F3, f32, f32)> {
        let pr = self.predicted.as_ref().filter(|_| self.camera.is_none())?;
        let v = pr.vehicle.as_ref()?;
        let s = &pr.player.s;
        s.mount?;
        datablock_f32(world, v.datablock, "cameraMaxDist", 0)?;
        let pose = vehicle_render_pose(&v.v.s, back);
        let Pose::Quat { position, rotation } = pose else {
            return None;
        };
        let mut render = collide::math::quat_matrix(rotation);
        render[3] = position[0];
        render[7] = position[1];
        render[11] = position[2];
        // the head as the frame draws it (`Player::interpolateTick` sets the render head `head + headVec * dt`
        // whether or not the Player is mounted; `getCameraTransform` reads it), so a driver's free look moves
        // every frame and not once a tick; with smooth aim the head the next move makes (`rh`, [`Self::view_angles`]).
        let head = [rh[0], rh[2]];
        // First person: the seat's mount node plus the Player's eye node (also the start and end of a Tab move).
        let seat = s.mount_node;
        let inst = shapes::instance::ShapeInstance::new(&v.shape.shape, &mut |_| true).ok();
        let node = |name: &str| {
            let inst = inst.as_ref()?;
            let n = v.shape.shape.find_name(name.as_bytes())?;
            let node = v.shape.shape.nodes.iter().position(|x| x.name == n)?;
            inst.node_transform(node)
        };
        let mount = node(&format!("mount{seat}"))
            .map(|m| [m[3], m[7], m[11]])
            .unwrap_or([0.0; 3]);
        // the eye node of the Player's pose as the frame has it (`getEyeTransform` reads the node from the shape instance's
        // last animate): seated, a mount's sitting thread (if the scripts play one) lowers the eye; the record's pose without
        // its script threads gives the standing eye.
        let ghost = world.ghosts.get(&pr.index);
        let eye_off = ghost
            .and_then(|g| {
                crate::eye::live_eye_offset(
                    world,
                    pr.index,
                    g.datablock()?,
                    g.record.as_ref(),
                    head[0],
                )
            })
            .unwrap_or([0.0, 0.0, PLAYER_EYE_Z]);
        let local = [0, 1, 2].map(|k| mount[k] * v.v.s.scale[k] + eye_off[k] * s.scale[k]);
        let eye = collide::math::mul_p(&render, local);
        let turn = collide::math::matrix_product(
            &collide::math::set_euler([0.0, 0.0, head[1]]),
            &collide::math::set_euler([head[0], 0.0, 0.0]),
        );
        let view = collide::math::matrix_product(&render, &turn);
        let (pitch, yaw) = torque_game::scene::camera::rotation_of(&view);
        let first = (eye, pitch, yaw);
        if self.camera_pos <= 0.0 {
            return Some(first);
        }
        let wb = collide::math::mul_box(&render, &v.shape.bounds());
        let centre = [0, 1, 2].map(|k| (wb.min[k] + wb.max[k]) * 0.5);
        let view = VehicleView {
            render,
            centre,
            head,
            free_look: self.free_look,
            prefs: self.prefs,
            camera_pos: self.camera_pos,
            eye: node("eye").map_or(render, |n| collide::math::matrix_product(&render, n)),
            cam: node("cam").map(|n| collide::math::mul_p(&render, [n[3], n[7], n[11]])),
        };
        let game = Rc::clone(self.game());
        let (index, db, disabled) = (pr.index, v.datablock, v.index);
        let (static_world, shapes, ghosts) = (
            &mut self.static_world,
            &mut self.shapes,
            &self.ghost_vehicles,
        );
        let mut collision = || {
            ClientCollision::new(
                world,
                index,
                TICK_WORK,
                Some(Ctx {
                    near: position,
                    static_world: &mut *static_world,
                    shapes: &mut *shapes,
                    driven: None,
                    ghosts: Some(ghosts),
                    disabled: Some(disabled),
                    player_at: None,
                }),
            )
        };
        let see_through = |c: &ClientCollision, o: ObjectId| game.see_through(world, c, o);
        let cast = CameraCast {
            mask: game.camera_masks().vehicle,
            see_through: &see_through,
        };
        let Some(third) = game.vehicle_eye(world, db, &view, &mut collision, &cast) else {
            return Some(first);
        };
        // the Tab move between the seat and the vehicle camera (both settled ends are TGE's).
        Some(tab_blend(
            first,
            (third.eye, third.pitch, third.yaw),
            third.start,
            self.camera_pos,
        ))
    }

    /// The client's view from its control object: the predicted Player's eye when there is one, else the last state the server
    /// sent (the world's `view`; a Camera's view is the world's own).
    pub fn update_view(&mut self, world: &mut W) {
        // the jumps the new moves made, sounded once each.
        let jumps = std::mem::take(&mut self.jumps);
        if let Some(profile) = world
            .control
            .object
            .and_then(|i| world.ghosts.get(&i))
            .and_then(|g| g.datablock())
            .and_then(|db| world.jump_sound(db))
        {
            for position in jumps {
                world.push_effect(Effect::Sound3D {
                    profile,
                    position,
                    cone: None,
                });
            }
        }
        let back = self.back;
        // the game's projectiles between their ticks and the explosions their own steps made.
        world.projectile_render.clear();
        if let Some(p) = self.projectiles.as_mut() {
            p.frame(world, back);
        }
        // `Player::interpolateTick`: the render position, yaw and head between the last tick's (`dt` 0) and the one
        // before (`dt` 1: the position before the tick's move, the yaw and head less their last change). A mounted Player's
        // position comes from its mount (`setRenderPosition`'s mounted branch); its yaw and head are interpolated as
        // any Player's (the driver's free look moved once a tick).
        // Smooth aim: the yaw and head drawn ahead of the latest tick by what the next move makes of the frame's move
        // globals, in every mode (on foot, free look, a seat, a driver's), so the view never steps back when that move lands.
        let angles = self.predicted.as_ref().map_or((0.0, [0.0; 3]), |pr| {
            self.view_angles(pr, back, controls_player(world, pr.player.s.control_object))
        });
        let lerp = |s: &sim::player::PlayerState| -> ([f32; 3], f32, f32) {
            let p = s.position();
            let (yaw, head) = (angles.0, angles.1[0]);
            if back <= 0.0 || s.mount.is_some() {
                return (p, yaw, head);
            }
            let b = back.min(1.0);
            (
                [0, 1, 2].map(|k| p[k] + (s.delta_pos[k] - p[k]) * b),
                yaw,
                head,
            )
        };
        world.hide_control =
            self.camera.is_none() && self.predicted.is_some() && self.camera_pos == 0.0;
        world.control_eye = None;
        // The control Player is drawn where it is predicted; a mounted one (a driver) is drawn at its mount's node, which the
        // renderer takes from the mount (`setRenderPosition`'s mounted branch), turned by this pose's yaw.
        world.control_render = self
            .predicted
            .as_ref()
            .filter(|_| self.camera.is_none())
            .map(|pr| {
                let (position, yaw, pitch) = lerp(&pr.player.s);
                Pose::Euler {
                    position,
                    pitch,
                    yaw,
                }
            });
        // The vehicle the Player drives, where the client predicts it, between its last two ticks
        // (`WheeledVehicle::interpolateTick`: the rotation `interpolate(rot[1], rot[0], dt)`, the position
        // `delta.pos + delta.posVec * dt`); the server's updates of it carry no position for its controlling client.
        world.vehicle_render.clear();
        world.vehicle_look.clear();
        if self.camera.is_none()
            && let Some(pr) = &self.predicted
            && let Some(v) = &pr.vehicle
            && pr.player.s.mount.is_some()
        {
            let pose = vehicle_render_pose(&v.v.s, back);
            world.vehicle_render.insert(v.index, pose);
            let springs = self.springs.drawn(v, back);
            world.vehicle_look.insert(
                v.index,
                look::vehicle_look(
                    v,
                    self.spins.spins.get(&v.index),
                    self.steering.steering(v.index),
                    Some(&springs),
                    &pose,
                ),
            );
        }
        // the vehicles the client runs on its own drawn between their ticks as the driven one is (TGE interpolates every
        // vehicle ghost each frame, `WheeledVehicle::interpolateTick`), not where their last update put them, in tick steps.
        // Their wheels as the client runs them.
        for (i, g) in &mut self.ghost_vehicles {
            if world.vehicle_render.contains_key(i) || !world.ghosts.contains_key(i) {
                continue;
            }
            // `interpolateTick(dt)` keeps the fraction drawn: the next update's warp starts from where this frame drew it.
            g.frame(back);
            let pose = vehicle_render_pose(&g.body.v.s, back);
            world.vehicle_render.insert(*i, pose);
            let springs = self.springs.drawn(&g.body, back);
            world.vehicle_look.insert(
                *i,
                look::vehicle_look(
                    &g.body,
                    self.spins.spins.get(i),
                    self.steering.steering(*i),
                    Some(&springs),
                    &pose,
                ),
            );
        }
        let looked = &world.vehicle_look;
        self.springs.retain(|i| looked.contains_key(&i));
        // The FlyingVehicle ghosts (not run by the engine's client): warped between their updates ([`look::FlyingWarps`]).
        self.flying.update(world, self.sim_ms);
        let flying: Vec<_> = self.flying.poses(self.sim_ms).collect();
        for (i, pose) in flying {
            world.vehicle_render.entry(i).or_insert(pose);
        }
        // The control Player's own jetting and crouching (its ghost's updates carry neither for its own client).
        let control_state = self
            .predicted
            .as_ref()
            .filter(|_| self.camera.is_none())
            .map(|pr| {
                let s = &pr.player.s;
                let on_ground = s.working_nonempty || s.contact_timer < 2;
                let water_root = self.game().water_root(s, on_ground);
                game::ControlState {
                    crouched: pr.kind.crouched(s),
                    pose: s.pose,
                    jetting: pr.kind.jetting(s),
                    falling: s.falling,
                    on_ground,
                    water_root,
                    velocity: s.velocity,
                }
            });
        world.set_control_state(control_state);
        world.ghost_render.clear();
        // Smooth aim: the Player a rider controls (a mount, a turret) is turned by the rider's move, and the rider's view
        // rides on its yaw: drawn ahead of its last update by the turn the next move gives it.
        if self.camera.is_none()
            && let Some(w) = self.aim_input.as_ref().filter(|_| self.smooth_aim)
            && let Some(pr) = &self.predicted
            && let s = &pr.player.s
            && let Some(m) = s.mount.filter(|m| s.control_object == Some(*m))
            && pr.vehicle.is_none()
            && controls_player(world, Some(m))
            && let Some(index) = m.0.checked_sub(1)
            && let Some(Pose::Euler { position, .. }) =
                world.ghosts.get(&index).and_then(|g| g.pose())
        {
            let angles = self
                .mount_pred
                .as_ref()
                .filter(|p| p.index == index)
                .map(|p| {
                    let [y, pt] = aim::mount_frame(
                        &**self.game(),
                        [p.yaw, p.pitch],
                        &p.data,
                        s.damage_state != 0,
                        w,
                        back,
                        self.camera_pos,
                    );
                    (y, pt)
                });
            if let Some((yaw, pitch)) = angles {
                world.ghost_render.insert(
                    index,
                    Pose::Euler {
                        position,
                        pitch,
                        yaw,
                    },
                );
            }
        }
        // The other Players where the client runs them, at the frame's time.
        let now = world.anim_ms;
        self.ghost_players.frame(world, now);
        if let Some(cam) = &self.camera {
            // `Camera::interpolateTick`: the view between the last two ticks, an orbit made from its target's render
            // position at this frame (the corpse drawn there too) and pulled in through the container. Smooth aim: the last
            // tick's pitch and yaw ahead by what the next move makes of them (the mouse's waiting turn
            // and the keys' turn, the pitch held as the move will hold it); an orbit's eye follows that turn.
            let aim = self.aim_input.as_ref().filter(|_| self.smooth_aim);
            let near = cam.position();
            let mut collision = ClientCollision::new(
                world,
                cam.index(),
                TICK_WORK,
                Some(Ctx {
                    near,
                    static_world: &mut self.static_world,
                    shapes: &mut self.shapes,
                    driven: None,
                    ghosts: Some(&self.ghost_vehicles),
                    disabled: None,
                    player_at: None,
                }),
            );
            let (position, [pitch, yaw]) = cam.frame(back, aim, world, &mut collision);
            if let Some((watched, at)) = cam.watched_render(back)
                && let Some(Pose::Euler { pitch, yaw, .. }) =
                    world.ghosts.get(&watched).and_then(|g| g.pose())
            {
                world.ghost_render.insert(
                    watched,
                    Pose::Euler {
                        position: at,
                        pitch,
                        yaw,
                    },
                );
            }
            world.view = Some(Eye {
                position,
                pitch,
                yaw,
                fov: world.control.server_fov.or(world.control.client_fov),
            });
            return;
        }
        // A driver looks through its vehicle ([`Self::driver_view`]).
        if let Some((position, pitch, yaw)) = self.driver_view(world, back, angles.1) {
            world.control_eye = Some(position);
            world.view = Some(Eye {
                position,
                pitch,
                yaw,
                fov: world.control.server_fov.or(world.control.client_fov),
            });
            return;
        }
        if let Some(pr) = &self.predicted {
            let s = &pr.player.s;
            let (lpos, lyaw, lhead) = lerp(s);
            // both the eye transform and the third-person camera turn the body's render transform by the
            // head, `Rz(head.z) * Rx(head.x)`: free look turns the view by the render head's side angle.
            let head_z = angles.1[2];
            let driver = pr.vehicle.is_some() && s.mount.is_some();
            // a Player on a seat it does not drive (a passenger) is drawn at its mount's `mount<N>` node turned by its own yaw
            // (`setRenderPosition`), and the view is from there (not the predicted Player's own position, the world's origin
            // for a mounted one).
            let seat = if s.mount.is_some() && pr.vehicle.is_none() {
                crate::eye::mount_render(world, pr.index)
            } else {
                None
            };
            let (body_pos, body_yaw) = match &seat {
                Some(m) => (
                    [m[3], m[7], m[11]],
                    torque_game::scene::camera::rotation_of(m).1,
                ),
                None => (lpos, lyaw),
            };
            let (position, pitch, yaw) = if driver {
                // A driver's eye rides with its vehicle (when its vehicle's camera cannot be made: no datablock or shape).
                let pos = pr.vehicle.as_ref().map_or(lpos, |v| v.v.s.position());
                (
                    [pos[0], pos[1], pos[2] + PLAYER_EYE_Z],
                    lhead,
                    lyaw + head_z,
                )
            } else if let Some(m) = &seat {
                let ghost = world.ghosts.get(&pr.index);
                let off = ghost
                    .and_then(|g| {
                        crate::eye::live_eye_offset(
                            world,
                            pr.index,
                            g.datablock()?,
                            g.record.as_ref(),
                            lhead,
                        )
                    })
                    .unwrap_or([0.0, 0.0, PLAYER_EYE_Z]);
                let eye = collide::math::mul_p(m, [0, 1, 2].map(|k| off[k] * s.scale[k]));
                let turn = collide::math::matrix_product(
                    &collide::math::set_euler([0.0, 0.0, head_z]),
                    &collide::math::set_euler([lhead, 0.0, 0.0]),
                );
                let (p, y) = torque_game::scene::camera::rotation_of(
                    &collide::math::matrix_product(m, &turn),
                );
                (eye, p, y)
            } else {
                (
                    control_eye(world, lpos, lyaw, lhead, s.scale),
                    lhead,
                    lyaw + head_z,
                )
            };
            world.control_eye = Some(position);
            let mut eye = (position, pitch);
            let mut view_yaw = yaw;
            // Third person: the game's camera for a camera distance above 0 ([`game::PredictGame::third_person_eye`]).
            if self.camera_pos > 0.0 && !driver {
                let (pos, yaw, head, scale, index) = (body_pos, body_yaw, lhead, s.scale, pr.index);
                let db = world
                    .control
                    .object
                    .and_then(|i| world.ghosts.get(&i))
                    .and_then(|g| g.datablock());
                if let Some(db) = db {
                    let game = Rc::clone(self.game());
                    let (static_world, shapes, ghosts) = (
                        &mut self.static_world,
                        &mut self.shapes,
                        &self.ghost_vehicles,
                    );
                    let mut collision = || {
                        ClientCollision::new(
                            world,
                            index,
                            TICK_WORK,
                            Some(Ctx {
                                near: pos,
                                static_world: &mut *static_world,
                                shapes: &mut *shapes,
                                driven: None,
                                ghosts: Some(ghosts),
                                disabled: None,
                                player_at: None,
                            }),
                        )
                    };
                    let view = ThirdPerson {
                        pos,
                        yaw,
                        head,
                        head_z,
                        scale,
                        camera_pos: self.camera_pos,
                        eye: position,
                    };
                    let see_through =
                        |c: &ClientCollision, o: ObjectId| game.see_through(world, c, o);
                    let cast = CameraCast {
                        mask: game.camera_masks().third_person,
                        see_through: &see_through,
                    };
                    if let Some(third) =
                        game.third_person_eye(world, db, &view, &mut collision, &cast)
                    {
                        eye = third;
                        view_yaw = yaw + head_z;
                    }
                }
            }
            world.view = Some(Eye {
                position: eye.0,
                pitch: eye.1,
                yaw: view_yaw,
                fov: world.control.server_fov.or(world.control.client_fov),
            });
            return;
        }
        if let Some(p) = world.control.tail.as_ref().and_then(EngineTail::player) {
            let pos = world.control.position.unwrap_or(p.translation);
            let scale = world
                .control
                .object
                .and_then(|i| world.ghosts.get(&i))
                .and_then(|g| g.scale())
                .unwrap_or([1.0; 3]);
            let position = control_eye(world, pos, p.rot_z, p.head_x, scale);
            world.view = Some(Eye {
                position,
                pitch: p.head_x,
                // turned by the head as the eye transform turns it.
                yaw: p.rot_z + p.head_z,
                fov: world.control.server_fov.or(world.control.client_fov),
            });
        } else {
            // A Camera control object (or none yet): the world's own eye.
            world.view = None;
        }
    }
}

/// A Player's render head `back` between its last tick (0) and the one before (1): `delta.head + delta.headVec * dt`
/// (`Player::interpolateTick`), mounted or not.
pub fn render_head(s: &sim::player::PlayerState, back: f32) -> [f32; 3] {
    if back <= 0.0 {
        return s.head;
    }
    let b = back.min(1.0);
    [0, 1, 2].map(|k| s.head[k] + s.delta_head_vec[k] * b)
}

/// A vehicle's render pose `back` between its last tick (0) and the one before (1), as `WheeledVehicle::interpolateTick`
/// makes it: at 0 the last tick's rotation and position, else `QuatF::interpolate(rot[1], rot[0], back)` and
/// `delta.pos + delta.posVec * back`.
pub fn vehicle_render_pose(s: &vehicle::state::VehicleState, back: f32) -> Pose {
    let b = back.clamp(0.0, 1.0);
    let (position, rotation) = if b == 0.0 {
        (s.delta_pos, s.delta_rot[1])
    } else {
        (
            [0, 1, 2].map(|k| s.delta_pos_vec[k] * b + s.delta_pos[k]),
            drive::quat_slerp(s.delta_rot[1], s.delta_rot[0], b),
        )
    };
    Pose::Quat { position, rotation }
}

/// A predicted Player's `writePacketData` as set `S`'s packet data: its own words, then its control object's (a driven
/// vehicle's) when it has one.
fn full_packet_data<S: EngineSet>(pr: &Predicted) -> Option<Box<dyn ControlData>> {
    let words = player_words(&pr.player.s, &*pr.kind);
    let controlled = match &pr.vehicle {
        Some(v) if pr.player.s.control_object.is_some() => {
            Some((v.index, v.packet_data::<S>(&mut ClientCollision::empty())))
        }
        _ => None,
    };
    Some(S::player_data(&words, controlled))
}

/// A Player's `writePacketData` words from its state, `kind` its class.
pub fn player_words(s: &PlayerState, kind: &dyn PlayerKind) -> PlayerWords {
    let pos = s.position();
    PlayerWords {
        energy: s.energy,
        flag: kind.jetting(s),
        free: s.mount.is_none().then_some(FreeMotion {
            position: pos,
            velocity: s.velocity,
            contact: s.jump_surface_last,
        }),
        head_x: s.head[0],
        head_z: s.head[2],
        rot_z: s.rot[2],
        translation: pos,
    }
}

/// The eye height over a Player's position when its shape has no `eye` node the client can pose ([`crate::eye`]).
pub const PLAYER_EYE_Z: f32 = 2.0;

/// The control object's eye (`Player::getEyeTransform`, [`crate::eye`]): the eye node of its datablock's shape at the head
/// pitch, through the object's transform at `pos` and `yaw`; [`PLAYER_EYE_Z`] over `pos` without one.
fn control_eye<W: PredictWorld>(
    world: &W,
    pos: [f32; 3],
    yaw: f32,
    pitch: f32,
    scale: [f32; 3],
) -> [f32; 3] {
    let ghost = world.control.object.and_then(|i| world.ghosts.get(&i));
    // The eye node of the client's own Player as the live pose has it (its predicted crouch, played over the crouch
    // sequence, and the action's transitions).
    let offset = ghost.and_then(|g| {
        crate::eye::live_eye_offset(
            world,
            world.control.object?,
            g.datablock()?,
            g.record.as_ref(),
            pitch,
        )
    });
    match offset {
        Some(o) => crate::eye::eye_point(pos, yaw, scale, o),
        None => [pos[0], pos[1], pos[2] + PLAYER_EYE_Z],
    }
}

/// A datablock's float field as its record carries it (a `W32` word holds the float's bits, an `Fd` field its value).
pub fn datablock_f32<W: PredictWorld>(world: &W, db: u32, name: &str, index: u32) -> Option<f32> {
    match world.datablocks.get(&db)?.value(name, index)? {
        Value::F32(v) => Some(*v),
        Value::Int(bits) => Some(f32::from_bits(*bits as u32)),
        _ => None,
    }
}

/// A datablock's bool field as its record carries it.
fn datablock_bool<W: PredictWorld>(world: &W, db: u32, name: &str) -> Option<bool> {
    match world.datablocks.get(&db)?.value(name, 0)? {
        Value::Bool(v) => Some(*v),
        Value::Int(v) => Some(*v != 0),
        _ => None,
    }
}

/// What the third-person camera is made from: the Player's position, yaw, head pitch and side angle (free look), scale, the camera
/// distance (0..=1) and its first-person eye.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThirdPerson {
    pub pos: F3,
    pub yaw: f32,
    pub head: f32,
    pub head_z: f32,
    pub scale: F3,
    pub camera_pos: f32,
    pub eye: F3,
}

/// TGE's third-person camera, `ShapeBase::getCameraTransform` for a camera distance above 0: the eye's position and pitch. The
/// camera looks along the eye (pitch `head`, yaw `yaw + head.z`); its point goes back from the start along the eye's forward axis
/// by `(cameraMaxDist - cameraMinDist) * pos`, and a cast there (`cast`) that meets something stops it short by
/// `-dot(vec, n) / |vec| * 0.1` of the way (at the eye when nothing is left). `None` without a `cameraMaxDist`. Engine limit: the
/// shape's `cam` node is not posed, so the cast starts at the eye (TGE starts at the `cam` node, or the render position without
/// one).
pub fn tge_third_person_eye<W: PredictWorld>(
    world: &W,
    db: u32,
    v: &ThirdPerson,
    collision: &mut dyn FnMut() -> ClientCollision,
    cast: &CameraCast<'_>,
) -> Option<(F3, f32)> {
    let max = datablock_f32(world, db, "cameraMaxDist", 0)?;
    let min = datablock_f32(world, db, "cameraMinDist", 0).unwrap_or(0.0);
    let view = torque_game::scene::camera::pose_matrix([0.0; 3], v.head, v.yaw + v.head_z);
    let forward = [view[1], view[5], view[9]];
    let back = -((max - min) * v.camera_pos);
    let vec = forward.map(|f| f * back);
    let start = v.eye;
    let end = add3(start, vec);
    let mut c = collision();
    let eye = match solid_hit(&mut c, start, end, cast) {
        None => end,
        Some(hit) => {
            let len = len3(vec);
            let t = if len > 0.0 {
                len3(sub3(hit.point, start)) / len
            } else {
                0.0
            };
            let adj = if len > 0.0 {
                -(vec[0] * hit.normal[0] + vec[1] * hit.normal[1] + vec[2] * hit.normal[2]) / len
                    * 0.1
            } else {
                0.0
            };
            let at = (t - adj).max(0.0);
            if at == 0.0 {
                v.eye
            } else {
                add3(start, vec.map(|x| x * at))
            }
        }
    };
    Some((eye, v.head))
}

/// How a camera's cast meets the client's container: its mask and the objects it looks through (the game's answers:
/// [`game::PredictGame::camera_masks`], [`game::PredictGame::see_through`]).
pub struct CameraCast<'a> {
    pub mask: u32,
    pub see_through: &'a dyn Fn(&ClientCollision, ObjectId) -> bool,
}

/// The frame of a forward vector without roll (`Vehicle::getCameraTransform`'s non-roll path): x = `normalize(f x z)`, y = `f`,
/// z = `normalize(x x f)`.
pub fn unrolled(f: F3) -> Matrix {
    let f = normalize3(f);
    let x = normalize3([f[1], -f[0], 0.0]);
    let z = normalize3([
        x[1] * f[2] - x[2] * f[1],
        x[2] * f[0] - x[0] * f[2],
        x[0] * f[1] - x[1] * f[0],
    ]);
    let mut m = collide::math::IDENTITY;
    for k in 0..3 {
        m[k * 4] = x[k];
        m[k * 4 + 1] = f[k];
        m[k * 4 + 2] = z[k];
    }
    m
}

/// What the vehicle camera is made from: the vehicle's render transform and world box centre, the driver's head (x, z), free look
/// (`$mvFreeLook`) and the game's preferences, the camera distance (0..=1), and the vehicle's render eye (its `eye` node through the
/// render transform, `ShapeBase::getRenderEyeTransform`) and `cam` node (in the world), as its shape's rest pose has them.
#[derive(Debug, Clone, Copy)]
pub struct VehicleView {
    pub render: Matrix,
    pub centre: F3,
    pub head: [f32; 2],
    pub free_look: bool,
    pub prefs: game::GamePrefs,
    pub camera_pos: f32,
    pub eye: Matrix,
    pub cam: Option<F3>,
}

/// A driver's third-person eye ([`game::PredictGame::vehicle_eye`]): the eye, its pitch and yaw, and where the camera is at
/// distance 0 (a Tab move eases from there, [`tab_blend`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VehicleEye {
    pub eye: F3,
    pub pitch: f32,
    pub yaw: f32,
    pub start: F3,
}

/// TGE's vehicle camera, `Vehicle::getCameraTransform` for a camera distance above 0. The camera looks along the vehicle's render
/// eye ([`VehicleView::eye`]), with `cameraRoll` as it is, else along its forward with z kept up; its point starts at the `cam`
/// node (the eye without one) lifted by `cameraOffset`, and goes back from there along the eye's forward axis by
/// `(cameraMaxDist - cameraMinDist) * pos`; a cast (`cast`) from the node that meets something stops it short by
/// `-dot(vec, n) / |vec| * 0.1` of the way (at the eye when nothing is left). `None` without a `cameraMaxDist`. Engine limits: the
/// nodes are the rest pose's, and the camera's lag (`cameraLag`, `cameraDecay`) is not run; the eye takes a yaw and a pitch, so a
/// rolled camera is drawn level.
pub fn tge_vehicle_eye<W: PredictWorld>(
    world: &W,
    db: u32,
    v: &VehicleView,
    collision: &mut dyn FnMut() -> ClientCollision,
    cast: &CameraCast<'_>,
) -> Option<VehicleEye> {
    let max = datablock_f32(world, db, "cameraMaxDist", 0)?;
    let min = datablock_f32(world, db, "cameraMinDist", 0).unwrap_or(0.0);
    let offset = datablock_f32(world, db, "cameraOffset", 0).unwrap_or(0.0);
    let roll = datablock_bool(world, db, "cameraRoll").unwrap_or(false);
    let m = v.eye;
    let forward = [m[1], m[5], m[9]];
    let frame = if roll { m } else { unrolled(forward) };
    let back = -((max - min) * v.camera_pos);
    let vec = forward.map(|f| f * back);
    let at = [m[3], m[7], m[11]];
    let from = v.cam.unwrap_or(at);
    let start = add3(from, [0.0, 0.0, offset]);
    let end = add3(start, vec);
    let mut c = collision();
    let eye = match solid_hit(&mut c, from, end, cast) {
        None => end,
        Some(hit) => {
            let (span, len) = (len3(sub3(end, from)), len3(vec));
            let t = if span > 0.0 {
                len3(sub3(hit.point, from)) / span
            } else {
                0.0
            };
            let adj = if len > 0.0 {
                -(vec[0] * hit.normal[0] + vec[1] * hit.normal[1] + vec[2] * hit.normal[2]) / len
                    * 0.1
            } else {
                0.0
            };
            let t = t - adj;
            if t > 0.0 {
                add3(start, vec.map(|x| x * t))
            } else {
                at
            }
        }
    };
    let (pitch, yaw) = torque_game::scene::camera::rotation_of(&frame);
    Some(VehicleEye {
        eye,
        pitch,
        yaw,
        start,
    })
}

/// A driver's view while Tab moves the camera distance between 0 and 1. TGE jumps there (`getCameraTransform` hands any distance
/// above 0 to the vehicle's camera, which starts away from the seat's eye, and back to the seat at the end of the way in). The
/// engine eases instead (an engine-only choice): the vehicle camera's eye at the distance, moved by `1 - pos` of the way from that
/// camera's start (`start`, its eye at distance 0 before the cast, [`VehicleEye::start`]) to the first-person eye,
/// so the eye leaves and comes back to the seat, and the angles from the first person's to the vehicle camera's by `pos`.
/// `first` and `third` are (eye, pitch, yaw); at 0 the first person, at 1 the vehicle camera, as TGE settles.
pub fn tab_blend(
    first: (F3, f32, f32),
    third: (F3, f32, f32),
    start: F3,
    pos: f32,
) -> (F3, f32, f32) {
    if pos <= 0.0 {
        return first;
    }
    if pos >= 1.0 {
        return third;
    }
    let k = 1.0 - pos;
    let eye = [0, 1, 2].map(|i| third.0[i] + (first.0[i] - start[i]) * k);
    let turn = |from: f32, to: f32| {
        let tau = std::f32::consts::TAU;
        let d = (from - to).rem_euclid(tau);
        let d = if d > std::f32::consts::PI { d - tau } else { d };
        to + d * k
    };
    (eye, turn(first.1, third.1), turn(first.2, third.2))
}

/// The first solid thing a cast from `from` to `to` meets: what the game looks through is passed (the cast goes on 0.1 past it,
/// and stops once less than 0.11 is left).
pub fn solid_hit(
    c: &mut ClientCollision,
    from: F3,
    to: F3,
    cast: &CameraCast<'_>,
) -> Option<RayHit> {
    let dir = normalize3(sub3(to, from));
    let mut start = from;
    for _ in 0..64 {
        let hit = c.cast_ray(start, to, cast.mask)?;
        if !(cast.see_through)(c, hit.object) {
            return Some(hit);
        }
        if len3(sub3(to, hit.point)) < 0.11 {
            return None;
        }
        start = add3(hit.point, dir.map(|d| d * 0.1));
    }
    None
}

pub fn sub3(a: F3, b: F3) -> F3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn add3(a: F3, b: F3) -> F3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn len3(a: F3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

pub fn normalize3(a: F3) -> F3 {
    let l = len3(a);
    if l > 0.0 { a.map(|x| x / l) } else { a }
}

/// What the step reads of the client's globals, taken before it runs: the game's preferences ([`game::client_prefs`]) and the
/// virtual time.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClientGlobals {
    pub prefs: game::GamePrefs,
    pub sim_time: u32,
}

impl ClientGlobals {
    pub fn of(vm: &Vm) -> ClientGlobals {
        ClientGlobals {
            prefs: game::client_prefs(vm),
            sim_time: vm.sim_time(),
        }
    }
}

/// The head's camera flag (`camera distance == 0`).
pub fn first_person(vm: &Vm) -> bool {
    vm.global_int("$firstPerson").is_none_or(|v| v & 0xff != 0)
}

/// A move as `processTick` reads it.
/// The Player a rider controls as this client's moves turn it ([`Predictor::mount_pred`]).
#[derive(Debug, Clone)]
pub struct MountPred {
    pub index: u32,
    pub yaw: f32,
    pub pitch: f32,
    pub still: u32,
    pub data: PlayerData,
}

/// The moves without a turn after which the controlled object's ghost yaw is trusted again, and the step its quantisation
/// leaves between it and the integrated yaw.
const MOUNT_SETTLE: u32 = 24;
const MOUNT_QUANTUM: f32 = 0.06;

/// An angle difference the short way round.
fn sim_turn(d: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let d = d.rem_euclid(tau);
    if d > std::f32::consts::PI { d - tau } else { d }
}

fn add_f32(a: f32, b: f32) -> f32 {
    (f64::from(a) + f64::from(b)) as f32
}

fn sim_move<W: PredictWorld>(m: &Move, game: &dyn game::PredictGame<W>) -> sim::Move {
    sim::Move {
        x: m.x,
        y: m.y,
        z: m.z,
        yaw: m.yaw,
        pitch: m.pitch,
        roll: m.roll,
        free_look: m.free_look,
        triggers: game.move_triggers(m),
    }
}

pub fn world_value<'w, W: PredictWorld>(
    world: &'w W,
    db: u32,
    name: &str,
    index: u32,
) -> Option<&'w Value> {
    world.datablocks.get(&db)?.value(name, index)
}

fn world_f32<W: PredictWorld>(world: &W, db: u32, name: &str) -> Option<f32> {
    match world_value(world, db, name, 0)? {
        Value::F32(x) => Some(*x),
        Value::Int(i) => Some(*i as f32),
        _ => None,
    }
}

/// The client's PlayerData from the datablock the server sent (`PlayerData::unpackData`'s fields; the ones the server does not
/// send keep the constructor's values), then `preload`'s products (the game's, `game`).
pub fn player_data<W: PredictWorld>(
    world: &W,
    db: u32,
    game: &dyn game::PredictGame<W>,
) -> Option<PlayerData> {
    let dbk = world.datablocks.get(&db)?;
    if dbk.class_name() != "PlayerData" {
        return None;
    }
    let mut d = PlayerData::default();
    let f = |name: &str, index: u32, dflt: f32| match world_value(world, db, name, index) {
        Some(Value::F32(x)) => *x,
        Some(Value::Int(i)) => *i as f32,
        _ => dflt,
    };
    let b = |name: &str, dflt: bool| match world_value(world, db, name, 0) {
        Some(Value::Bool(v)) => *v,
        Some(Value::Int(i)) => *i != 0,
        _ => dflt,
    };
    let i = |name: &str, dflt: i32| match world_value(world, db, name, 0) {
        Some(Value::Int(v)) => *v as i32,
        _ => dflt,
    };
    macro_rules! fl {
        ($field:ident, $name:literal) => {
            d.$field = f($name, 0, d.$field);
        };
    }
    fl!(mass, "mass");
    fl!(drag, "drag");
    fl!(density, "density");
    fl!(max_energy, "maxEnergy");
    d.is_invincible = b("isInvincible", d.is_invincible);
    d.inherit_energy = b("inheritEnergyFromMount", d.inherit_energy);
    fl!(min_look_angle, "minLookAngle");
    fl!(max_look_angle, "maxLookAngle");
    fl!(max_freelook_angle, "maxFreelookAngle");
    fl!(max_step_height, "maxStepHeight");
    fl!(run_force, "runForce");
    fl!(run_energy_drain, "runEnergyDrain");
    fl!(min_run_energy, "minRunEnergy");
    fl!(max_forward_speed, "maxForwardSpeed");
    fl!(max_backward_speed, "maxBackwardSpeed");
    fl!(max_side_speed, "maxSideSpeed");
    fl!(max_underwater_forward_speed, "maxUnderwaterForwardSpeed");
    fl!(max_underwater_backward_speed, "maxUnderwaterBackwardSpeed");
    fl!(max_underwater_side_speed, "maxUnderwaterSideSpeed");
    fl!(run_surface_angle, "runSurfaceAngle");
    fl!(jump_force, "jumpForce");
    fl!(jump_energy_drain, "jumpEnergyDrain");
    fl!(min_jump_energy, "minJumpEnergy");
    fl!(min_jump_speed, "minJumpSpeed");
    fl!(max_jump_speed, "maxJumpSpeed");
    fl!(jump_surface_angle, "jumpSurfaceAngle");
    d.jump_delay = i("jumpDelay", d.jump_delay);
    fl!(horiz_max_speed, "horizMaxSpeed");
    fl!(horiz_resist_speed, "horizResistSpeed");
    fl!(horiz_resist_factor, "horizResistFactor");
    fl!(up_max_speed, "upMaxSpeed");
    fl!(up_resist_speed, "upResistSpeed");
    fl!(up_resist_factor, "upResistFactor");
    fl!(min_impact_speed, "minImpactSpeed");
    fl!(ground_impact_min_speed, "groundImpactMinSpeed");
    for k in 0..3 {
        d.bounding_box[k] = f("boundingBox", k as u32, d.bounding_box[k]);
    }
    game.player_preload(&mut d);
    Some(d)
}

// ---- the client's collision world

/// One object of the client's world as the step sees it.
#[derive(Debug, Clone)]
pub struct Object {
    pub view: ObjectView,
    pub source: Source,
    /// A shape's collision hulls (a vehicle's, a game object's).
    pub shape: Option<Rc<drive::ClientShape>>,
    /// A map object's collision (torque-collide's `MapShape`: a terrain, an interior, water).
    pub map: Option<Rc<collide::map::MapShape>>,
}

/// A container object's `buildConvex`: one of torque-collide's classes, or a game object that also answers the client's ray
/// casts itself ([`GameSource`]).
#[derive(Debug, Clone)]
pub enum Source {
    Torque(ConvexSource),
    Game(Rc<dyn GameSource>),
}

impl Source {
    /// The source torque-collide builds the object's convexes from.
    fn convex(&self) -> ConvexSource {
        match self {
            Source::Torque(s) => s.clone(),
            Source::Game(g) => g.convex(),
        }
    }
}

/// A game object of the client's container (`SceneClass` on the client): its convex, and its answer to a ray in its own space.
pub trait GameSource: Debug + Any {
    /// The convex torque-collide's queries build (`buildConvex`).
    fn convex(&self) -> ConvexSource;

    /// `castRay` in the object's space: the segment `start` to `end` already moved into it. A cast asks twice: `first` with the
    /// whole mask (the index's objects, then the container's others, each segment moved by `to_object_space`), then with the
    /// mask less the index's bits (the container's others again, as the bins move it: `object_point`). The fraction and the
    /// normal in the object's space; `None` for a miss.
    fn cast_ray(&self, start: F3, end: F3, first: bool) -> Option<(f32, F3)>;
}

/// The game's static objects for one container ([`StaticIndex::set_near`]): asked before the container's other objects, in the
/// index's order.
pub trait StaticSet {
    /// The objects whose boxes the query box `lo`..`hi` may reach, in the index's order, charged to `work` (`Exhausted` past
    /// `budget`).
    fn near(&self, lo: F3, hi: F3, work: &mut u64, budget: u64) -> Result<Vec<u32>, Exhausted>;

    /// Object `i` of the set.
    fn get(&self, i: u32) -> &(ObjectId, Object);

    /// The set's object of id `id`, when the container holds it.
    fn find(&self, id: ObjectId) -> Option<u32>;

    /// Object `i` moved: its view now (the container's own copy), offered to every query from now on.
    fn set_view(&mut self, i: u32, view: ObjectView);
}

/// The game's static collision world on the client (its spatial index, `SpatialIndex` on the client): the objects it keeps apart
/// from the container's (a game's static objects), the ghosts whose objects it makes (a ground plane, a map's terrain), what it says
/// about them. Kept by the predictor for the connection ([`StaticWorld`]).
pub trait StaticIndex<W: PredictWorld>: Any {
    fn clone_box(&self) -> Box<dyn StaticIndex<W>>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// A client tick starts.
    fn start_tick(&mut self) {}

    /// The console lines about objects the client collides with otherwise than declared, each said once.
    fn take_notes(&mut self) -> Vec<String> {
        Vec::new()
    }

    /// The index's objects for a container made around `near` (made or brought up to date from `world`, the work charged to
    /// `work` up to `budget`); `None` for none.
    fn set_near(
        &mut self,
        world: &W,
        reader: Option<&FileReader>,
        near: F3,
        work: &mut u64,
        budget: u64,
    ) -> Option<Box<dyn StaticSet>> {
        let _ = (world, reader, near, work, budget);
        None
    }

    /// The object the index makes of ghost `index` among the container's own objects (in the ghosts' order); `None` for a ghost
    /// it does not answer for.
    fn ghost_object(
        &mut self,
        world: &W,
        index: u32,
        g: &Ghost<W::Set>,
        reader: Option<&FileReader>,
    ) -> Option<Object> {
        let _ = (world, index, g, reader);
        None
    }

    /// The type bits the index answers for: a ray asks the container's other objects for the rest.
    fn type_bits(&self) -> u32 {
        0
    }
}

impl<W: PredictWorld> Clone for Box<dyn StaticIndex<W>> {
    fn clone(&self) -> Self {
        (**self).clone_box()
    }
}

/// The predictor's static collision world for its connection: the game's answers, the install's file reader and the game's
/// static index ([`StaticIndex`], none in Torque's).
pub struct StaticWorld<W: PredictWorld> {
    /// What the prediction asks the game (Torque's until a game's is given: [`Predictor::with_game`], [`Predictor::set_view_globals`]).
    pub game: game::GameRef<W>,
    pub reader: Option<FileReader>,
    pub index: Option<Box<dyn StaticIndex<W>>>,
}

impl<W: PredictWorld> Clone for StaticWorld<W> {
    fn clone(&self) -> StaticWorld<W> {
        StaticWorld {
            game: self.game.clone(),
            reader: self.reader.clone(),
            index: self.index.clone(),
        }
    }
}

impl<W: PredictWorld> Default for StaticWorld<W> {
    fn default() -> StaticWorld<W> {
        StaticWorld {
            game: game::GameRef::default(),
            reader: None,
            index: None,
        }
    }
}

impl<W: PredictWorld> StaticWorld<W> {
    /// `game`'s static world, the reader kept.
    fn set_game(&mut self, game: Rc<dyn game::PredictGame<W>>) {
        self.index = game.static_index();
        self.game = game::GameRef(game);
    }

    /// A client tick starts.
    pub fn start_tick(&mut self) {
        if let Some(i) = self.index.as_deref_mut() {
            i.start_tick();
        }
    }

    /// The index's console lines since the last call.
    pub fn take_notes(&mut self) -> Vec<String> {
        self.index
            .as_deref_mut()
            .map_or_else(Vec::new, |i| i.take_notes())
    }

    /// The game's index as its own type.
    pub fn index<T: StaticIndex<W>>(&self) -> Option<&T> {
        self.index.as_deref()?.as_any().downcast_ref()
    }

    /// The game's index as its own type, mutably.
    pub fn index_mut<T: StaticIndex<W>>(&mut self) -> Option<&mut T> {
        self.index.as_deref_mut()?.as_any_mut().downcast_mut()
    }
}

/// What the client's container holds beside the ghosts' own: the static objects near `near`, the vehicles' shapes, the driven vehicle (its
/// predicted pose; its collision off: it is the mover).
pub struct Ctx<'a, W: PredictWorld> {
    pub near: F3,
    pub static_world: &'a mut StaticWorld<W>,
    pub shapes: &'a mut drive::ShapeCache,
    pub driven: Option<&'a drive::DrivenVehicle>,
    /// The vehicles the client runs on its own: at their own poses.
    pub ghosts: Option<&'a std::collections::BTreeMap<u32, drive::GhostVehicle>>,
    /// A ghost whose collision is off (the mover of a ghost vehicle's own tick).
    pub disabled: Option<u32>,
    /// The client's control Player at its predicted position (a ghost vehicle's tick meets it there).
    pub player_at: Option<(u32, F3)>,
}

/// A ray's hit in the client's container.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    pub t: f32,
    pub point: F3,
    pub normal: F3,
    pub object: ObjectId,
}

/// The client's container (`collide::World`) over the ghosts it holds, for one step: the ground plane and the other Players
/// (their boxes), newest first as the bins are. Every query is charged against the tick's budget ([`TICK_WORK`]).
pub struct ClientCollision {
    /// The static index's objects for this container ([`StaticIndex::set_near`]): they come before `objects`.
    statics: Option<Box<dyn StaticSet>>,
    /// The type bits the index answers for ([`StaticIndex::type_bits`]).
    index_bits: u32,
    objects: Vec<(ObjectId, Object)>,
    work: u64,
    budget: u64,
    /// The game's working-list order (none: the container's).
    order: Option<Rc<dyn CandidateOrder>>,
}

/// The mount of a Player the server says is mounted before its ghost's update said on what.
pub const PENDING_MOUNT: ObjectId = ObjectId(u32::MAX);

/// Whether the control object `ctrl` is a Player or AIPlayer ghost (a turret the Player rides): the server's driver split reads it
/// (`Player::processTick`'s `info.player`: the rider's turn goes to the turret, not to the rider).
fn controls_player<W: PredictWorld>(world: &W, ctrl: Option<ObjectId>) -> bool {
    let Some(index) = ctrl.and_then(|c| c.0.checked_sub(1)) else {
        return false;
    };
    world
        .ghosts
        .get(&index)
        .and_then(|g| g.class)
        .is_some_and(|c| <W::Set as EngineSet>::class_kind(c).is_player())
}

/// The mount of Player ghost `index` when it is a Player or AIPlayer whose ShapeBase part says a connection controls it (the turret
/// a rider controls; this client's control chain is the only one a rider's head comes with).
fn controlled_mount<W: PredictWorld>(world: &W, index: u32) -> Option<ObjectId> {
    let m = ghost_mount(world, index)?;
    let g = world.ghosts.get(&m.0.checked_sub(1)?)?;
    g.record.as_ref()?.player_state()?.controlled.then_some(m)
}

/// The object a ghost's ShapeBase part last said it is mounted on (`None`: not mounted, or not said yet).
fn ghost_mount<W: PredictWorld>(world: &W, index: u32) -> Option<ObjectId> {
    let g = world.ghosts.get(&index)?;
    let (i, _) = g.record.as_ref()?.player_state()?.mount?;
    Some(drive::ghost_id(i))
}

/// The damage state a Player ghost's record carries (`None`: none yet, or not a Player's).
fn damage_state<W: PredictWorld>(world: &W, index: u32) -> Option<u32> {
    let g = world.ghosts.get(&index)?;
    if g.class.map(<W::Set as EngineSet>::class_kind) != Some(ClassKind::Player) {
        return None;
    }
    g.record.as_ref()?.player_state()?.damage
}

/// An object's view at `m` with object box `obj_box` and `scale` (`resetWorldBox`'s world box and radius).
pub fn view_of(m: Matrix, obj_box: Box3, scale: F3, type_mask: u32) -> ObjectView {
    let scaled = Box3::new(
        [0, 1, 2].map(|i| (f64::from(obj_box.min[i]) * f64::from(scale[i])) as f32),
        [0, 1, 2].map(|i| (f64::from(obj_box.max[i]) * f64::from(scale[i])) as f32),
    );
    let mut w = mul_box(&m, &scaled);
    if !(w.max[0] >= w.min[0] && w.max[1] >= w.min[1] && w.max[2] >= w.min[2]) {
        w = Box3::new([-1.0; 3], [1.0; 3]);
    }
    let c = |i: usize| ((f64::from(w.max[i]) + f64::from(w.min[i])) * 0.5) as f32;
    let center = [c(0), c(1), c(2)];
    let dd = |i: usize| (f64::from(w.max[i]) - f64::from(center[i])) as f32;
    let (dx, dy, dz) = (dd(0), dd(1), dd(2));
    let sq = ((f64::from(dx) * f64::from(dx) + f64::from(dy) * f64::from(dy))
        + f64::from(dz) * f64::from(dz)) as f32;
    ObjectView {
        obj_to_world: m,
        world_to_obj: affine_inverse(&m),
        scale,
        obj_box,
        world_box: w,
        world_radius: collide::math::sqrt_f32(sq),
        type_mask,
        collision_disabled: false,
    }
}

impl ClientCollision {
    /// The world of `world` for the predicted Player whose ghost is `me` (left out: the predicted Player is the mover, its ghost is
    /// the same object).
    ///
    /// The construction is charged to `budget` (every ghost looked at, every datablock read): what is left is the tick's.
    /// With `ctx`, the game's static index's objects near the point are in it too (before every other object, as the server's
    /// container asks its indexes before its bins), and the ghosts it makes objects of.
    pub fn new<W: PredictWorld>(
        world: &W,
        me: u32,
        budget: u64,
        ctx: Option<Ctx<'_, W>>,
    ) -> ClientCollision {
        let game = ctx.as_ref().map_or_else(
            || Rc::new(game::TorquePrediction) as Rc<dyn game::PredictGame<W>>,
            |c| Rc::clone(&c.static_world.game.0),
        );
        let mut objects = Vec::new();
        let mut work = 0u64;
        let mut set = None;
        let (mut shapes, reader, driven, ghost_vehicles, disabled, player_at, mut statics) =
            match ctx {
                Some(c) => {
                    let reader = c.static_world.reader.clone();
                    let mut index = c.static_world.index.as_deref_mut();
                    if let Some(i) = index.as_deref_mut() {
                        set = i.set_near(world, reader.as_ref(), c.near, &mut work, budget);
                    }
                    (
                        Some(c.shapes),
                        reader,
                        c.driven,
                        c.ghosts,
                        c.disabled,
                        c.player_at,
                        index,
                    )
                }
                None => (None, None, None, None, None, None, None),
            };
        let index_bits = statics.as_deref().map_or(0, |i| i.type_bits());
        let mut datablocks: std::collections::HashMap<u32, Option<PlayerData>> =
            std::collections::HashMap::new();
        // The static objects are in the set (none of them is one of the kinds below); the others from the world's index by
        // class, not a walk of every ghost (half a million static ghosts would be walked at every move).
        let mut ghosts = world.container_ghosts();
        // The bins answer the newest object first.
        ghosts.sort_by_key(|(_, g)| std::cmp::Reverse(g.serial));
        for (index, g) in ghosts {
            work = work.saturating_add(GHOST_WORK);
            if work > budget {
                break;
            }
            if index == me {
                continue;
            }
            let id = ObjectId(index.wrapping_add(1));
            let kind = g.class.map(<W::Set as EngineSet>::class_kind);
            match (kind, &g.record) {
                (Some(k), _) if k.is_player() => {
                    let pos = match player_at {
                        Some((pi, at)) if pi == index => Some(at),
                        _ => g.position,
                    };
                    let Some(pos) = pos else { continue };
                    let Some(db_id) = g.datablock() else {
                        continue;
                    };
                    let db = datablocks.entry(db_id).or_insert_with(|| {
                        work = work.saturating_add(DATABLOCK_WORK);
                        player_data(world, db_id, &*game)
                    });
                    let Some(db) = db.as_ref() else {
                        continue;
                    };
                    let h = |x: f32| (f64::from(x) * 0.5) as f32;
                    let obj_box = Box3::new(
                        [-h(db.bounding_box[0]), -h(db.bounding_box[1]), 0.0],
                        [
                            h(db.bounding_box[0]),
                            h(db.bounding_box[1]),
                            db.bounding_box[2],
                        ],
                    );
                    let mut m = collide::math::IDENTITY;
                    m[3] = pos[0];
                    m[7] = pos[1];
                    m[11] = pos[2];
                    let dead = g
                        .record
                        .as_ref()
                        .and_then(|r| r.player_state())
                        .and_then(|s| s.damage)
                        .is_some_and(|d| d != 0);
                    let mask = if dead {
                        CORPSE_TYPE_MASK
                    } else {
                        PLAYER_TYPE_MASK
                    };
                    let view = view_of(m, obj_box, [1.0; 3], mask);
                    objects.push((
                        id,
                        Object {
                            view,
                            source: Source::Torque(game.player_convex()),
                            shape: None,
                            map: None,
                        },
                    ));
                }
                (Some(k), Some(rec)) if k.is_vehicle() => {
                    let Some(shapes) = shapes.as_deref_mut() else {
                        continue;
                    };
                    let Some(path) = g.datablock().and_then(|db| drive::shape_path(world, db))
                    else {
                        continue;
                    };
                    let Some(shape) = shapes.get(path, reader.as_ref(), &mut work, budget) else {
                        continue;
                    };
                    let ghost = ghost_vehicles.and_then(|m| m.get(&index));
                    let (m, off) = match (driven.filter(|d| d.index == index), ghost) {
                        (Some(d), _) => (d.v.s.obj_to_world, true),
                        (None, Some(gv)) => (gv.body.v.s.obj_to_world, disabled == Some(index)),
                        (None, None) => {
                            let pose = rec.vehicle().and_then(|v| v.pose);
                            let (Some(pose), Some(pos)) = (pose, g.position) else {
                                continue;
                            };
                            let mut m = quat_matrix(pose.rotation);
                            m[3] = pos[0];
                            m[7] = pos[1];
                            m[11] = pos[2];
                            (m, false)
                        }
                    };
                    let mut view = drive::vehicle_view(&m, &shape);
                    view.collision_disabled = off;
                    objects.push((
                        id,
                        Object {
                            view,
                            source: Source::Torque(ConvexSource::Shape {
                                has_shape: true,
                                slots: shape.hull_slots(),
                            }),
                            shape: Some(shape),
                            map: None,
                        },
                    ));
                }
                _ => {
                    if let Some(o) = statics
                        .as_deref_mut()
                        .and_then(|s| s.ghost_object(world, index, g, reader.as_ref()))
                    {
                        objects.push((id, o));
                    }
                }
            }
        }
        ClientCollision {
            statics: set,
            index_bits,
            objects,
            work,
            budget,
            order: game.candidate_order(),
        }
    }

    /// Whether the budget is spent.
    pub fn exhausted(&self) -> bool {
        self.work > self.budget
    }

    /// The work done so far.
    pub fn work(&self) -> u64 {
        self.work
    }

    /// The object `id` of the container (the game's static set first), if it is there.
    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        if let Some(set) = &self.statics
            && let Some(i) = set.find(id)
        {
            return Some(&set.get(i).1);
        }
        self.objects.iter().find(|(i, _)| *i == id).map(|(_, o)| o)
    }

    /// An object's type mask (0 for one that is not there).
    pub fn type_mask(&self, id: ObjectId) -> u32 {
        self.get(id).map_or(0, |o| o.view.type_mask)
    }

    /// A container with nothing in it (a placement outside a tick).
    pub fn empty() -> ClientCollision {
        ClientCollision {
            statics: None,
            index_bits: 0,
            objects: Vec::new(),
            work: 0,
            budget: TICK_WORK,
            order: None,
        }
    }

    /// An object moved (`setPosition`'s rebin): its view now, its collision switch as it was.
    pub fn set_view(&mut self, id: ObjectId, view: ObjectView) {
        // An index object moved (none does today): the set's to answer, the object offered to every query.
        if let Some(set) = self.statics.as_mut()
            && let Some(i) = set.find(id)
        {
            let mut view = view;
            view.collision_disabled = set.get(i).1.view.collision_disabled;
            set.set_view(i, view);
            return;
        }
        if let Some((_, o)) = self.objects.iter_mut().find(|(i, _)| *i == id) {
            let disabled = o.view.collision_disabled;
            o.view = view;
            o.view.collision_disabled = disabled;
        }
    }

    /// `Container::castRay` on the client's container: a segment with a NaN or an infinity answers nothing; the static
    /// index's objects first (each against the segment in its space, the nearest, the first reached on a tie), then the bins for
    /// the mask less the index's bits, taken only when strictly nearer. Charged per object looked at.
    pub fn cast_ray(&mut self, start: F3, end: F3, mask: u32) -> Option<RayHit> {
        if !start.iter().chain(&end).all(|v| v.is_finite()) {
            return None;
        }
        let mut best_t = torque_scene::NO_HIT;
        let mut best: Option<RayHit> = None;
        let mut work = self.work;
        // Only the index's objects the segment's box may reach (they answer only the first pass).
        let lo = [0, 1, 2].map(|k| start[k].min(end[k]));
        let hi = [0, 1, 2].map(|k| start[k].max(end[k]));
        let near: Vec<u32> = match &self.statics {
            Some(set) => match set.near(lo, hi, &mut work, self.budget) {
                Ok(v) => v,
                Err(Exhausted) => {
                    self.work = work;
                    return None;
                }
            },
            None => Vec::new(),
        };
        let statics = self.statics.as_deref();
        for pass in [0u8, 1] {
            let pass_mask = if pass == 0 {
                mask
            } else {
                mask & !self.index_bits
            };
            let listed = near
                .iter()
                .filter(|_| pass == 0)
                .filter_map(|&i| statics.map(|s| s.get(i)));
            for (id, o) in listed.chain(&self.objects) {
                work = work.saturating_add(collide::work::CANDIDATE);
                if work > self.budget {
                    self.work = work;
                    return best;
                }
                if o.view.type_mask & pass_mask == 0 || o.view.collision_disabled {
                    continue;
                }
                // A map object answers in the world's space.
                if pass == 1
                    && let Some(m) = &o.map
                {
                    let budget = self.budget;
                    let hit = m.cast_ray(start, end, &mut |u| {
                        work = work.saturating_add(u);
                        work <= budget
                    });
                    match hit {
                        Ok(Some((t, n))) if t < best_t => {
                            best_t = t;
                            best = Some(RayHit {
                                t,
                                point: torque_scene::geometry::ray_point(start, end, t),
                                normal: n,
                                object: *id,
                            });
                        }
                        Ok(_) => {}
                        Err(Exhausted) => {
                            self.work = work;
                            return best;
                        }
                    }
                    continue;
                }
                let Source::Game(g) = &o.source else {
                    continue;
                };
                // As the server's container: the first pass's segment by `to_object_space` (a spatial index's), the second's by
                // `object_point` (the bins'), both through `affine_inverse` of the transform.
                let w2o = transform::affine_inverse(&o.view.obj_to_world);
                let (a, b) = if pass == 0 {
                    (
                        transform::to_object_space(&w2o, o.view.scale, start),
                        transform::to_object_space(&w2o, o.view.scale, end),
                    )
                } else {
                    let point = torque_scene::library::get().object_point;
                    (
                        point(&w2o, o.view.scale, start),
                        point(&w2o, o.view.scale, end),
                    )
                };
                if let Some((t, n)) = g.cast_ray(a, b, pass == 0)
                    && t < best_t
                {
                    best_t = t;
                    best = Some(RayHit {
                        t,
                        point: torque_scene::geometry::ray_point(start, end, t),
                        normal: transform::rotate(&o.view.obj_to_world, n),
                        object: *id,
                    });
                }
            }
        }
        self.work = work;
        best
    }
}

impl collide::World for ClientCollision {
    fn object(&self, id: ObjectId) -> Option<ObjectView> {
        self.get(id).map(|o| o.view)
    }

    fn find_objects(
        &mut self,
        query: &Box3,
        type_mask: u32,
        exclude: Option<ObjectId>,
        limit: usize,
        out: &mut Vec<ObjectId>,
    ) -> Result<(), Exhausted> {
        // The index's objects near the query first (in its order), then the other objects, as one list held them.
        let mut work = self.work;
        let near = match &self.statics {
            Some(set) => set.near(query.min, query.max, &mut work, self.budget),
            None => Ok(Vec::new()),
        };
        self.work = work;
        let near = near?;
        let statics = self.statics.as_deref();
        let listed = near.iter().filter_map(|&i| statics.map(|s| s.get(i)));
        for (id, o) in listed.chain(&self.objects) {
            self.work = self.work.saturating_add(collide::work::CANDIDATE);
            if self.work > self.budget {
                return Err(Exhausted);
            }
            if out.len() >= limit {
                return Ok(());
            }
            if Some(*id) == exclude
                || o.view.type_mask & type_mask == 0
                || o.view.collision_disabled
                || !o.view.world_box.overlaps(query)
            {
                continue;
            }
            out.push(*id);
        }
        Ok(())
    }

    fn order_candidates(&mut self, ids: &mut [ObjectId]) {
        let Some(order) = self.order.clone() else {
            return;
        };
        let mut work = self.work;
        let budget = self.budget;
        order.order(&*self, ids, &mut || {
            work = work.saturating_add(1);
            work <= budget
        });
        self.work = work;
    }

    fn convex_source(&self, id: ObjectId) -> ConvexSource {
        self.get(id)
            .map_or(ConvexSource::None, |o| o.source.convex())
    }

    fn hull(&self, id: ObjectId, slot: u32) -> Option<Rc<Hull>> {
        self.get(id)?
            .shape
            .as_ref()?
            .hulls
            .get(usize::try_from(slot).ok()?)?
            .clone()
    }

    fn charge(&mut self, units: u64) -> bool {
        self.work = self.work.saturating_add(units);
        self.work <= self.budget
    }

    fn map_shape(&self, id: ObjectId) -> Option<Rc<collide::map::MapShape>> {
        self.get(id)?.map.clone()
    }
}

/// The predicted Player's host: the client's world, no skeleton, no callbacks.
struct ClientPlayerHost<'a, W: PredictWorld> {
    world: &'a mut ClientCollision,
    skeleton: NoSkeleton,
    statics: &'a mut LiquidStatics,
    sim_time: u32,
    vehicle: Option<&'a mut drive::DrivenVehicle>,
    /// The control object is a Player or AIPlayer ghost (a turret the Player rides and controls).
    controls_player: bool,
    /// The Player's class (the game's).
    class: Rc<dyn PlayerClass>,
    game: Rc<dyn game::PredictGame<W>>,
}

impl<W: PredictWorld> ClientPlayerHost<'_, W> {
    fn driven(&mut self, id: ObjectId) -> Option<&mut drive::DrivenVehicle> {
        self.vehicle
            .as_deref_mut()
            .filter(|v| drive::ghost_id(v.index) == id)
    }
}

impl<W: PredictWorld> PlayerHost for ClientPlayerHost<'_, W> {
    fn world(&mut self) -> &mut dyn collide::World {
        self.world
    }

    fn player_class(&self) -> Option<Rc<dyn sim::player::PlayerClass>> {
        Some(Rc::clone(&self.class))
    }

    fn skeleton(&mut self) -> &mut dyn Skeleton {
        &mut self.skeleton
    }

    fn sim_time(&self) -> u32 {
        self.sim_time
    }

    fn type_masks(&self) -> &collide::TypeMasks {
        self.game.type_masks()
    }

    fn liquid_statics(&mut self) -> &mut LiquidStatics {
        self.statics
    }

    fn controlled_info(&mut self, id: ObjectId) -> sim::player::host::ControlledInfo {
        sim::player::host::ControlledInfo {
            vehicle: self.driven(id).is_some(),
            player: self.controls_player && self.driven(id).is_none(),
        }
    }

    /// The game's Player subclass's questions ([`game::PredictGame::class_query`]).
    fn class_query(&mut self, query: &mut dyn std::any::Any) {
        self.game.class_query(query, self.vehicle.as_deref());
    }

    fn tick_controlled(&mut self, _s: &mut PlayerState, control: ObjectId, mv: Option<&sim::Move>) {
        let sim_time = self.sim_time;
        let mv = mv.map(|m| vehicle::Move {
            x: m.x,
            y: m.y,
            z: m.z,
            yaw: m.yaw,
            pitch: m.pitch,
            roll: m.roll,
            free_look: m.free_look,
            triggers: m.triggers,
        });
        let Some(v) = self
            .vehicle
            .as_deref_mut()
            .filter(|v| drive::ghost_id(v.index) == control)
        else {
            return;
        };
        v.tick(mv.as_ref(), self.world, sim_time);
    }
}

pub mod aim;
pub mod drive;
pub mod game;
pub mod ghost_players;
pub mod look;
