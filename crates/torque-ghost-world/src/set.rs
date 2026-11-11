//! What the client's world asks of a game: its classes and records ([`ClientSet`], [`ClientRecord`]), its datablocks
//! ([`ClientDatablock`]), the control object's packet data ([`ClientTail`]), the part a game keeps on each ghost ([`GhostPart`]),
//! and the ghost table a connection's decoder fills ([`SentTable`], [`SentGhost`]). A game's own steps around a change of the ghost
//! table run through [`GhostHooks`].

use std::fmt::Debug;
use std::hash::Hash;

use net::control::ControlData;

use super::images::{ImageRules, ImageSlot, ImageStates, TgeImageRules};
use super::threads::{self, Thread};
use super::world::{ClientWorld, Ghost, Pose};

/// A game's ghost classes and records as the client's world keeps them. The set is a marker type: the world is
/// [`ClientWorld<S>`].
pub trait ClientSet: Debug + Clone + Copy + PartialEq + Default + Send + Sync + 'static {
    /// A ghost class.
    type Class: Copy + Eq + Hash + Debug + Send + Sync + 'static;
    /// A record of any class of the set.
    type Record: ClientRecord<Self>;
    /// A datablock as its event carried it.
    type Datablock: ClientDatablock;
    /// What a control section held about the control object.
    type Tail: ClientTail;
    /// The game's own part of every ghost ([`Ghost::part`]).
    type Part: GhostPart;
    /// The game's own effects ([`super::effects::Effect::Game`]).
    type Effect: Clone + Debug + PartialEq + Send + Sync;

    /// The class's name (the net class table's).
    fn class_name(class: Self::Class) -> &'static str;

    /// Whether the class is a vehicle whose updates the world keeps in order ([`ClientWorld::vehicle_updates`]).
    fn is_vehicle(class: Self::Class) -> bool;

    /// The control object's packet data a control section resent.
    fn tail(data: &dyn ControlData) -> Self::Tail;

    /// The server-to-client remote commands the world keeps the last arguments of ([`ClientWorld::commands`]), lower case: the
    /// ones something reads. None by default.
    const KEPT_COMMANDS: &'static [&'static [u8]] = &[];

    /// How the client runs its mounted images where the game's differs from TGE's ([`ImageRules`]; TGE's by default).
    fn image_rules() -> &'static dyn ImageRules {
        &TgeImageRules
    }

    /// The thread block of one record applied to a ghost's script threads at the world clock `now` (TGE's rule by default,
    /// [`threads::apply`]).
    fn apply_threads(
        slots: &mut threads::Slots,
        sent: &[Option<Thread>; threads::THREAD_SLOTS],
        now: Option<u32>,
    ) {
        threads::apply(slots, sent, now);
    }
}

/// A ShapeBase sound thread slot as an update sent it: `playing` and, when playing, the profile's datablock id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SoundSlot {
    pub playing: bool,
    pub profile: u32,
}

/// A projectile's explosion block: where it exploded and the surface's normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Explosion {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

/// A Player's move block as an update sent it (its position is [`ClientRecord::point`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveBlock {
    /// The velocity block's vector (zero when it was not sent).
    pub velocity: [f32; 3],
    /// The client may warp to it.
    pub warp: bool,
    /// The Player jumped (a game's move block may say so; TGE's does not).
    pub jumped: bool,
}

/// What the world asks of a ghost's record. `S` is the set the record belongs to (the record's own crate need not know the world).
pub trait ClientRecord<S>: Clone + Debug + PartialEq + Send + Sync {
    /// An update record merged into the kept one: a block that was sent replaces the kept one, a block that was not leaves it.
    fn merge_update(&mut self, sent: &Self);

    /// The datablock id the record names.
    fn datablock(&self) -> Option<u32>;

    /// The compressed point the record carries, resolved against the packet's compression point `cp`.
    fn point(&self, cp: [f32; 3]) -> Option<[f32; 3]>;

    /// Where the ghost is, as the record puts it; `point` is the ghost's last resolved compressed point.
    fn pose(&self, point: Option<[f32; 3]>) -> Option<Pose>;

    /// The scale the GameBase part sent last, for a ShapeBase class.
    fn scale(&self) -> Option<[f32; 3]>;

    /// The ghost index and node the ShapeBase part's mount block names.
    fn mount(&self) -> Option<(u32, u32)>;

    /// The mounted image slots the record sent.
    fn images(&self) -> Option<[Option<ImageSlot>; 4]>;

    /// The script thread slots the record sent.
    fn threads(&self) -> Option<[Option<Thread>; 4]>;

    /// The sound thread slots the record sent.
    fn sounds(&self) -> Option<[Option<SoundSlot>; 4]>;

