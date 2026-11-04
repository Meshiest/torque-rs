//! What the client's engine half reads of a game's records beside the world's own questions ([`ghost_world::set`]): a set's
//! classes as the prediction sorts them, its control objects' packet data ([`EngineSet`]), a record's ShapeBase, Player and
//! vehicle blocks ([`EngineRecord`]), a datablock's fields ([`EngineDatablock`]) and a control section's tail ([`EngineTail`]).
//! The engine reads them as plain values ([`PlayerWords`], [`VehicleUpdate`], torque-ghosts' [`Value`]); a game gives the conversions from
//! its own records (TGE's: [`tge`], over torque-ghost-world's `TgeClientSet`).

use collide::math::F3;
use ghost_world::ClientSet;
use ghosts::Value;
use net::control::ControlData;
use net::moves::Move;
use vehicle::words::{VehicleWords, WheelWords};

pub mod tge;

/// A ghost class as the prediction sorts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassKind {
    Player,
    AiPlayer,
    WheeledVehicle,
    FlyingVehicle,
    Other,
}

impl ClassKind {
    /// A Player or an AIPlayer.
    pub fn is_player(self) -> bool {
        matches!(self, ClassKind::Player | ClassKind::AiPlayer)
    }

    /// A WheeledVehicle or a FlyingVehicle.
    pub fn is_vehicle(self) -> bool {
        matches!(self, ClassKind::WheeledVehicle | ClassKind::FlyingVehicle)
    }
}

/// A Player's position block in its packet data (sent when it is not mounted).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FreeMotion {
    pub position: F3,
    pub velocity: F3,
    pub contact: u32,
}

/// A Player's `writePacketData` words, its control object's data apart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerWords {
    pub energy: f32,
    /// The word a game's packet data carries beside Torque's (none in Torque's: false).
    pub flag: bool,
    pub free: Option<FreeMotion>,
    pub head_x: f32,
    pub head_z: f32,
    pub rot_z: f32,
    /// The object's world translation (the control section's equal branch sends it raw).
    pub translation: F3,
}

/// A Player's packet data as read: its words and its control object's ghost index and data, when it controls one.
pub struct PlayerPacket<'a> {
    pub words: PlayerWords,
    pub controlled: Option<(u32, &'a dyn ControlData)>,
}

/// What a game's set gives the engine half beside the world's questions.
pub trait EngineSet:
    ClientSet<Record: EngineRecord<Self>, Datablock: EngineDatablock, Tail: EngineTail>
{
    /// How the prediction sorts `class`.
    fn class_kind(class: Self::Class) -> ClassKind;

    /// A Player's packet data (`None` for another class's).
    fn read_player(data: &dyn ControlData) -> Option<PlayerPacket<'_>>;

    /// A Player's packet data from its words and its control object's ghost index and data.
    fn player_data(
        words: &PlayerWords,
        controlled: Option<(u32, Box<dyn ControlData>)>,
    ) -> Box<dyn ControlData>;

    /// A vehicle's packet data (`None` for another class's).
    fn read_vehicle(data: &dyn ControlData) -> Option<VehicleWords>;

    /// A vehicle's packet data from its words, at world translation `translation`.
    fn vehicle_data(words: &VehicleWords, translation: F3) -> Box<dyn ControlData>;
}

/// A ShapeBase part's state as the record last sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShapeState {
    /// The damage state (`None`: not sent yet).
    pub damage: Option<u32>,
    /// The ghost index and node it is mounted on.
    pub mount: Option<(u32, u32)>,
    /// A connection controls the object (its cloak block's controlled bit).
    pub controlled: bool,
}

/// A vehicle update's pose block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VehiclePose {
    pub rotation: [f32; 4],
    pub linear: F3,
    pub angular: F3,
    pub at_rest: bool,
    pub warp: bool,
}

/// A wheel's tire and spring datablocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WheelDatablocks {
    pub tire: u32,
    pub spring: u32,
}

/// A WheeledVehicle's or FlyingVehicle's update as the client's vehicles read it.
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleUpdate {
    /// A FlyingVehicle's (no wheels).
    pub flying: bool,
    /// The server's client controls it (its update carries no state for that client).
    pub controlled: bool,
    /// The steering as sent: two 9-bit fractions of the datablock's maximum.
    pub steering: [u32; 2],
    pub last_move: Move,
    pub pose: Option<VehiclePose>,
    pub brake: bool,
    pub wheels: Option<Vec<WheelWords>>,
    pub wheel_datablocks: Option<Vec<Option<WheelDatablocks>>>,
}

/// What the engine half reads of a record of the set `S` beside the world's questions.
pub trait EngineRecord<S: ?Sized> {
    /// The ShapeBase part's state, for a record of a ShapeBase class.
    fn shape_state(&self) -> Option<ShapeState>;

    /// The ShapeBase part's state, for a Player's or an AIPlayer's record.
    fn player_state(&self) -> Option<ShapeState>;

    /// A vehicle's update, for a WheeledVehicle's or a FlyingVehicle's record.
    fn vehicle(&self) -> Option<VehicleUpdate>;

    /// A Player's or an AIPlayer's move block: its velocity and its head's pitch as a fraction of the look range (-1 to 1),
    /// when the record carries one ([`crate::predict::ghost_players`] runs the Player on from them). None by default.
    fn player_motion(&self) -> Option<(F3, f32)> {
        None
    }
}

/// What the engine half reads of a datablock.
pub trait EngineDatablock {
    /// The datablock's class name.
    fn class_name(&self) -> &str;

    /// Element `index` of field `name` as the datablock's event carried it, when it carries it.
    fn value(&self, name: &str, index: u32) -> Option<&Value>;
}

/// What the engine half reads of a control section's tail.
pub trait EngineTail {
    /// A Player's words, for a Player's tail.
    fn player(&self) -> Option<PlayerWords>;
}
