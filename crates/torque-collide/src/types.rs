//! The object type bits (`mTypeMask`, the `SimObjectTypes` of Torque's `objectTypes.h`) and the masks the classes query the
//! container with and test their hits against ([`TypeMasks`]).
//!
//! The classes of this library test only the role bits below on what they hit (a Player, an Item, a trigger ...), and read
//! every container and ray mask from the host's [`TypeMasks`]. A game that claims bits of its own for its classes, or queries
//! with other masks, gives its own table; [`TypeMasks::TGE`] is Torque's.

/// `StaticObjectType`: world that never moves.
pub const STATIC: u32 = 1 << 0;
/// `EnvironmentObjectType`.
pub const ENVIRONMENT: u32 = 1 << 1;
/// `TerrainObjectType`.
pub const TERRAIN: u32 = 1 << 2;
/// `InteriorObjectType`.
pub const INTERIOR: u32 = 1 << 3;
/// `WaterObjectType`.
pub const WATER: u32 = 1 << 4;
/// `TriggerObjectType`.
pub const TRIGGER: u32 = 1 << 5;
/// `MarkerObjectType`.
pub const MARKER: u32 = 1 << 6;
/// `GameBaseObjectType`.
pub const GAME_BASE: u32 = 1 << 10;
/// `ShapeBaseObjectType`.
pub const SHAPE_BASE: u32 = 1 << 11;
/// `CameraObjectType`.
pub const CAMERA: u32 = 1 << 12;
/// `StaticShapeObjectType`.
pub const STATIC_SHAPE: u32 = 1 << 13;
/// `PlayerObjectType`.
pub const PLAYER: u32 = 1 << 14;
/// `ItemObjectType`.
pub const ITEM: u32 = 1 << 15;
/// `VehicleObjectType`.
pub const VEHICLE: u32 = 1 << 16;
/// `VehicleBlockerObjectType`.
pub const VEHICLE_BLOCKER: u32 = 1 << 17;
/// `ProjectileObjectType`.
pub const PROJECTILE: u32 = 1 << 18;
/// `ExplosionObjectType`.
pub const EXPLOSION: u32 = 1 << 19;
/// `CorpseObjectType`.
pub const CORPSE: u32 = 1 << 20;
/// `DebrisObjectType`.
pub const DEBRIS: u32 = 1 << 22;
/// `PhysicalZoneObjectType`.
pub const PHYSICAL_ZONE: u32 = 1 << 23;
/// `StaticTSObjectType`.
pub const STATIC_TS: u32 = 1 << 24;
/// `AIObjectType`.
pub const AI: u32 = 1 << 25;
/// `StaticRenderedObjectType`.
pub const STATIC_RENDERED: u32 = 1 << 26;
/// `DamagableItemObjectType`: the one bit a datablock may add to an object's type.
pub const DAMAGABLE_ITEM: u32 = 1 << 27;

/// The masks the classes query with and test hits against. Every container query, ray and hit test of the classes reads its
/// mask here; the role bits they test on a hit (a Player, an Item, a trigger, a corpse, a zone, a shape) are the constants of
/// this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeMasks {
    /// The Player's working-set query on a server (`sServerCollisionContactMask`).
    pub player_working: u32,
    /// The Player's working-set query on a ghost (`sClientCollisionContactMask`).
    pub player_ghost_working: u32,
    /// What the Player's `updatePos` and its early out take from the working set (`sCollisionMoveMask`).
    pub player_move_collide: u32,
    /// What the Player's `step` takes from the working set.
    pub player_step: u32,
    /// The types of the contacts `findContact` tells a host about, for a host that wants to be told. Torque's `findContact`
    /// tells no one: its 0 offers none.
    pub player_touch: u32,
    /// The Item's working-set query and the types its `updatePos` collides with.
    pub item_collide: u32,
    /// The hits the Item's `updatePos` treats as static world.
    pub item_solid: u32,
    /// The bits of `ItemData::dynamicType` an Item takes into its type.
    pub item_dynamic_allowed: u32,
    /// The Projectile's ray (it hits Players).
    pub projectile_ray_players: u32,
    /// The hits the Projectile treats as static world (no pose re-send), and its world-only re-cast.
    pub projectile_solid: u32,
    /// The AIPlayer's line-of-sight ray.
    pub ai_los: u32,
    /// The corrected-aim ray (`getCorrectedAim`).
    pub muzzle_aim: u32,
    /// A vehicle's working-set query (`sCollisionMoveMask` of the vehicles).
    pub vehicle_working: u32,
    /// A vehicle's wheel and hover rays.
    pub vehicle_ray: u32,
    /// A vehicle's per-tick scan for triggers, items and corpses.
    pub vehicle_scan: u32,
}

impl TypeMasks {
    /// Torque's masks, from its per-class masks.
    ///
    /// The corrected-aim ray hits every type, as Torque's does. The vehicle working set is the wheeled and hover vehicles' (a
    /// flying vehicle also takes water); the vehicle ray is the wheeled vehicle's.
    pub const TGE: TypeMasks = {
        let player_move_collide = TERRAIN
            | INTERIOR
            | WATER
            | PLAYER
            | STATIC_SHAPE
            | VEHICLE
            | PHYSICAL_ZONE
            | STATIC_TS;
        let projectile_ray_players =
            TERRAIN | INTERIOR | STATIC | PLAYER | VEHICLE | DAMAGABLE_ITEM;
        TypeMasks {
            player_working: player_move_collide | ITEM | TRIGGER | CORPSE,
            player_ghost_working: player_move_collide | PHYSICAL_ZONE,
            player_move_collide,
            player_step: STATIC,
            player_touch: 0,
            item_collide: TERRAIN | INTERIOR | STATIC_SHAPE | VEHICLE | PLAYER | STATIC_TS,
            item_solid: TERRAIN | INTERIOR | STATIC,
            item_dynamic_allowed: DAMAGABLE_ITEM,
            projectile_ray_players,
            projectile_solid: TERRAIN | INTERIOR | STATIC,
            ai_los: INTERIOR | STATIC_SHAPE | STATIC | TERRAIN,
            muzzle_aim: u32::MAX,
            vehicle_working: TERRAIN
                | INTERIOR
                | PLAYER
                | STATIC_SHAPE
                | VEHICLE
                | VEHICLE_BLOCKER
                | STATIC_TS,
            vehicle_ray: TERRAIN | INTERIOR | STATIC_SHAPE | VEHICLE | VEHICLE_BLOCKER | STATIC_TS,
            vehicle_scan: ITEM | TRIGGER | CORPSE,
        }
    };
}

impl Default for TypeMasks {
    fn default() -> TypeMasks {
        TypeMasks::TGE
    }
}
