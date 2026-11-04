//! What the client's prediction asks the game ([`PredictGame`], given by [`crate::game::ClientGame::prediction`]): the
//! Player class a `PlayerData` makes and what it keeps on the predicted Player, the moves' trigger format, the type masks the
//! movers query with and the working list's order, the view's cameras and smooth aim's next move. Every method has Torque's
//! answer by default (TGE's Player, six triggers, TGE's masks, the container's order, TGE's cameras). The game's preferences
//! ([`GamePrefs`]) are read without the world type, by a `Vm` extension ([`ClientPrefs`]).
//!
//! Credit: the cameras' casts follow TGE as Torque3D carries it (MIT: `T3D/shapeBase.cpp`, Copyright (c) 2012 GarageGames, LLC;
//! Copyright (C) 2015 Faust Logic, Inc.; `T3D/vehicles/vehicle.cpp` and `T3D/fps/guiShapeNameHud.cpp`, each Copyright (c) 2012
//! GarageGames, LLC).

use std::any::Any;
use std::fmt::Debug;
use std::ops::DerefMut;
use std::rc::Rc;

use collide::convex::ConvexSource;
use collide::math::F3;
use collide::{ObjectId, TypeMasks};
use ghost_world::Ghost;
use net::control::ControlData;
use net::moves::{MAX_TRIGGER_KEYS, Move};
use shapes::preload::SlotRule;
use sim::player::{PlayerData, PlayerState};
use torque_game::scene::container::CandidateOrder;
use torque_game::sim_classes::{PlayerKind, TorquePlayerKind};
use vehicle::VehicleState;
use vehicle::wheeled::WheeledClass;

use ts_vm::vm::Vm;

use super::aim::{Ctx, Waiting};
use super::drive::DrivenVehicle;
use super::{CameraCast, ClientCollision, StaticIndex, ThirdPerson, VehicleEye, VehicleView};
use crate::records::EngineSet;
use crate::render::PoseView;

/// The client's world as the prediction reads and sets it: the record world and the posing ([`PoseView`]) over a set the engine
/// half reads ([`EngineSet`]), and what the prediction asks of the game's view beside them.
pub trait PredictWorld: PoseView<Set: EngineSet> + DerefMut + 'static {
    /// The ghosts a client container makes objects of itself (the ones the game's static index keeps are left out), any order.
    fn container_ghosts(&self) -> Vec<(u32, &Ghost<Self::Set>)>;

    /// The control Player's own state as the client predicts it (its ghost's updates carry none of it for its own client), for the
    /// renderer; `None` when no Player is predicted.
    fn set_control_state(&mut self, state: Option<ControlState>);
}

/// The control Player's state the renderer reads ([`PredictWorld::set_control_state`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlState {
    pub crouched: bool,
    pub jetting: bool,
    pub falling: bool,
    pub on_ground: bool,
    /// The action's root for water ([`PredictGame::water_root`]).
    pub water_root: bool,
    pub velocity: [f32; 3],
}

/// A game's answers to the client's prediction over its world `W`.
pub trait PredictGame<W: PredictWorld>: Debug {
    /// The Player class of `PlayerData` `db` as the server sent it (the game's subclass reads its own fields of the record).
    fn player_kind(&self, world: &W, db: u32) -> Rc<dyn PlayerKind> {
        let _ = (world, db);
        Rc::new(TorquePlayerKind)
    }

    /// `PlayerData::preload`'s products, after the client read the datablock's fields.
    fn player_preload(&self, d: &mut PlayerData) {
        d.preload();
    }

    /// `Player::readPacketData`'s words the subclass keeps on the Player: `flag` is the word the game's packet data carries
    /// beside Torque's (none in Torque's).
    fn read_packet_flag(&self, s: &mut PlayerState, flag: bool) {
        let _ = (s, flag);
    }

    /// The client's preferences the subclass reads before a step.
    fn before_step(&self, s: &mut PlayerState, prefs: &GamePrefs) {
        let _ = (s, prefs);
    }

    /// The convex another Player gives the client's container (`Player::buildConvex`).
    fn player_convex(&self) -> ConvexSource {
        ConvexSource::Player { has_shape: true }
    }