    /// A projectile's explosion block.
    fn explosion(&self) -> Option<Explosion>;

    /// A Player's move block.
    fn move_block(&self) -> Option<MoveBlock>;

    /// A Camera's position, pitch and yaw when the record moved one its client does not control.
    fn camera_move(&self) -> Option<[f32; 5]>;
}

/// What the world asks of a datablock.
pub trait ClientDatablock: Clone + Debug + PartialEq + Send + Sync {
    /// A string field's first value.
    fn text(&self, field: &str) -> Option<&[u8]>;

    /// A Player datablock's jump sound (the profile's id), where a game's datablocks have one (TGE's PlayerData does not).
    fn jump_sound(&self) -> Option<u32>;

    /// A projectile datablock's explosion (the explosion datablock's id).
    fn explosion(&self) -> Option<u32>;

    /// A sound profile's description (the description datablock's id): what a client plays a ShapeBase sound slot or a
    /// sound event with, where its own profile object cannot name it (a remote client's datablocks are unnamed).
    fn audio_description(&self) -> Option<u32>;

    /// A shape constructor's base shape.
    fn base_shape(&self) -> Option<&[u8]>;

    /// A shape constructor's sequence texts, at most `max`.
    fn sequences(&self, max: usize) -> Vec<Vec<u8>>;

    /// An image datablock's states (`None` for another class).
    fn image_states(&self) -> Option<ImageStates>;
}

/// What the world asks of the control object's packet data.
pub trait ClientTail: Clone + Debug + PartialEq + Send + Sync {
    /// A Camera's position, pitch and yaw.
    fn camera(&self) -> Option<([f32; 3], f32, f32)>;
}

/// The game's own part of a ghost (the world keeps it and never looks inside but for these).
pub trait GhostPart: Clone + Debug + PartialEq + Default + Send + Sync {
    /// The datablock the part names, ahead of the record's.
    fn datablock(&self) -> Option<u32> {
        None
    }

    /// The ghost's updates are also summed apart ([`super::world::GhostIndex::part_updates`]).
    fn summed_apart(&self) -> bool {
        false
    }

    /// The ghost is in the index's flagged set ([`super::world::GhostIndex::flagged`]).
    fn flagged(&self) -> bool {
        false
    }
}

impl GhostPart for () {}

/// A ghost as a connection's ghost table holds it.
pub trait SentGhost<S: ClientSet> {
    fn class(&self) -> Option<S::Class>;
    /// The creating record's number among the client's records (1 first).
    fn serial(&self) -> usize;
    /// The last record the table read for it (the creating one, or an update holding only its mask's blocks).
    fn record(&self) -> Option<&S::Record>;
    /// Update records applied after the creating one.
    fn updates(&self) -> u32;
    /// The per-ghost block the last record carried ([`ghosts::block`]), when the connection carries one.
    fn block(&self) -> Option<&ghosts::block::GhostBlock> {
        None
    }
}

/// A connection's ghost table.
pub trait SentTable<S: ClientSet> {
    type Ghost: SentGhost<S>;
    fn ghost(&self, index: u32) -> Option<&Self::Ghost>;
}

/// The game's steps around each change of the ghost table ([`ClientWorld::apply_ghosts`]); none by default.
pub trait GhostHooks<S: ClientSet, G> {
    /// After the table was cleared.
    fn cleared(&mut self, world: &mut ClientWorld<S>) {
        let _ = world;
    }

    /// After ghost `index` was deleted (`gone`: the ghost the world held).
    fn deleted(&mut self, world: &mut ClientWorld<S>, index: u32, gone: Option<&Ghost<S>>) {
        let _ = (world, index, gone);
    }

    /// The part of a ghost made from `sent`.
    fn part(&mut self, sent: &G) -> S::Part {
        let _ = sent;
        S::Part::default()
    }

    /// After ghost `index` was made from `sent` (`old`: the ghost it replaced).
    fn created(
        &mut self,
        world: &mut ClientWorld<S>,
        index: u32,
        sent: &G,
        old: Option<&Ghost<S>>,
    ) {
        let _ = (world, index, sent, old);
    }

    /// While ghost `index` is updated from `sent`: after its record was merged, before the index counts it.
    fn updating(&mut self, index: u32, ghost: &mut Ghost<S>, sent: &G) {
        let _ = (index, ghost, sent);
    }

    /// After ghost `index` was updated from `sent`.
    fn updated(&mut self, world: &mut ClientWorld<S>, index: u32, sent: &G) {
        let _ = (world, index, sent);
    }
}

impl<S: ClientSet, G> GhostHooks<S, G> for () {}
