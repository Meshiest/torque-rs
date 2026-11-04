//! The engine's record world: what a client holds of the ghosts its connection was sent, the records merged block by block,
//! the datablocks, the net strings, the control object and the eye, and what the records drive (images, threads, effects).
//! It is generic over the game's classes and records ([`ClientSet`]); a game's view of the world wraps it and adds its own parts.
//!
//! Credit: the client's rules named here are TGE's, as Torque3D (MIT) carries them in `T3D/shapeBase.cpp`, `T3D/player.cpp`,
//! `T3D/projectile.cpp`, `T3D/gameBase/gameBase.cpp` and `T3D/gameBase/gameConnection.cpp` (each Copyright (c) 2012 GarageGames,
//! LLC; Copyright (C) 2015 Faust Logic, Inc.), `T3D/camera.cpp`, `T3D/vehicles/vehicle.cpp`, `T3D/vehicles/wheeledVehicle.cpp`
//! and `sim/netGhost.cpp` (each Copyright (c) 2012 GarageGames, LLC).

use std::collections::{BTreeMap, BTreeSet};

use ghosts::{NetStr, Xf};
use net::control::{ClientBlock, ControlData, ControlSection, ServerHead};
use net::ghost::GhostChange;

use super::effects::{Effect, EffectEvent};
use super::set::{
    ClientDatablock, ClientRecord, ClientSet, ClientTail, Explosion, GhostHooks, GhostPart,
    SentGhost, SentTable, SoundSlot,
};
use super::threads::Thread;

/// A ghost: the class, the record kept as [`ClientRecord::merge_update`] merges it, the game's own part, and what is resolved when a
/// record is applied.
#[derive(Debug, Clone, PartialEq)]
pub struct Ghost<S: ClientSet> {
    /// `None` for the placeholder of an empty ghost-always event.
    pub class: Option<S::Class>,
    /// The creating record's number among the client's records (1 first); 0 for a placeholder. Orders the ghosts as the client's
    /// object ids do.
    pub serial: usize,
    /// Created by a `GhostAlwaysObjectEvent` (the environment objects, mission markers).
    pub always: bool,
    /// The record after every update so far.
    pub record: Option<S::Record>,
    /// The game's own part of the ghost (the ghost reads through to it).
    pub part: S::Part,
    /// The world position of the last compressed point a record carried (a Player's or AIPlayer's move block, a vehicle's pose, a
    /// quantised projectile), resolved against the compression point of its packet. `None` for a class whose position travels
    /// as raw floats (read it from the record).
    pub position: Option<[f32; 3]>,
    /// Update records applied after the creating one.
    pub updates: u32,
}

impl<S: ClientSet> Ghost<S> {
    /// The datablock id the ghost's records named last (the game's part first).
    pub fn datablock(&self) -> Option<u32> {
        self.part
            .datablock()
            .or_else(|| self.record.as_ref()?.datablock())
    }

    /// The scale the GameBase part sent last.
    pub fn scale(&self) -> Option<[f32; 3]> {
        self.record.as_ref()?.scale()
    }

    /// Where the ghost is, as its records put it (`None` until a record carried a position, and for classes without one).
    pub fn pose(&self) -> Option<Pose> {
        self.record.as_ref()?.pose(self.position)
    }
}

impl<S: ClientSet> std::ops::Deref for Ghost<S> {
    type Target = S::Part;

    fn deref(&self) -> &S::Part {
        &self.part
    }
}

impl<S: ClientSet> std::ops::DerefMut for Ghost<S> {
    fn deref_mut(&mut self) -> &mut S::Part {
        &mut self.part
    }
}

/// Where a ghost is, in the form its record carries it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pose {
    /// `writeAffineTransform`: position, the quaternion's x, y, z and the sign of w (the StaticShape family, fxSunLight,
    /// fxLight, AudioEmitter), with the scale where the record has one.
    Affine { xf: Xf, scale: Option<[f32; 3]> },
    /// A raw 4x4 matrix (TSStatic, VehicleBlocker, ParticleEmitterNode, PhysicalZone) and its scale.
    Matrix { m: [f32; 16], scale: [f32; 3] },
    /// A position with pitch and yaw (a Camera's move block, a Player's or AIPlayer's: the angles as the record quantised them).
    Euler {
        position: [f32; 3],
        pitch: f32,
        yaw: f32,
    },
    /// A vehicle: position and the rotation quaternion `x, y, z, w` as sent.
    Quat {
        position: [f32; 3],
        rotation: [f32; 4],
    },
    /// An Item: position and the rotation block (`flag`, angle) when one was sent.
    Item {
        position: [f32; 3],
        rotation: Option<(bool, f32)>,
    },
}

/// The most wheels a vehicle look holds (`WheeledVehicleData::MaxWheels`).
pub const LOOK_WHEELS: usize = 8;

/// A WheeledVehicle the client runs, as its client draws it each frame: the steering the steered tires are turned by (`mSteering`,
/// the client's own for the vehicle it drives: the server's updates carry no steering for its controlling client), the velocity
/// `advanceTime` emits the tire dust by, and each wheel's state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VehicleLook {
    pub steering: f32,
    pub velocity: [f32; 3],
    /// At most [`LOOK_WHEELS`], in the datablock's wheel order.
    pub wheels: Vec<WheelLook>,
}

/// One wheel of a [`VehicleLook`] (`WheeledVehicle::Wheel`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WheelLook {
    /// A tire and a spring (`tire`, `spring`): a wheel without both is neither drawn nor emits.
    pub fitted: bool,
    /// The spring's extension (`extension`, 0 up to 1 hanging) and the spin (`apos`, turns in [0, 1)).
    pub extension: f32,
    pub apos: f32,
    /// In contact (`surface.contact`) and the contact point (`surface.pos`) where the frame draws the vehicle.
    pub contact: bool,
    pub point: [f32; 3],
}

/// A Projectile ghost as the client draws it this frame (`Projectile::interpolateTick`): its render transform (the point
/// between its last two ticks, turned to face along its velocity), its fade and velocity, and the ghost's birth number (a ghost index
/// is used again by later ghosts).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectileLook {
    pub birth: u64,
    /// Row-major, translation in elements 3, 7 and 11.
    pub transform: [f32; 16],
    pub velocity: [f32; 3],
    /// `mFadeValue`: 1 until `fadeDelay`, then down over `lifetime`.
    pub fade: f32,
}