    /// The type masks the client's movers query and test with.
    fn type_masks(&self) -> &'static TypeMasks {
        &TypeMasks::TGE
    }

    /// The masks of the view's casts (TGE's by default, [`CameraMasks::TGE`]).
    fn camera_masks(&self) -> CameraMasks {
        CameraMasks::TGE
    }

    /// Whether a camera's cast looks through `object`, a hit in `collision` (TGE's stops at every hit).
    fn see_through(&self, world: &W, collision: &ClientCollision, object: ObjectId) -> bool {
        let _ = (world, collision, object);
        false
    }

    /// The order a collision phase builds its working list in; none keeps the container's (TGE).
    fn candidate_order(&self) -> Option<Rc<dyn CandidateOrder>> {
        None
    }

    /// A question the game's Player subclass asks its host (`PlayerHost::class_query`), answered in place; `driven` is the vehicle
    /// the predicted Player drives, if any.
    fn class_query(&self, query: &mut dyn Any, driven: Option<&DrivenVehicle>) {
        let _ = (query, driven);
    }

    /// The game's static collision world for a connection ([`StaticIndex`]: its own static objects and map objects); none in
    /// Torque's (the container holds the ghosts' own objects only).
    fn static_index(&self) -> Option<Box<dyn StaticIndex<W>>> {
        None
    }

    /// `Camera::readPacketData` of a resent control object: `None` when `data` is not a camera's, else the camera the client
    /// predicts from it (`None` in it: one the client does not predict, whose packets then carry checksum 0); `old` is the camera
    /// predicted until now. Torque's predicts no camera.
    fn read_camera(
        &self,
        old: Option<Box<dyn ControlCamera<W>>>,
        index: u32,
        data: &dyn ControlData,
        world: &W,
    ) -> Option<Option<Box<dyn ControlCamera<W>>>> {
        let _ = (old, index, data, world);
        None
    }

    /// The rule a shape's collision and line-of-sight details are picked by (TGE's by default).
    fn shape_slots(&self) -> SlotRule {
        shapes::preload::collision_slots
    }

    /// The WheeledVehicle subclass of `WheeledVehicleData` `db` and its steering rules (TGE's: none).
    fn wheeled(&self, world: &W, db: u32) -> Wheeling {
        let _ = (world, db);
        Wheeling::default()
    }

    /// What the subclass sets on a WheeledVehicle the client makes (nothing by default).
    fn new_wheeled(&self, s: &mut VehicleState) {
        let _ = s;
    }

    /// The controlling client's auto-return preference as the vehicle holds it (on by default).
    fn client_auto_return(&self, s: &VehicleState) -> bool {
        let _ = s;
        true
    }

    /// The game's projectile ghosts the client runs on its own ([`GhostProjectiles`]); none in Torque's.
    fn projectiles(&self) -> Option<Box<dyn GhostProjectiles<W>>> {
        None
    }

    /// Whether the view is drawn ahead of the last tick by the next move ([`super::aim`]: smooth aim), as the game's preferences
    /// say now (off in Torque's).
    fn smooth_aim(&self, vm: &ts_vm::vm::Vm) -> bool {
        let _ = vm;
        false
    }

    /// The triggers a move gives the step (all of Torque's).
    fn move_triggers(&self, m: &Move) -> [bool; MAX_TRIGGER_KEYS] {
        m.triggers
    }

    /// The control Player's yaw and head after `mv`, the Player's part of its next tick as smooth aim draws it ([`super::aim`]):
    /// TGE's driver split and `updateMove`'s head and rotation by default ([`super::aim::next`]).
    fn next_angles(
        &self,
        s: &PlayerState,
        db: &PlayerData,
        mv: &sim::Move,
        ctx: &Ctx,
    ) -> (f32, [f32; 3]) {
        super::aim::next(s, db, mv, ctx)
    }

    /// The yaw and pitch a move turns the Player its rider controls by (a mount, a turret: the driver split's control move), at
    /// camera distance `camera_pos`: TGE's by default ([`super::aim::mount_move`]).
    fn control_turn(&self, mv: &sim::Move, camera_pos: f32) -> [f32; 2] {
        super::aim::mount_move(mv, camera_pos)
    }

    /// Whether the control Player's action is the root for the water it is in (`on_ground`: its contact as the renderer reads it).
    /// TGE's `pickActionAnimation` has no water rule.
    fn water_root(&self, s: &PlayerState, on_ground: bool) -> bool {
        let _ = (s, on_ground);
        false
    }

    /// The control Player's third-person eye and its pitch (`getCameraTransform` at a camera distance above 0) for its datablock
    /// `db`; `collision` makes the client's container for the casts. `None` keeps the first-person eye. TGE's
    /// `ShapeBase::getCameraTransform` by default ([`super::tge_third_person_eye`]).
    fn third_person_eye(
        &self,
        world: &W,
        db: u32,
        view: &ThirdPerson,
        collision: &mut dyn FnMut() -> ClientCollision,
        cast: &CameraCast<'_>,
    ) -> Option<(F3, f32)> {
        super::tge_third_person_eye(world, db, view, collision, cast)
    }

    /// A driver's third-person eye through its vehicle of datablock `db` (`getCameraTransform` at a camera distance above 0);
    /// `collision` makes the client's container for the casts. `None` keeps the first-person eye. TGE's
    /// `Vehicle::getCameraTransform` by default ([`super::tge_vehicle_eye`]).
    fn vehicle_eye(
        &self,
        world: &W,
        db: u32,
        view: &VehicleView,
        collision: &mut dyn FnMut() -> ClientCollision,
        cast: &CameraCast<'_>,
    ) -> Option<VehicleEye> {
        super::tge_vehicle_eye(world, db, view, collision, cast)
    }
}

