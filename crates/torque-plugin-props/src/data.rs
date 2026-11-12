//! The `PropData` datablock: what a kind of prop collides as, how heavy and slippery it is, when it sleeps and how it is drawn;
//! its wire record ([`PropDataRecord`]); and the same numbers made safe ([`PropParams`]).

use torque_engine::{EngineClass, Filename};
use torque_ts_vm::classes::DatablockChange;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use crate::quant;

/// The largest half-extent of a prop's collision box or ball (units).
pub const MAX_HALF_EXTENT: f32 = 64.0;
/// The smallest.
pub const MIN_HALF_EXTENT: f32 = 0.01;
/// The largest scale a prop's shape is drawn at, along any axis.
pub const MAX_DRAW_SCALE: f32 = 256.0;
/// The longest shape file a datablock's record carries.
pub const MAX_SHAPE_BYTES: usize = 96;

/// A kind of prop (a game datablock, so `function Crate::onPropSleep(%this, %prop)` is a method of the datablock named `Crate`): a
/// script writes one as `datablock PropData(Crate) { shapeFile = "art/crate.dts"; size = "1 1 1"; mass = 20; };`
/// and names it when it spawns a prop (`propSpawn(Crate, "0 0 2 1 0 0 0")`). Every number is clamped to its range when it is
/// written, and again when a prop reads it, so a hostile value never reaches the physics world ([`PropParams`]).
#[derive(EngineClass, Clone, Debug, PartialEq)]
#[class(
    name = "PropData",
    parent = "GameBaseData",
    preload = PropData::canonicalize,
    refresh = PropData::refresh
)]
pub struct PropData {
    /// The shape the prop is drawn as (a `.dts`, or a format the engine imports); empty draws nothing. What it collides as is
    /// `collision`: `size` or `radius` by default, the shape's collision meshes with `collision = "shape"`.
    #[field(name = "shapeFile", default = "")]
    pub shape_file: Filename,
    /// What the prop collides as in the physics world: `"box"` (the default; the box of `size`, or the ball of `radius`) or
    /// `"shape"` (the convex hulls of the `shapeFile`'s collision details, `Collision-1` and on, at `drawScale`; a shape with
    /// none collides as its bounds, and one the server refuses ([`crate::hulls`]) as the box). With `"shape"`, `size` is still the
    /// box players, placement checks and shots see, and `mass` is the mass of the whole shape. Any other text is `"box"`.
    #[field(default = "box")]
    pub collision: String,
    /// The scale the shape is drawn at along each axis (0.001 to 256).
    #[field(name = "drawScale", default = "1 1 1")]
    pub draw_scale: [f32; 3],
    /// The collision box's full size along each axis, centered on the prop's origin (each 0.02 to 128 units).
    #[field(default = "1 1 1")]
    pub size: [f32; 3],
    /// Above 0: the prop collides as a ball of this radius (0.01 to 64) and `size` is not used.
    #[field(range = 0.0..=64.0, default = 0.0)]
    pub radius: f32,
    /// The mass; 0 (the default) takes it from `density` and the volume.
    #[field(range = 0.0..=100000.0, default = 0.0)]
    pub mass: f32,
    /// Mass a cubic unit, when `mass` is 0.
    #[field(range = 0.001..=100000.0, default = 1.0)]
    pub density: f32,
    /// 0 (ice) to 10.
    #[field(range = 0.0..=10.0, default = 0.6)]
    pub friction: f32,
    /// How much a hit bounces back, 0 to 1.
    #[field(range = 0.0..=1.0, default = 0.1)]
    pub restitution: f32,
    /// How fast it loses speed and spin (roughly a fraction a second), 0 to 100.
    #[field(name = "linearDamping", range = 0.0..=100.0, default = 0.1)]
    pub linear_damping: f32,
    #[field(name = "angularDamping", range = 0.0..=100.0, default = 0.3)]
    pub angular_damping: f32,
    /// The fastest it may move, units a second (at most the world's 500).
    #[field(name = "maxSpeed", range = 0.0..=500.0, default = 100.0)]
    pub max_speed: f32,
    /// It falls asleep after half a second slower than these (units and radians a second); 0 for either never sleeps.
    #[field(name = "sleepLinear", range = 0.0..=10.0, default = 0.1)]
    pub sleep_linear: f32,
    #[field(name = "sleepAngular", range = 0.0..=10.0, default = 0.2)]
    pub sleep_angular: f32,
    /// Whether `propFreeze` may pin it in place.
    #[field(name = "canFreeze", default = true)]
    pub can_freeze: bool,
    /// The most props of this kind one player may own (0: only the server's per-player cap).
    #[field(name = "maxPerPlayer", range = 0..=1024, default = 0)]
    pub max_per_player: i32,
}

impl PropData {
    /// The datablock's preload: `collision` is rewritten as the canonical text of the mode it names (trimmed, lower case, `box`
    /// for anything that is not `shape`), so the record a client is sent says what the server reads, whatever the script wrote.
    fn canonicalize(&self, vm: &mut Vm, id: ObjId) -> Result<(), String> {
        let canonical = PropCollision::of(&self.collision).text();
        if self.collision != canonical {
            vm.objects_mut()
                .set_field(id, b"collision", b"", canonical.as_bytes());
        }
        Ok(())
    }