impl ProjectileLook {
    pub fn position(&self) -> [f32; 3] {
        [self.transform[3], self.transform[7], self.transform[11]]
    }
}

/// The control object, the camera and the field of view as the connection's heads gave them; `T` is the game's packet data.
#[derive(Debug, Clone, PartialEq)]
pub struct Control<T> {
    /// The ghost index of the control object, from the last section that named one (a mismatch resend).
    pub object: Option<u32>,
    /// The control object's packet data from that section.
    pub tail: Option<T>,
    /// The control object's position from the last section that had one (the raw position of a matching checksum, or the resent
    /// data's position).
    pub position: Option<[f32; 3]>,
    /// Server packets whose control section was empty, matching and resent.
    pub sections: [usize; 3],
    /// The camera object the last server head named (`None` is the control object itself).
    pub camera_object: Option<u32>,
    /// The field of view the last server head carried, and the one the client's last packet carried (each `None` until sent).
    pub server_fov: Option<u8>,
    pub client_fov: Option<u8>,
    /// How many server heads carried a fov (a client notices each one).
    pub server_fov_count: u32,
    /// The fov the client's camera draws with now (its zoom, set by the client host each frame); None: [`ClientWorld::fov`].
    pub view_fov: Option<f32>,
}

impl<T> Default for Control<T> {
    fn default() -> Control<T> {
        Control {
            object: None,
            tail: None,
            position: None,
            sections: [0; 3],
            camera_object: None,
            server_fov: None,
            server_fov_count: 0,
            client_fov: None,
            view_fov: None,
        }
    }
}

/// The field of view both ends of a connection start with (degrees): `GameConnection::mCameraFov` is 90 at construction, as is a
/// ShapeBase's (`ShapeBaseData::cameraDefaultFov` by default).
pub const DEFAULT_FOV: u8 = 90;

/// A camera to draw from: position, pitch and yaw in radians as the Camera's packet data holds them, and the field of view in
/// degrees (`None` when neither direction carried one). The renderers draw from it (`torque-render-scene` re-exports it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Eye {
    pub position: [f32; 3],
    pub pitch: f32,
    pub yaw: f32,
    /// Degrees; `None` when the source carried none.
    pub fov: Option<u8>,
}

impl From<&Eye> for Eye {
    fn from(e: &Eye) -> Eye {
        *e
    }
}

/// The most entries [`ClientWorld::ghost_log`] keeps between takes; past it the log is dropped and marked overflowed (a reader then walks
/// the ghost table once).
pub const MAX_GHOST_LOG: usize = 1 << 16;

/// One ghost made or deleted, or an update that changed its datablock, with its place in the birth order
/// ([`ClientWorld::birth_order`]). The client repoints the object then (`GameBase::unpackUpdate` calls `setDataBlock` when the id
/// differs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhostLogEntry {
    Born {
        index: u32,
        birth: u64,
    },
    Gone {
        index: u32,
        birth: u64,
    },
    Repointed {
        index: u32,
        birth: u64,
        datablock: u32,
    },
}

/// The ghosts made and deleted since the last take, in order; `overflowed` when entries were lost (a reset, or more than
/// [`MAX_GHOST_LOG`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GhostLog {
    pub entries: Vec<GhostLogEntry>,
    pub overflowed: bool,
}

impl GhostLog {
    pub fn push(&mut self, entry: GhostLogEntry) {
        if self.overflowed {
            return;
        }
        if self.entries.len() >= MAX_GHOST_LOG {
            self.entries = Vec::new();
            self.overflowed = true;
            return;
        }
        self.entries.push(entry);
    }
}

/// The most sequence fields of one `TSShapeConstructor` read (the client's import of its sequences).
pub const MAX_CONSTRUCTOR_SEQUENCES: usize = 128;

/// A value derived from the world's datablocks: computed when first asked, dropped when a datablock event arrives, and never part of
/// a comparison of two worlds (it is what their datablocks give).
#[derive(Debug, Default)]
pub struct Derived<T>(pub std::sync::Mutex<Option<T>>);

impl<T> Derived<T> {
    /// The value (none until derived), held while the guard lives.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Option<T>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<T: Clone> Clone for Derived<T> {
    fn clone(&self) -> Derived<T> {
        Derived(std::sync::Mutex::new(self.lock().clone()))
    }
}

impl<T> PartialEq for Derived<T> {
    fn eq(&self, _: &Derived<T>) -> bool {
        true
    }
}

/// Every datablock's `baseShape` (a `TSShapeConstructor`'s; lower case, `\` as `/`) and its `seqName` texts, the first datablock
/// by id that names a shape winning; and how many datablocks the world had when it was built (a caller that changes
/// [`ClientWorld::datablocks`] directly makes a rebuild when the count changes; [`ClientWorld::datablocks_changed`] for anything else).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConstructorIndex {
    datablocks: usize,
    by_shape: std::collections::HashMap<String, Vec<Vec<u8>>>,
}

impl ConstructorIndex {
    pub fn of<D: ClientDatablock>(datablocks: &BTreeMap<u32, D>) -> ConstructorIndex {
        let mut by_shape = std::collections::HashMap::new();
        for d in datablocks.values() {
            let Some(base) = d.base_shape() else {
                continue;
            };
            let key = String::from_utf8_lossy(base)
                .replace('\\', "/")
                .to_ascii_lowercase();
            by_shape
                .entry(key)
                .or_insert_with(|| d.sequences(MAX_CONSTRUCTOR_SEQUENCES));
        }
        ConstructorIndex {
            datablocks: datablocks.len(),
            by_shape,
        }
    }
}

/// The most arguments, and bytes of each, a kept command keeps ([`ClientSet::KEPT_COMMANDS`]).
pub const MAX_KEPT_ARGS: usize = 8;
pub const MAX_KEPT_ARG_BYTES: usize = 255;