/// The client's preferences the game's prediction reads ([`ClientPrefs`]); all off in Torque's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GamePrefs {
    /// Strafe steering: a driver steers with the strafe keys where the vehicle allows it ([`SteerRules::strafe`]).
    pub strafe_steering: bool,
    /// The steering's auto-return as the client set it.
    pub auto_return_steering: bool,
}

/// How a game reads its preferences from the client's globals (a `Vm` extension: [`set_client_prefs`], read without the world type
/// by [`super::ClientGlobals::of`]). Torque's reads none.
pub trait ClientPrefs: Debug {
    fn read(&self, vm: &Vm) -> GamePrefs;
}

struct InstalledPrefs(Rc<dyn ClientPrefs>);

/// The game's preference reader for `vm` from now on.
pub fn set_client_prefs(vm: &mut Vm, prefs: Rc<dyn ClientPrefs>) {
    vm.set_extension(Rc::new(InstalledPrefs(prefs)));
}

/// The game's preferences as `vm`'s globals hold them (all off without a reader).
pub fn client_prefs(vm: &Vm) -> GamePrefs {
    vm.extension::<InstalledPrefs>()
        .map_or_else(GamePrefs::default, |p| p.0.read(vm))
}

/// The masks of the view's casts: the third-person camera's (`ShapeBase::getCameraTransform`), a vehicle's
/// (`Vehicle::getCameraTransform`) and the shape-name HUD's line of sight (`GuiShapeNameHud::onRender`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraMasks {
    pub third_person: u32,
    pub vehicle: u32,
    pub names: u32,
}

impl CameraMasks {
    /// TGE's: the cameras cast against every type but water, game objects and the default type (TGE 1.4 `shapeBase.cc`,
    /// `vehicle.cc`); the names against terrain, interiors and shapes (`guiShapeNameHud.cc`).
    pub const TGE: CameraMasks = {
        use collide::types::{GAME_BASE, INTERIOR, SHAPE_BASE, TERRAIN, WATER};
        let mask = !(WATER | GAME_BASE);
        CameraMasks {
            third_person: mask,
            vehicle: mask,
            names: TERRAIN | INTERIOR | SHAPE_BASE,
        }
    };
}

/// A Camera control object the game predicts: its modes, its tick over the moves, its view and its packet data's checksum.
pub trait ControlCamera<W: PredictWorld>: Debug + Any {
    fn box_clone(&self) -> Box<dyn ControlCamera<W>>;

    /// The camera's ghost index.
    fn index(&self) -> u32;

    /// Its position (the client's container is made around it).
    fn position(&self) -> F3;

    /// A client tick begins (once a packet, whatever moves it runs).
    fn begin(&mut self, world: &W);

    /// One `processTick` with move `mv` over the client's container.
    fn step(&mut self, mv: &Move, world: &W, collision: &mut ClientCollision);