    /// The datablock's refresh on a server (its fields written, or it declared again, under the props built from it): every
    /// prop among `users` takes the numbers as they are now, its body made again at its exact pose and velocities
    /// ([`crate::server`]'s `refresh_props`). A write and a redeclare do the same work: the shape's hulls are read from the
    /// book, which has them already unless the file changed.
    fn refresh(
        &self,
        vm: &mut Vm,
        id: ObjId,
        users: &[ObjId],
        _change: DatablockChange,
    ) -> Result<(), String> {
        crate::server::refresh_props(vm, id, self, users);
        Ok(())
    }
}

/// What a prop collides as.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PropShape {
    /// A box of these half-extents round the origin.
    Box([f32; 3]),
    /// A ball of this radius round the origin.
    Ball(f32),
    /// The shape's collision hulls ([`crate::hulls::PropHulls`]): `half` is the box that stands for them where only a box is
    /// asked (the datablock's `size`, or its `radius`), `volume` what they hold.
    Hulls { half: [f32; 3], volume: f32 },
}

/// What a prop collides as in the physics world ([`PropData::collision`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PropCollision {
    /// The box of `size`, or the ball of `radius`.
    #[default]
    Box,
    /// The convex hulls of the shape's collision meshes.
    Shape,
}

impl PropCollision {
    /// The mode `text` names: `shape` (any case, blanks round it) is [`PropCollision::Shape`]; anything else the box.
    pub fn of(text: &str) -> PropCollision {
        if text.trim().eq_ignore_ascii_case("shape") {
            PropCollision::Shape
        } else {
            PropCollision::Box
        }
    }

    /// The text the datablock keeps and sends for the mode: `shape` or `box`, which [`PropCollision::of`] reads back as itself
    /// and which the wire's 8 bytes never cut.
    pub fn text(self) -> &'static str {
        match self {
            PropCollision::Box => "box",
            PropCollision::Shape => "shape",
        }
    }
}

impl PropShape {
    /// The half-extents of the box round the shape.
    pub fn half_extents(self) -> [f32; 3] {
        match self {
            PropShape::Box(h) | PropShape::Hulls { half: h, .. } => h,
            PropShape::Ball(r) => [r; 3],
        }
    }

    /// The volume (a ball's by `4/3 pi r^3`).
    pub fn volume(self) -> f32 {
        match self {
            PropShape::Box([x, y, z]) => 8.0 * x * y * z,
            PropShape::Ball(r) => 4.188_790_2 * r * r * r,
            PropShape::Hulls { volume, .. } => volume,
        }
    }
}

/// A [`PropData`] (or the record a client was sent) with every number made safe ([`PropNumbers::held`]): a NaN or an infinity becomes the field's default,
/// the rest is held to its range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PropParams {
    pub shape: PropShape,
    /// What the datablock asked for. With [`PropCollision::Shape`] and no hulls built (yet, or refused) `shape` is the box.
    pub collision: PropCollision,
    /// The datablock's `mass` (0: the mass comes from `density`).
    pub mass: f32,
    /// Mass a cubic unit, after a `mass` was turned into one.
    pub density: f32,
    pub friction: f32,
    pub restitution: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub max_speed: f32,
    pub sleep_linear: f32,
    pub sleep_angular: f32,
    pub can_freeze: bool,
    pub max_per_player: u32,
}

/// `v` held to `lo..=hi`; `default` when it is not a number.
pub fn held(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
    torque_collide::math::finite_clamp(v, lo, hi).unwrap_or(default)
}

/// A datablock's numbers as they were written (or sent), before they are made safe ([`PropNumbers::held`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PropNumbers {
    pub collision: PropCollision,
    pub size: [f32; 3],
    pub radius: f32,
    pub mass: f32,
    pub density: f32,
    pub friction: f32,
    pub restitution: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub max_speed: f32,
    pub sleep_linear: f32,
    pub sleep_angular: f32,
    pub can_freeze: bool,
    pub max_per_player: i32,
}

impl Default for PropNumbers {
    /// The class's defaults.
    fn default() -> PropNumbers {
        PropNumbers {
            collision: PropCollision::Box,
            size: [1.0; 3],
            radius: 0.0,
            mass: 0.0,
            density: 1.0,
            friction: 0.6,
            restitution: 0.1,
            linear_damping: 0.1,
            angular_damping: 0.3,
            max_speed: 100.0,
            sleep_linear: 0.1,
            sleep_angular: 0.2,
            can_freeze: true,
            max_per_player: 0,
        }
    }
}