/// The ghost table by class (each class's ghost indices in order) and every ghost's updates summed, kept by
/// [`ClientWorld::apply_ghosts`]: [`ClientWorld::of_class`] and [`ClientWorld::updates_total`] read it instead of walking every ghost (a large
/// mission has many ghosts, and a frame asks for the Sky, the Sun, the lights and every shape class in turn). A table changed by
/// hand (tests and tools insert ghosts) is seen by its size and walked as before.
#[derive(Debug, Clone, PartialEq)]
pub struct GhostIndex<S: ClientSet> {
    pub by_class: std::collections::HashMap<Option<S::Class>, BTreeSet<u32>>,
    pub count: usize,
    pub updates: u64,
    /// The share of `updates` of the ghosts whose part is [`GhostPart::summed_apart`] (wrapping; a ghost whose part changed after
    /// it was counted leaves a constant behind, which the sums' changes do not mind).
    pub part_updates: u64,
    /// The ghosts whose part is [`GhostPart::flagged`].
    pub flagged: BTreeSet<u32>,
}

impl<S: ClientSet> Default for GhostIndex<S> {
    fn default() -> GhostIndex<S> {
        GhostIndex {
            by_class: Default::default(),
            count: 0,
            updates: 0,
            part_updates: 0,
            flagged: BTreeSet::new(),
        }
    }
}

impl<S: ClientSet> GhostIndex<S> {
    pub fn insert(&mut self, index: u32, g: &Ghost<S>) {
        if self.by_class.entry(g.class).or_default().insert(index) {
            self.count += 1;
        }
        self.updates = self.updates.wrapping_add(u64::from(g.updates));
        if g.part.summed_apart() {
            self.part_updates = self.part_updates.wrapping_add(u64::from(g.updates));
        }
        self.part_changed(index, g);
    }

    /// The ghost's part changed: the flagged set follows it.
    pub fn part_changed(&mut self, index: u32, g: &Ghost<S>) {
        if g.part.flagged() {
            self.flagged.insert(index);
        } else {
            self.flagged.remove(&index);
        }
    }

    pub fn remove(&mut self, index: u32, g: &Ghost<S>) {
        if self
            .by_class
            .get_mut(&g.class)
            .is_some_and(|s| s.remove(&index))
        {
            self.count -= 1;
        }
        self.updates = self.updates.wrapping_sub(u64::from(g.updates));
        if g.part.summed_apart() {
            self.part_updates = self.part_updates.wrapping_sub(u64::from(g.updates));
        }
        self.flagged.remove(&index);
    }

    pub fn clear(&mut self) {
        *self = GhostIndex::default();
    }
}