    /// The view `back` between the last tick (0) and the one before (1): the eye and its (pitch, yaw); `aim` is smooth aim's
    /// waiting move, when it is on.
    fn frame(
        &self,
        back: f32,
        aim: Option<&Waiting>,
        world: &W,
        collision: &mut ClientCollision,
    ) -> (F3, [f32; 2]);

    /// The checksum of its packet data (`writePacketData`), ghost indexes `ghost_bits` wide.
    fn checksum(&self, ghost_bits: u8) -> u32;

    /// The Player the client predicted until the camera came (ghost `index` at `pos` with `velocity`, `seen` the world's last
    /// Player move): a camera that watches it goes on from there. Nothing by default.
    fn watch(&mut self, index: u32, pos: F3, velocity: F3, seen: u64) {
        let _ = (index, pos, velocity, seen);
    }

    /// The ghost the camera watches and where this frame draws it, if any.
    fn watched_render(&self, back: f32) -> Option<(u32, F3)> {
        let _ = back;
        None
    }
}

impl<W: PredictWorld> Clone for Box<dyn ControlCamera<W>> {
    fn clone(&self) -> Self {
        (**self).box_clone()
    }
}

/// The game's projectile ghosts the client runs on its own between their updates (`Projectile::processTick` on a ghost).
pub trait GhostProjectiles<W: PredictWorld>: Any {
    fn box_clone(&self) -> Box<dyn GhostProjectiles<W>>;

    /// The world's projectile ghosts taken (new ones made, gone ones dropped, the newest blocks applied), then `ticks` ticks of
    /// each: `container(near, budget)` makes the client's container for one projectile's tick, `spent` is the work the ghosts
    /// share, `stops` counts the ticks the budget stopped.
    fn advance(
        &mut self,
        ticks: u32,
        world: &W,
        container: &mut dyn FnMut(F3, u64) -> ClientCollision,
        spent: &mut u64,
        stops: &mut u64,
    );

    /// A frame `back` before the last tick: where the projectiles are drawn, and the effects their own steps made.
    fn frame(&mut self, world: &mut W, back: f32);
}

impl<W: PredictWorld> Clone for Box<dyn GhostProjectiles<W>> {
    fn clone(&self) -> Self {
        (**self).box_clone()
    }
}

/// A WheeledVehicle datablock's subclass ([`PredictGame::wheeled`]) and its steering rules.
#[derive(Debug, Clone, Default)]
pub struct Wheeling {
    pub class: Option<Rc<dyn WheeledClass>>,
    pub steer: SteerRules,
}

/// The steering rules a driver's move reads of a WheeledVehicle datablock: the auto return (on, its rate and top speed) and
/// strafe steering (on, its rate). TGE's vehicles have neither.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SteerRules {
    pub auto_return: bool,
    pub return_rate: f32,
    pub return_speed: f32,
    pub strafe: bool,
    pub strafe_rate: f32,
}

/// Torque's answers.
#[derive(Debug, Default)]
pub struct TorquePrediction;

impl<W: PredictWorld> PredictGame<W> for TorquePrediction {}

/// The game's answers as a value the prediction's state keeps (Torque's by default).
#[derive(Debug)]
pub struct GameRef<W: PredictWorld>(pub Rc<dyn PredictGame<W>>);

impl<W: PredictWorld> Clone for GameRef<W> {
    fn clone(&self) -> GameRef<W> {
        GameRef(Rc::clone(&self.0))
    }
}

impl<W: PredictWorld> Default for GameRef<W> {
    fn default() -> GameRef<W> {
        GameRef(Rc::new(TorquePrediction))
    }
}

/// A game's prediction for the world type its connection keeps, as [`crate::game::ClientGame::prediction`] hands it
/// over (behind `Any`: the engine's game is not generic).
pub struct Prediction<W: PredictWorld>(pub Rc<dyn PredictGame<W>>);

/// The prediction a game handed over for world type `W` (Torque's when it handed none, or one for another world type).
pub fn prediction_for<W: PredictWorld>(given: Option<Rc<dyn Any>>) -> Rc<dyn PredictGame<W>> {
    given
        .and_then(|a| a.downcast::<Prediction<W>>().ok())
        .map_or_else(
            || Rc::new(TorquePrediction) as Rc<dyn PredictGame<W>>,
            |p| Rc::clone(&p.0),
        )
}