impl PropNumbers {
    /// The numbers made safe.
    #[must_use]
    pub fn held(&self) -> PropParams {
        let radius = held(self.radius, 0.0, MAX_HALF_EXTENT, 0.0);
        let shape = if radius >= MIN_HALF_EXTENT {
            PropShape::Ball(radius)
        } else {
            PropShape::Box(
                self.size
                    .map(|s| held(s * 0.5, MIN_HALF_EXTENT, MAX_HALF_EXTENT, 0.5)),
            )
        };
        let mass = held(self.mass, 0.0, 100_000.0, 0.0);
        let density = if mass > 0.0 {
            held(mass / shape.volume(), 0.001, 100_000.0, 1.0)
        } else {
            held(self.density, 0.001, 100_000.0, 1.0)
        };
        PropParams {
            shape,
            collision: self.collision,
            mass,
            density,
            friction: held(self.friction, 0.0, 10.0, 0.6),
            restitution: held(self.restitution, 0.0, 1.0, 0.1),
            linear_damping: held(self.linear_damping, 0.0, 100.0, 0.1),
            angular_damping: held(self.angular_damping, 0.0, 100.0, 0.3),
            max_speed: held(self.max_speed, 0.0, quant::MAX_SPEED, 100.0),
            sleep_linear: held(self.sleep_linear, 0.0, 10.0, 0.1),
            sleep_angular: held(self.sleep_angular, 0.0, 10.0, 0.2),
            can_freeze: self.can_freeze,
            max_per_player: self.max_per_player.clamp(0, 1024) as u32,
        }
    }
}

impl PropData {
    /// The datablock's numbers as written, before they are made safe.
    pub fn numbers(&self) -> PropNumbers {
        PropNumbers {
            collision: PropCollision::of(&self.collision),
            size: self.size,
            radius: self.radius,
            mass: self.mass,
            density: self.density,
            friction: self.friction,
            restitution: self.restitution,
            linear_damping: self.linear_damping,
            angular_damping: self.angular_damping,
            max_speed: self.max_speed,
            sleep_linear: self.sleep_linear,
            sleep_angular: self.sleep_angular,
            can_freeze: self.can_freeze,
            max_per_player: self.max_per_player,
        }
    }
}

impl From<&PropData> for PropParams {
    fn from(d: &PropData) -> PropParams {
        d.numbers().held()
    }
}

impl PropDataRecord {
    /// The record's numbers as sent, before they are made safe.
    pub fn numbers(&self) -> PropNumbers {
        PropNumbers {
            collision: PropCollision::of(&self.collision),
            size: self.size,
            radius: self.radius,
            mass: self.mass,
            density: self.density,
            friction: self.friction,
            restitution: self.restitution,
            linear_damping: self.linear_damping,
            angular_damping: self.angular_damping,
            max_speed: self.max_speed,
            sleep_linear: self.sleep_linear,
            sleep_angular: self.sleep_angular,
            can_freeze: self.can_freeze,
            max_per_player: 0,
        }
    }
}

impl From<&PropDataRecord> for PropParams {
    fn from(r: &PropDataRecord) -> PropParams {
        r.numbers().held()
    }
}

impl PropParams {
    /// These numbers for a prop that collides as hulls holding `volume`: the box or ball becomes the hulls' bounds, and a `mass`
    /// is spread over the hulls (a `density` is a density still). Unchanged for a volume that is not a positive number.
    #[must_use]
    pub fn with_hulls(self, volume: f32) -> PropParams {
        if !(volume.is_finite() && volume > 0.0) {
            return self;
        }
        let density = if self.mass > 0.0 {
            held(self.mass / volume, 0.001, 100_000.0, 1.0)
        } else {
            self.density
        };
        PropParams {
            shape: PropShape::Hulls {
                half: self.shape.half_extents(),
                volume,
            },
            density,
            ..self
        }
    }

    /// The prop's mass.
    pub fn mass(&self) -> f32 {
        self.density * self.shape.volume()
    }
}

/// `PropData` as it travels to a client: the console fields a client draws and (later) predicts with, by the same names as the
/// class (a test keeps the two in step). `maxPerPlayer` stays on the server.
#[derive(Debug, Clone, PartialEq, Default, torque_engine::DatablockCodec)]
pub struct PropDataRecord {
    #[net(max_len = 96)]
    #[net(name = "shapeFile")]
    pub shape_file: String,
    #[net(name = "drawScale")]
    pub draw_scale: [f32; 3],
    #[net(max_len = 8)]
    pub collision: String,
    pub size: [f32; 3],
    pub radius: f32,
    pub mass: f32,
    pub density: f32,
    pub friction: f32,
    pub restitution: f32,
    #[net(name = "linearDamping")]
    pub linear_damping: f32,
    #[net(name = "angularDamping")]
    pub angular_damping: f32,
    #[net(name = "maxSpeed")]
    pub max_speed: f32,
    #[net(name = "sleepLinear")]
    pub sleep_linear: f32,
    #[net(name = "sleepAngular")]
    pub sleep_angular: f32,
    #[net(name = "canFreeze")]
    pub can_freeze: bool,
}

/// A draw scale held to what the renderer draws: each axis finite and within `0.001..=`[`MAX_DRAW_SCALE`] (1 when not a number).
pub fn draw_scale(s: [f32; 3]) -> [f32; 3] {
    s.map(|v| held(v, 0.001, MAX_DRAW_SCALE, 1.0))
}