/// The engine's half of the client's world: the ghosts and their records, the datablocks, the net strings, the
/// control object and the eye, the images, threads and effects the records drive. A game's view wraps it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientWorld<S: ClientSet> {
    /// Datablocks by id, as their events carried them (an event without a body leaves what the client had).
    pub datablocks: BTreeMap<u32, S::Datablock>,

    /// The server-to-client net strings by slot.
    pub strings: BTreeMap<u8, Vec<u8>>,

    /// Every ghost by index.
    pub ghosts: BTreeMap<u32, Ghost<S>>,

    pub control: Control<S::Tail>,

    /// Ghost resets the client ran (connection message 2).
    pub resets: usize,

    /// The feed's record (datagram index) the world was last changed by.
    pub last_record: Option<usize>,

    /// Ghosts the table named that the world could not follow: a change for an index the table no longer held (created and deleted
    /// in one packet), and updates of an index the world did not have.
    pub skipped_changes: usize,

    /// Client-to-server net strings, for naming the client's remote commands (the game's client command markers).
    pub client_strings: BTreeMap<u8, Vec<u8>>,

    /// Each live ghost's place in the order the client made them (`births`, counted over the world's life): the client makes a
    /// ghost's object as it reads the ghost (`ghostReadPacket`), so its connection holds them in this order (a client host's script
    /// objects follow it).
    pub birth_order: BTreeMap<u32, u64>,

    pub births: u64,

    /// The ghosts by class and their updates summed, kept as [`ClientWorld::apply_ghosts`] changes the table ([`GhostIndex`]).
    pub ghost_index: GhostIndex<S>,

    /// The ghosts made and deleted since the last [`ClientWorld::take_ghost_log`], in order (the client host's script objects follow it
    /// without walking the table), and the updates that changed a ghost's datablock; any other update logs nothing.
    pub ghost_log: GhostLog,

    /// The last server-to-client remote command of each name the world keeps ([`ClientSet::KEPT_COMMANDS`], lower case): its
    /// arguments as the client's script receives them (a tag through the string table), each at most [`MAX_KEPT_ARG_BYTES`], at most
    /// [`MAX_KEPT_ARGS`]: the client's state that only a command sets. Only the names the set keeps, so a server naming a new
    /// command every time cannot grow it.
    pub commands: BTreeMap<Vec<u8>, Vec<Vec<u8>>>,

    /// The client's own camera (the spectator flying free while its control object is not a Camera): [`ClientWorld::eye`] answers it
    /// when set. No packet sets it; the server still scopes ghosts from its control object.
    pub view: Option<Eye>,

    /// The control Player's own eye point where the client host predicts it (first person's eye; in third person the camera
    /// sits elsewhere; a game may place things there). The prediction sets it; a world fed only by records never does.
    pub control_eye: Option<[f32; 3]>,

    /// The control object is not drawn: the client's first-person view of its own Player (the client host's prediction sets it;
    /// `GameConnection::isFirstPerson`, `mCameraPos` 0). A world fed only by records never sets it.
    pub hide_control: bool,

    /// The control object's transform as the client predicts it: the client host's prediction sets it every tick, and the
    /// renderer draws the control Player and its images there instead of at its ghost's (for the controlling client the server's
    /// updates carry no position: `Player::packUpdate`'s control branch, so the ghost stays where it was first sent). The client
    /// renders its control object from its own predicted render transform (`Player::interpolateTick`, `setRenderPosition`). A
    /// world fed only by records never sets it.
    pub control_render: Option<Pose>,

    /// The vehicles the client runs itself, where it draws them (`Vehicle::interpolateTick`'s render position:
    /// between the last tick's and the one before): the vehicle the client drives (for the controlling client the server's updates
    /// carry no position, as a Player's) and the ghost vehicles it runs on its own. The client host's prediction sets it; a world fed only by
    /// records never does (the vehicles are drawn where their ghosts were sent).
    pub vehicle_render: BTreeMap<u32, Pose>,

    /// The other ghosts the client runs itself, where it draws them: the Player its orbit camera watches (a dead Player, at
    /// its render position between ticks, `Player::interpolateTick`, the place the camera orbits). The client host's
    /// prediction sets it; a world fed only by records never does.
    pub ghost_render: BTreeMap<u32, Pose>,

    /// The vehicles the client runs itself as they are drawn and emit this frame ([`VehicleLook`]: the steering, the velocity and
    /// each wheel's spin, extension and contact). Set with [`ClientWorld::vehicle_render`]; a world fed only by records never sets it (the tires
    /// are drawn from the record's steering, still and let down to the ground plane).
    pub vehicle_look: BTreeMap<u32, VehicleLook>,

    /// The Projectile ghosts the client runs, as this frame draws them ([`ProjectileLook`]): the client host's projectile step sets it
    /// every frame; a world fed only by records never does (its Projectiles are not drawn).
    pub projectile_render: BTreeMap<u32, ProjectileLook>,

    /// The live client animates its shapes by the frame's time: the Players' and AIPlayers' action threads advance as the client
    /// advances them every frame, and the live window draws every moving shape each frame. A world fed only by records never sets it
    /// (its frames pose each thread where the record puts it).
    pub animate: bool,

    /// The client's clock for that (ms): its simulation time at the pass that last set it (the window's frames run it by the wall).
    pub anim_ms: u32,

    /// The vehicles' ghost updates as each one came (the update's own record, unmerged, and its point resolved): the client runs a
    /// vehicle nobody drives forward from each (`Vehicle::unpackUpdate`'s warp), so it reads them in order by their sequence
    /// number. At most [`VEHICLE_UPDATES`] are kept.
    pub vehicle_updates: std::collections::VecDeque<VehicleUpdate<S>>,

    pub vehicle_update_seq: u64,

    /// The Camera ghosts' updates that moved one the client does not control (`Camera::unpackUpdate`: `flag(controlled)` clear,
    /// then the position, pitch and yaw it `setPosition`s): (sequence number, ghost index, x, y, z, pitch, yaw), in order, at
    /// most [`CAMERA_MOVES`]. A client whose control object that Camera still is runs on from there (a server may move the Camera a packet
    /// before it changes the client's control object).
    pub camera_moves: std::collections::VecDeque<(u64, u32, [f32; 5])>,

    pub camera_move_seq: u64,

    /// The Player and AIPlayer ghosts' move blocks as each came (unmerged), in order, at most [`PLAYER_MOVES`]: the client runs the
    /// one its orbit camera watches as its ghost does (`Player::unpackUpdate`'s warp).
    pub player_moves: std::collections::VecDeque<PlayerMove>,

    pub player_move_seq: u64,

    /// The mounted images of the ShapeBase ghosts by ghost index, as the client's `unpackUpdate` and image machine keep them
    /// ([`super::images`]). Run to [`ClientWorld::clock_ms`] when read ([`ClientWorld::mounted_images`]).
    pub images: BTreeMap<u32, super::images::Slots>,

    /// The four slots' fire counts of each ShapeBase ghost, kept while a slot is empty or changes image (the slot keeps them
    /// through `setImage`; [`super::images::apply`]).
    pub image_fire_counts: BTreeMap<u32, [u32; super::images::IMAGE_SLOTS]>,

    /// The script threads of the ShapeBase ghosts by ghost index ([`super::threads`]).
    pub threads: BTreeMap<u32, super::threads::Slots>,

    /// The client's clock (ms): the time of the datagram the world was last changed by, as the feed set it ([`ClientWorld::set_clock`]:
    /// the feed's datagram times, the live feed's clock). `None` when the feed gives none: the images' ticks do not run then.
    pub clock_ms: Option<u32>,

    /// The image datablocks' states as parsed once (dropped when a datablock event replaces the datablock).
    pub image_states_cache: BTreeMap<u32, Option<super::images::ImageStates>>,

    /// The constructors' sequences by base shape ([`ClientWorld::constructor_sequences`]), built when first asked and dropped when a
    /// datablock arrives.
    pub constructors: Derived<ConstructorIndex>,

    /// The image ticks left to run at this clock step ([`super::images::TICK_BUDGET`] each step) and the ghost the last step
    /// reached.
    pub image_budget: u32,

    pub image_cursor: u32,

    /// The image ticks run, skipped and deferred so far.
    pub image_ticks: super::images::TickStats,

    /// The effects in order ([`super::effects`]): at most [`super::effects::MAX_EFFECTS`], the oldest dropped and counted;
    /// the next number; the projectile ghosts that exploded already (one explosion each).
    pub effects: std::collections::VecDeque<EffectEvent<S::Effect>>,

    pub effect_seq: u64,

    pub effects_dropped: u64,

    pub exploded: BTreeSet<u32>,

    /// `$pref::Player::renderMyItems` is 0: the control Player's images are not drawn in its first-person view (the client host
    /// sets it from the pref, 1 by default).
    pub hide_own_items: bool,

    /// The view distance's cap: the scene's visible distance is the Sky's or this, whichever is less, the fog distance scaled with
    /// it. The client host sets it each frame by its game's rule. None: no cap.
    pub max_view_distance: Option<f32>,
}

// The derive would ask the set's classes and records for defaults they need not have.
impl<S: ClientSet> Default for ClientWorld<S> {
    fn default() -> ClientWorld<S> {
        ClientWorld {
            datablocks: Default::default(),
            strings: Default::default(),
            ghosts: Default::default(),
            control: Default::default(),
            resets: Default::default(),
            last_record: Default::default(),
            skipped_changes: Default::default(),
            client_strings: Default::default(),
            birth_order: Default::default(),
            births: Default::default(),
            ghost_index: Default::default(),
            ghost_log: Default::default(),
            commands: Default::default(),
            view: Default::default(),
            control_eye: Default::default(),
            hide_control: Default::default(),
            control_render: Default::default(),
            vehicle_render: Default::default(),
            ghost_render: Default::default(),
            vehicle_look: Default::default(),
            projectile_render: Default::default(),
            animate: Default::default(),
            anim_ms: Default::default(),
            vehicle_updates: Default::default(),
            vehicle_update_seq: Default::default(),
            camera_moves: Default::default(),
            camera_move_seq: Default::default(),
            player_moves: Default::default(),
            player_move_seq: Default::default(),
            images: Default::default(),
            image_fire_counts: Default::default(),
            threads: Default::default(),
            clock_ms: Default::default(),
            image_states_cache: Default::default(),
            constructors: Default::default(),
            image_budget: Default::default(),
            image_cursor: Default::default(),
            image_ticks: Default::default(),
            effects: Default::default(),
            effect_seq: Default::default(),
            effects_dropped: Default::default(),
            exploded: Default::default(),
            hide_own_items: Default::default(),
            max_view_distance: Default::default(),
        }
    }
}

/// The most Player moves [`ClientWorld::player_moves`] keeps.
pub const PLAYER_MOVES: usize = 1024;

/// One Player ghost's move block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerMove {
    pub seq: u64,
    pub index: u32,
    /// The position resolved against the packet's compression point.
    pub position: [f32; 3],
    /// The velocity block's vector (zero when it was not sent).
    pub velocity: [f32; 3],
    /// `flag(!(mask & NoWarpMask))`: the client may warp to it.
    pub warp: bool,
}

/// The most Camera moves [`ClientWorld::camera_moves`] keeps.
pub const CAMERA_MOVES: usize = 64;

/// The most vehicle updates [`ClientWorld::vehicle_updates`] keeps.
pub const VEHICLE_UPDATES: usize = 1024;

/// One vehicle ghost's update.
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleUpdate<S: ClientSet> {
    pub seq: u64,
    pub index: u32,
    pub record: S::Record,
    pub position: Option<[f32; 3]>,
}

impl<S: ClientSet> ClientWorld<S> {
    /// The ghost log since the last take (see [`ClientWorld::ghost_log`]), emptied.
    pub fn take_ghost_log(&mut self) -> GhostLog {
        std::mem::take(&mut self.ghost_log)
    }

    /// A net string's text: a slot through the server's string table, a text as it is; `None` for null or an unmapped slot.
    pub fn net_string<'a>(&'a self, s: &'a NetStr) -> Option<&'a [u8]> {
        match s {
            NetStr::Null => None,
            NetStr::Slot(slot) => self.strings.get(slot).map(Vec::as_slice),
            NetStr::Text(t) => Some(t),
        }
    }

    /// A datablock's string field (its first value).
    pub fn datablock_text(&self, id: u32, field: &str) -> Option<&[u8]> {
        self.datablocks.get(&id)?.text(field)
    }

    /// The ghosts of one class.
    pub fn of_class(&self, class: S::Class) -> impl Iterator<Item = (u32, &Ghost<S>)> {
        // The class's own list when the index holds the table ([`GhostIndex`]), else every ghost.
        let indexed = self.indexed();
        let listed = indexed
            .then(|| self.ghost_index.by_class.get(&Some(class)))
            .flatten()
            .into_iter()
            .flatten()
            .filter_map(move |i| {
                self.ghosts
                    .get(i)
                    .filter(|g| g.class == Some(class))
                    .map(|g| (*i, g))
            });
        let walked = (!indexed)
            .then(|| {
                self.ghosts
                    .iter()
                    .filter(move |(_, g)| g.class == Some(class))
                    .map(|(i, g)| (*i, g))
            })
            .into_iter()
            .flatten();
        listed.chain(walked)
    }

    /// Whether [`ClientWorld::ghost_index`] holds every ghost (the table was changed only by [`ClientWorld::apply_ghosts`]).
    pub fn indexed(&self) -> bool {
        self.ghost_index.count == self.ghosts.len()
    }

    /// Every ghost's updates summed (kept, when the index holds the table; else summed now).
    pub fn updates_total(&self) -> u64 {
        if self.indexed() {
            self.ghost_index.updates
        } else {
            self.ghosts.values().map(|g| u64::from(g.updates)).sum()
        }
    }

    /// The live ghosts by class name (`placeholder` for an empty ghost-always event's object).
    pub fn census(&self) -> BTreeMap<&'static str, usize> {
        let mut out = BTreeMap::new();
        for g in self.ghosts.values() {
            *out.entry(g.class.map_or("placeholder", S::class_name))
                .or_insert(0) += 1;
        }
        out
    }

    /// The ghosts the client counts as active (`getGhostsActive`): every ghost but those a ghost-always event created.
    pub fn ordinary_ghosts(&self) -> usize {
        self.ghosts.values().filter(|g| !g.always).count()
    }

    /// The camera the client draws from: [`ClientWorld::view`] when the client set one, else, when its control object is a Camera whose
    /// data a section resent, the data's position and angles. The client's own moves since that section are not applied (a fixed-camera scene sends none); a Player control
    /// object's eye is worked out the way the client does it.
    pub fn eye(&self) -> Option<Eye> {
        if let Some(view) = self.view {
            return Some(view);
        }
        let (position, pitch, yaw) = self.control.tail.as_ref()?.camera()?;
        Some(Eye {
            position: self.control.position.unwrap_or(position),
            pitch,
            yaw,
            fov: self.control.server_fov.or(self.control.client_fov),
        })
    }

    /// The field of view the client draws with (degrees): the last one a server or client head carried, else [`DEFAULT_FOV`]. A
    /// head carries one only when its integer part changed (`setControlCameraFov` arms the update flag on `(int)new != (int)old`),
    /// so a connection that never sent one is still at the 90 both ends start with.
    pub fn fov(&self) -> u8 {
        self.control
            .server_fov
            .or(self.control.client_fov)
            .unwrap_or(DEFAULT_FOV)
    }

    /// The fov the frame is drawn with (degrees): the client's own camera fov when it keeps one ([`Control::view_fov`], its zoom),
    /// else [`ClientWorld::fov`].
    pub fn fov_deg(&self) -> f32 {
        self.control
            .view_fov
            .unwrap_or_else(|| f32::from(self.fov()))
    }

    // ---- the input

    /// A server packet's head.
    pub fn apply_server_head(&mut self, record: usize, head: &ServerHead<Box<dyn ControlData>>) {
        self.last_record = Some(record);
        let c = &mut self.control;
        match &head.control {
            ControlSection::None => c.sections[0] += 1,
            ControlSection::Matching { position } => {
                c.sections[1] += 1;
                c.position = Some(*position);
            }
            ControlSection::Resend { index, data } => {
                c.sections[2] += 1;
                c.object = Some(*index);
                c.position = Some(data.position());
                c.tail = Some(S::tail(data.as_ref()));
            }
        }
        c.camera_object = head.camera_object;
        if head.fov.is_some() {
            c.server_fov = head.fov;
            c.server_fov_count = c.server_fov_count.wrapping_add(1);
        }
    }

    /// A client packet's head.
    pub fn apply_client_block(&mut self, record: usize, block: &ClientBlock) {
        let _ = record;
        if block.fov.is_some() {
            self.control.client_fov = block.fov;
        }
    }

    /// Each sound slot an update carried, as `updateAudioState` takes it: the slot's sound stopped, and one started when it
    /// plays a profile.
    fn apply_sounds(&mut self, index: u32, sent: &[Option<SoundSlot>; 4]) {
        for (slot, s) in sent.iter().enumerate() {
            let Some(s) = s else { continue };
            let profile = s.playing.then_some(s.profile);
            self.push_effect(Effect::ShapeSound {
                ghost: index,
                slot,
                profile,
            });
        }
    }

    fn apply_threads(&mut self, index: u32, sent: &[Option<Thread>; 4]) {
        let now = self.clock_ms;
        let slots = self.threads.entry(index).or_default();
        S::apply_threads(slots, sent, now);
    }

    /// The sequence texts of the client's constructor for `shape_path` (any case, `\` or `/`), if a datablock names it as its
    /// `baseShape`: the first by id, at most [`MAX_CONSTRUCTOR_SEQUENCES`]. An index built once per change of the datablocks;
    /// the shapes a frame draws and the eyes a tick places asked every datablock for each object.
    pub fn constructor_sequences(&self, shape_path: &str) -> Option<Vec<Vec<u8>>> {
        let key = shape_path.replace('\\', "/").to_ascii_lowercase();
        let mut cache = self.constructors.lock();
        if cache
            .as_ref()
            .is_none_or(|c| c.datablocks != self.datablocks.len())
        {
            *cache = Some(ConstructorIndex::of(&self.datablocks));
        }
        cache.as_ref()?.by_shape.get(&key).cloned()
    }

    /// Drops what the world derived from its datablocks (call it after changing [`ClientWorld::datablocks`] by hand).
    pub fn datablocks_changed(&mut self) {
        *self
            .constructors
            .0
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// The image states of datablock `id` (`None` when it is not an image datablock the client has), parsed once.
    pub fn image_states(&mut self, id: u32) -> Option<super::images::ImageStates> {
        if let Some(cached) = self.image_states_cache.get(&id) {
            return cached.clone();
        }
        let parsed = self.datablocks.get(&id)?.image_states();
        self.image_states_cache.insert(id, parsed.clone());
        parsed
    }

    /// Parses the states of every datablock `ids` names into the cache (so a closure over the cache can answer them).
    fn cache_image_states(&mut self, ids: impl IntoIterator<Item = u32>) {
        for id in ids {
            let _ = self.image_states(id);
        }
    }

    /// Adds an effect at the world's clock (the oldest dropped past [`super::effects::MAX_EFFECTS`]).
    pub fn push_effect(&mut self, effect: Effect<S::Effect>) {
        self.effect_seq += 1;
        if self.effects.len() >= super::effects::MAX_EFFECTS {
            self.effects.pop_front();
            self.effects_dropped += 1;
        }
        self.effects.push_back(super::effects::EffectEvent {
            seq: self.effect_seq,
            ms: self.clock_ms,
            effect,
        });
    }

    /// The effects after number `seq`, in order.
    pub fn effects_after(&self, seq: u64) -> impl Iterator<Item = &EffectEvent<S::Effect>> {
        self.effects.iter().filter(move |e| e.seq > seq)
    }

    /// A Player datablock's jump sound, where a game's datablocks have one ([`ClientDatablock::jump_sound`]): the profile's id.
    pub fn jump_sound(&self, db: u32) -> Option<u32> {
        self.datablocks.get(&db)?.jump_sound()
    }

    /// A projectile record's explosion block (`Projectile::unpackUpdate`: `explode(position, normal, type)` once): the
    /// datablock's `Explosion` at the point.
    fn projectile_explosion(&mut self, index: u32, x: Explosion, datablock: Option<u32>) {
        if !self.exploded.insert(index) {
            return;
        }
        let explosion = datablock.and_then(|d| self.datablocks.get(&d)?.explosion());
        if let Some(datablock) = explosion {
            self.push_effect(Effect::Explosion {
                ghost: index,
                datablock,
                position: x.position,
                normal: x.normal,
            });
        }
    }

    /// The ghost `index` is mounted on (`ShapeBase::unpackUpdate`'s mount block: the mount's ghost index and node), when its
    /// record says so and the mount is a ghost the client holds.
    pub fn mount_of(&self, index: u32) -> Option<(u32, u32)> {
        let (m, node) = self.ghosts.get(&index)?.record.as_ref()?.mount()?;
        self.ghosts.contains_key(&m).then_some((m, node))
    }

    /// The images mounted on ghost `index` as the world holds them (run to the clock by [`ClientWorld::set_clock`]): (slot, image).
    pub fn mounted_images(&self, index: u32) -> Vec<(usize, super::images::MountedImage)> {
        let Some(slots) = self.images.get(&index) else {
            return Vec::new();
        };
        slots
            .iter()
            .enumerate()
            .filter_map(|(i, m)| Some((i, m.clone()?)))
            .collect()
    }

    /// The ghost table changed: `changes` in order, `table` as it is now, `cp` the compression point the records were read against;
    /// `hooks` runs the game's own steps around each change.
    pub fn apply_ghosts<T: SentTable<S>>(
        &mut self,
        record: usize,
        changes: &[GhostChange],
        table: &T,
        cp: [f32; 3],
        hooks: &mut impl GhostHooks<S, T::Ghost>,
    ) {
        self.last_record = Some(record);
        for change in changes {
            match *change {
                GhostChange::Cleared => {
                    let gone: Vec<u32> = self.ghosts.keys().copied().collect();
                    for ghost in gone {
                        self.push_effect(Effect::GhostGone { ghost });
                    }
                    self.ghosts.clear();
                    self.ghost_index.clear();
                    self.birth_order.clear();
                    // A reset forgets every ghost: the reader walks the (empty) table instead of a log entry per ghost.
                    self.ghost_log.entries.clear();
                    self.ghost_log.overflowed = true;
                    self.images.clear();
                    self.image_fire_counts.clear();
                    self.threads.clear();
                    self.exploded.clear();
                    self.resets += 1;
                    hooks.cleared(self);
                }
                GhostChange::Deleted(index) => {
                    let gone = self.ghosts.remove(&index);
                    if let Some(gone) = &gone {
                        self.ghost_index.remove(index, gone);
                        self.push_effect(Effect::GhostGone { ghost: index });
                    }
                    if let Some(birth) = self.birth_order.remove(&index) {
                        self.ghost_log.push(GhostLogEntry::Gone { index, birth });
                    }
                    self.images.remove(&index);
                    self.image_fire_counts.remove(&index);
                    self.threads.remove(&index);
                    self.exploded.remove(&index);
                    hooks.deleted(self, index, gone.as_ref());
                }
                GhostChange::Created(index) => {
                    let Some(o) = table.ghost(index) else {
                        self.skipped_changes += 1;
                        continue;
                    };
                    let sent = o.record();
                    let position = sent.and_then(|r| r.point(cp));
                    self.images.remove(&index);
                    self.image_fire_counts.remove(&index);
                    self.threads.remove(&index);
                    self.exploded.remove(&index);
                    if let Some(images) = sent.and_then(ClientRecord::images) {
                        self.apply_images(index, &images, false);
                    }
                    if let Some(x) = sent.and_then(ClientRecord::explosion) {
                        self.projectile_explosion(index, x, sent.and_then(ClientRecord::datablock));
                    }
                    if let Some(threads) = sent.and_then(ClientRecord::threads) {
                        self.apply_threads(index, &threads);
                    }
                    if let Some(sounds) = sent.and_then(ClientRecord::sounds) {
                        self.apply_sounds(index, &sounds);
                    }
                    if let Some(birth) = self.birth_order.get(&index).copied() {
                        self.ghost_log.push(GhostLogEntry::Gone { index, birth });
                    }
                    self.births += 1;
                    self.birth_order.insert(index, self.births);
                    self.ghost_log.push(GhostLogEntry::Born {
                        index,
                        birth: self.births,
                    });
                    let made = Ghost {
                        class: o.class(),
                        serial: o.serial(),
                        always: false,
                        record: sent.cloned(),
                        part: hooks.part(o),
                        position,
                        updates: o.updates(),
                    };
                    let old = self.ghosts.insert(index, made);
                    if let Some(old) = &old {
                        self.ghost_index.remove(index, old);
                    }
                    if let Some(g) = self.ghosts.get(&index) {
                        self.ghost_index.insert(index, g);
                    }
                    hooks.created(self, index, o, old.as_ref());
                }
                GhostChange::Updated(index) => {
                    let sent = table.ghost(index).and_then(SentGhost::record);
                    let held = self.ghosts.contains_key(&index);
                    if held && let Some(images) = sent.and_then(ClientRecord::images) {
                        self.apply_images(index, &images, true);
                    }
                    if held && let Some(threads) = sent.and_then(ClientRecord::threads) {
                        self.apply_threads(index, &threads);
                    }
                    if held && let Some(sounds) = sent.and_then(ClientRecord::sounds) {
                        self.apply_sounds(index, &sounds);
                    }
                    if held && let Some(x) = sent.and_then(ClientRecord::explosion) {
                        let datablock = self
                            .ghosts
                            .get(&index)
                            .and_then(|g| g.datablock())
                            .or(sent.and_then(ClientRecord::datablock));
                        self.projectile_explosion(index, x, datablock);
                    }
                    // A move block that says the Player jumped (a game's records may carry it; TGE's do not) plays its
                    // datablock's jump sound at the Player.
                    if let Some(r) = sent
                        && r.move_block().is_some_and(|m| m.jumped)
                        && let Some(position) = r
                            .point(cp)
                            .or_else(|| self.ghosts.get(&index).and_then(|g| g.position))
                        && let Some(profile) = self
                            .ghosts
                            .get(&index)
                            .and_then(|g| g.datablock())
                            .and_then(|db| self.jump_sound(db))
                    {
                        self.push_effect(Effect::Sound3D {
                            profile,
                            position,
                            cone: None,
                        });
                    }
                    let (Some(o), Some(g)) = (table.ghost(index), self.ghosts.get_mut(&index))
                    else {
                        self.skipped_changes += 1;
                        continue;
                    };
                    let datablock_before = g.datablock();
                    if let Some(sent) = o.record() {
                        let point = sent.point(cp);
                        if let Some(p) = point {
                            g.position = Some(p);
                        }
                        if let (Some(m), Some(position)) = (sent.move_block(), point) {
                            self.player_move_seq += 1;
                            if self.player_moves.len() >= PLAYER_MOVES {
                                self.player_moves.pop_front();
                            }
                            self.player_moves.push_back(PlayerMove {
                                seq: self.player_move_seq,
                                index,
                                position,
                                velocity: m.velocity,
                                warp: m.warp,
                            });
                        }
                        if let Some(m) = sent.camera_move() {
                            self.camera_move_seq += 1;
                            if self.camera_moves.len() >= CAMERA_MOVES {
                                self.camera_moves.pop_front();
                            }
                            self.camera_moves
                                .push_back((self.camera_move_seq, index, m));
                        }
                        if o.class().is_some_and(S::is_vehicle) {
                            self.vehicle_update_seq += 1;
                            if self.vehicle_updates.len() >= VEHICLE_UPDATES {
                                self.vehicle_updates.pop_front();
                            }
                            self.vehicle_updates.push_back(VehicleUpdate {
                                seq: self.vehicle_update_seq,
                                index,
                                record: sent.clone(),
                                position: point,
                            });
                        }
                        match &mut g.record {
                            Some(kept) => kept.merge_update(sent),
                            kept => *kept = Some(sent.clone()),
                        }
                    }
                    hooks.updating(index, g, o);
                    self.ghost_index.part_changed(index, g);
                    self.ghost_index.updates = self
                        .ghost_index
                        .updates
                        .wrapping_add(u64::from(o.updates()))
                        .wrapping_sub(u64::from(g.updates));
                    if g.part.summed_apart() {
                        self.ghost_index.part_updates = self
                            .ghost_index
                            .part_updates
                            .wrapping_add(u64::from(o.updates()))
                            .wrapping_sub(u64::from(g.updates));
                    }
                    g.updates = o.updates();
                    let repointed = g.datablock().filter(|d| Some(*d) != datablock_before);
                    if let (Some(datablock), Some(&birth)) =
                        (repointed, self.birth_order.get(&index))
                    {
                        self.ghost_log.push(GhostLogEntry::Repointed {
                            index,
                            birth,
                            datablock,
                        });
                    }
                    hooks.updated(self, index, o);
                }
            }
        }
    }

    /// One more record of the feed changed the world.
    pub fn note_record(&mut self, record: usize) {
        self.last_record = Some(record);
    }

    /// A client-to-server net string.
    pub fn apply_client_string(&mut self, slot: u8, text: &[u8]) {
        self.client_strings.insert(slot, text.to_vec());
    }

    /// A server-to-client net string.
    pub fn apply_net_string(&mut self, slot: u8, text: &[u8]) {
        self.strings.insert(slot, text.to_vec());
    }

    /// A server-to-client remote command: kept when the world keeps its name ([`ClientSet::KEPT_COMMANDS`]).
    pub fn apply_remote(&mut self, args: &[net::remote::RemoteArg]) {
        let text = |a: &net::remote::RemoteArg| {
            let mut t = match a {
                net::remote::RemoteArg::Tag(slot) => {
                    self.strings.get(slot).cloned().unwrap_or_default()
                }
                other => other.to_script(),
            };
            t.truncate(MAX_KEPT_ARG_BYTES);
            t
        };
        // The name first: a command nothing reads is not expanded or kept.
        let name = args.first().map(|a| text(a).to_ascii_lowercase());
        if let Some(name) = name
            && S::KEPT_COMMANDS.contains(&name.as_slice())
        {
            let args = args.iter().skip(1).take(MAX_KEPT_ARGS).map(text).collect();
            self.commands.insert(name, args);
        }
    }

    /// A datablock event's datablock.
    pub fn apply_datablock(&mut self, id: u32, datablock: S::Datablock) {
        self.image_states_cache.remove(&id);
        self.datablocks_changed();
        self.datablocks.insert(id, datablock);
    }

    /// A ghost-always event for ghost `index`.
    pub fn apply_ghost_always(&mut self, index: u32) {
        if let Some(g) = self.ghosts.get_mut(&index) {
            g.always = true;
        }
    }

    /// The feed's clock moves to `ms` ([`ClientWorld::clock_ms`]): on a new time the image ticks get a fresh budget and every slot is run
    /// towards it ([`super::images::advance_all`]; what the budget does not reach is deferred to a later step).
    pub fn set_clock(&mut self, ms: u32) {
        if self.clock_ms == Some(ms) {
            return;
        }
        self.clock_ms = Some(ms);
        self.image_budget = super::images::TICK_BUDGET;
        if self.images.is_empty() {
            return;
        }
        let ids: Vec<u32> = self
            .images
            .values()
            .flat_map(|s| s.iter().flatten().map(|m| m.datablock))
            .collect();
        self.cache_image_states(ids);
        let cache = &self.image_states_cache;
        let states = |id: u32| cache.get(&id).cloned().flatten();
        super::images::advance_all(
            S::image_rules(),
            &mut self.images,
            &mut self.image_cursor,
            &states,
            Some(ms),
            &mut self.image_budget,
            &mut self.image_ticks,
        );
        self.take_image_events(None);
    }

    fn apply_images(
        &mut self,
        index: u32,
        sent: &[Option<super::images::ImageSlot>; 4],
        added: bool,
    ) {
        let now = self.clock_ms;
        let mut ids: Vec<u32> = sent.iter().flatten().filter_map(|s| s.datablock).collect();
        if let Some(slots) = self.images.get(&index) {
            ids.extend(slots.iter().flatten().map(|m| m.datablock));
        }
        self.cache_image_states(ids);
        let cache = &self.image_states_cache;
        let states = |id: u32| cache.get(&id).cloned().flatten();
        let slots = self.images.entry(index).or_default();
        let counts = self.image_fire_counts.entry(index).or_default();
        let mut gone = Vec::new();
        super::images::apply(
            S::image_rules(),
            slots,
            counts,
            sent,
            added,
            now,
            &states,
            &mut self.image_budget,
            &mut self.image_ticks,
            &mut gone,
        );
        if slots.iter().all(Option::is_none) {
            self.images.remove(&index);
        }
        for (slot, event) in gone {
            self.push_effect(Effect::Image {
                ghost: index,
                slot,
                event,
            });
        }
        self.take_image_events(Some(index));
    }

    /// Moves the slots' image events (of ghost `only`, else of every ghost) into the effects.
    fn take_image_events(&mut self, only: Option<u32>) {
        let mut taken = Vec::new();
        let ghosts: Vec<u32> = match only {
            Some(g) => vec![g],
            None => self.images.keys().copied().collect(),
        };
        for ghost in ghosts {
            let Some(slots) = self.images.get_mut(&ghost) else {
                continue;
            };
            for (slot, m) in slots.iter_mut().enumerate() {
                if let Some(m) = m {
                    taken.extend(m.events.drain(..).map(|event| (ghost, slot, event)));
                }
            }
        }
        for (ghost, slot, event) in taken {
            self.push_effect(Effect::Image { ghost, slot, event });
        }
    }
}
