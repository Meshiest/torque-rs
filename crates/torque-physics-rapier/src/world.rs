//! The world: bodies by id over a Rapier world, the map's cells, the fixed step and the state read back.

use std::collections::BTreeMap;
use std::rc::Rc;

use rapier3d::math::Rotation as Quat;
use rapier3d::prelude::{
    ColliderBuilder, PhysicsWorld, Pose, RigidBodyBuilder, RigidBodyHandle, RigidBodyType, Vector,
};
use torque_collide::ObjectId;
use torque_collide::map::MapShape;
use torque_collide::math::Box3;

use crate::guard;
use crate::limits::{Config, Limits, MAX_SUBSTEPS};
use crate::map::{MapStats, Maps};
use crate::shape::{self, Shape};
use crate::{TICK, work};

/// The friction of every map collider (the other body's is combined with it by Rapier's average rule).
pub const MAP_FRICTION: f32 = 0.8;

/// A body's id: the caller's object id. Bodies are kept, stepped and read in id order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BodyId(pub u32);

/// How a body moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Motion {
    /// Moved by forces, contacts and gravity.
    #[default]
    Dynamic,
    /// Moved only to the poses it is given ([`World::set_kinematic_target`]); it pushes dynamic bodies and is not pushed.
    Kinematic,
    /// Never moves.
    Fixed,
}

/// A body to add.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyDesc {
    pub motion: Motion,
    pub shape: Shape,
    pub position: [f32; 3],
    /// A quaternion `[x, y, z, w]` (normalized on the way in).
    pub rotation: [f32; 4],
    pub linvel: [f32; 3],
    /// Radians a second about each world axis.
    pub angvel: [f32; 3],
    /// Mass a cubic unit.
    pub density: f32,
    /// 0 to 10.
    pub friction: f32,
    /// 0 to 1.
    pub restitution: f32,
    /// 0 to 1,000 (a second's fraction of speed lost, roughly).
    pub linear_damping: f32,
    pub angular_damping: f32,
    /// Continuous collision for a fast body.
    pub ccd: bool,
}

impl Default for BodyDesc {
    fn default() -> Self {
        BodyDesc {
            motion: Motion::Dynamic,
            shape: Shape::Box { half: [0.5; 3] },
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            linvel: [0.0; 3],
            angvel: [0.0; 3],
            density: 1.0,
            friction: 0.6,
            restitution: 0.0,
            linear_damping: 0.0,
            angular_damping: 0.0,
            ccd: false,
        }
    }
}

/// A body's state as the world holds it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BodyState {
    pub position: [f32; 3],
    /// `[x, y, z, w]`.
    pub rotation: [f32; 4],
    pub linvel: [f32; 3],
    pub angvel: [f32; 3],
    pub sleeping: bool,
}

impl BodyState {
    /// The state as words (each float by its bits), for a hash.
    pub fn words(&self) -> [u32; 14] {
        let mut out = [0u32; 14];
        let floats = self
            .position
            .iter()
            .chain(&self.rotation)
            .chain(&self.linvel)
            .chain(&self.angvel);
        for (o, f) in out.iter_mut().zip(floats) {
            *o = f.to_bits();
        }
        out[13] = u32::from(self.sleeping);
        out
    }
}

/// A cap that refused a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    Bodies,
    Colliders,
}

/// Why a call was refused. A refused call changes nothing, except where noted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A value was NaN or infinite (or a rotation was zero).
    NonFinite,
    /// The budget was spent. A step refused this way did not move anything; a region build keeps the pieces it made.
    Exhausted,
    /// A cap was reached ([`Limits`]).
    Cap(Cap),
    /// A body of this id is in the world already.
    Duplicate(BodyId),
    /// No body has this id.
    Unknown(BodyId),
    /// Rapier panicked (native builds catch it); the world is poisoned.
    Panicked,
    /// A panic was caught earlier: the world refuses all work. Make a new one.
    Poisoned,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NonFinite => write!(f, "a value is not finite"),
            Error::Exhausted => write!(f, "the work budget is spent"),
            Error::Cap(Cap::Bodies) => write!(f, "the world holds its most bodies"),
            Error::Cap(Cap::Colliders) => write!(f, "the world holds its most colliders"),
            Error::Duplicate(id) => write!(f, "body {} is in the world already", id.0),
            Error::Unknown(id) => write!(f, "no body {}", id.0),
            Error::Panicked => write!(f, "the physics step panicked"),
            Error::Poisoned => write!(f, "the world was poisoned by an earlier panic"),
        }
    }
}

impl std::error::Error for Error {}

/// What a step did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StepReport {
    /// The sub-steps run (fewer than the world's when the contact cap stopped the tick).
    pub substeps: u8,
    /// Solver contact points after the last sub-step run.
    pub contacts: usize,
    /// The contact cap stopped the tick early.
    pub contacts_capped: bool,
    /// Map cells built for this step.
    pub cells_built: usize,
    /// A cap or the budget stopped a cell being built (the bodies near it may miss part of the map this tick).
    pub map_capped: bool,
    /// Bodies put back where they were before the step, at rest: their state left the finite range or the world's reach.
    pub restored: Vec<BodyId>,
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    handle: RigidBodyHandle,
    /// The state after the last step that kept it in range.
    good: BodyState,
    /// How far the body's colliders reach from its origin.
    radius: f32,
    /// A kinematic body given a target this tick: it stops when the step reaches it.
    arriving: bool,
}

/// Counts a world keeps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorldStats {
    /// Body hulls that became their box.
    pub hulls_replaced: usize,
    /// Bodies put back by a step.
    pub restored: usize,
    /// Ticks the contact cap cut short.
    pub contacts_capped: usize,
}

/// A deterministic Rapier world (crate docs).
pub struct World {
    config: Config,
    rapier: PhysicsWorld,
    bodies: BTreeMap<BodyId, Entry>,
    maps: Maps,
    poisoned: bool,
    stats: WorldStats,
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World")
            .field("config", &self.config)
            .field("bodies", &self.bodies.len())
            .field("colliders", &self.rapier.colliders.len())
            .field("maps", &self.maps.stats)
            .field("poisoned", &self.poisoned)
            .finish()
    }
}

/// Limits held to ranges that keep the world sound whatever a caller (or a pref) asked for.
fn sane(l: Limits) -> Limits {
    let pos = |v: f32, lo: f32, hi: f32, default: f32| {
        if v.is_finite() {
            v.clamp(lo, hi)
        } else {
            default
        }
    };
    let d = Limits::default();
    let min_half_extent = pos(l.min_half_extent, 1e-4, 1.0, d.min_half_extent);
    let min_density = pos(l.min_density, 1e-6, 1.0, d.min_density);
    Limits {
        max_bodies: l.max_bodies.min(65_536),
        max_colliders: l.max_colliders.min(1 << 20),
        max_hull_vertices: l.max_hull_vertices.clamp(4, 4_096),
        max_contacts: l.max_contacts.min(1 << 20),
        max_cells_per_query: l.max_cells_per_query.clamp(1, 4_096),
        max_coordinate: pos(l.max_coordinate, 1.0, 1.0e7, d.max_coordinate),
        max_speed: pos(l.max_speed, 0.0, 1.0e5, d.max_speed),
        max_angular_speed: pos(l.max_angular_speed, 0.0, 1.0e4, d.max_angular_speed),
        max_half_extent: pos(l.max_half_extent, min_half_extent, 1.0e5, d.max_half_extent),
        min_half_extent,
        max_density: pos(l.max_density, min_density, 1.0e9, d.max_density),
        min_density,
        max_impulse: pos(l.max_impulse, 0.0, 1.0e12, d.max_impulse),
    }
}

fn v3(v: [f32; 3]) -> Vector {
    Vector::new(v[0], v[1], v[2])
}

fn a3(v: Vector) -> [f32; 3] {
    [v.x, v.y, v.z]
}

fn quat(q: [f32; 4]) -> Quat {
    Quat::from_xyzw(q[0], q[1], q[2], q[3])
}

/// The angular velocity (radians a second about the world axes) that turns `from` into `to` in `dt` seconds, the short way.
/// In `f64`, with `libm`'s `atan2` (the same bits on every target).
fn turn(from: [f32; 4], to: [f32; 4], dt: f64) -> [f32; 3] {
    let [ax, ay, az, aw] = from.map(f64::from);
    let [bx, by, bz, bw] = to.map(f64::from);
    // d = to * conjugate(from).
    let mut d = [
        bw * -ax + bx * aw + by * -az - bz * -ay,
        bw * -ay - bx * -az + by * aw + bz * -ax,
        bw * -az + bx * -ay - by * -ax + bz * aw,
        bw * aw - bx * -ax - by * -ay - bz * -az,
    ];
    if d[3] < 0.0 {
        d = d.map(|v| -v);
    }
    let s = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if !(s > 1e-12) {
        return [0.0; 3];
    }
    let angle = 2.0 * libm::atan2(s, d[3]);
    [0, 1, 2].map(|k| (d[k] / s * angle / dt) as f32)
}

impl World {
    /// An empty world of `config` (its values held to their ranges).
    pub fn new(config: Config) -> World {
        let limits = sane(config.limits);
        let gravity = guard::magnitude(config.gravity, 1.0e4).unwrap_or([0.0, 0.0, -20.0]);
        let config = Config {
            substeps: config.substeps.clamp(1, MAX_SUBSTEPS),
            gravity,
            solver_iterations: config.solver_iterations.clamp(1, 16),
            sleeping: config.sleeping,
            limits,
        };
        let mut rapier = PhysicsWorld::new();
        rapier.gravity = v3(gravity);
        let p = &mut rapier.integration_parameters;
        p.dt = TICK / f32::from(config.substeps);
        p.num_solver_iterations = usize::from(config.solver_iterations);
        p.normalized_max_linear_velocity = limits.max_speed.max(1.0);
        p.min_ccd_dt = p.dt / 100.0;
        let mut maps = Maps::default();
        maps.friction = MAP_FRICTION;
        World {
            config,
            rapier,
            bodies: BTreeMap::new(),
            maps,
            poisoned: false,
            stats: WorldStats::default(),
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn limits(&self) -> &Limits {
        &self.config.limits
    }

    /// Whether a caught panic has stopped the world.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Stops the world as a caught panic does: every later call is refused with [`Error::Poisoned`]. For a caller's test of
    /// how it recovers (a fresh world rebuilt from its own record of the bodies).
    pub fn poison(&mut self) {
        self.poisoned = true;
    }

    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }

    pub fn collider_count(&self) -> usize {
        self.rapier.colliders.len()
    }

    pub fn map_stats(&self) -> MapStats {
        self.maps.stats
    }

    pub fn stats(&self) -> WorldStats {
        self.stats
    }

    /// The bodies' ids, ascending.
    pub fn ids(&self) -> impl Iterator<Item = BodyId> + '_ {
        self.bodies.keys().copied()
    }

    /// The Rapier world underneath, read only (queries, debugging).
    pub fn rapier(&self) -> &PhysicsWorld {
        &self.rapier
    }

    fn live(&self) -> Result<(), Error> {
        if self.poisoned {
            Err(Error::Poisoned)
        } else {
            Ok(())
        }
    }

    /// Runs `f` on the Rapier world; a panic poisons the world.
    fn guarded<T>(&mut self, f: impl FnOnce(&mut PhysicsWorld) -> T) -> Result<T, Error> {
        let rapier = &mut self.rapier;
        let out = shape::caught(|| f(rapier));
        if out.is_err() {
            self.poisoned = true;
        }
        out
    }

    /// Adds or replaces a map piece (a terrain or an interior, as the engine's collision holds it) by its object id. Its
    /// colliders are made by cell when bodies come near ([`crate::map`]); every cell already built is built again.
    pub fn add_map(&mut self, id: ObjectId, shape: Rc<MapShape>) -> Result<(), Error> {
        self.live()?;
        let maps = &mut self.maps;
        let rapier = &mut self.rapier;
        let out = shape::caught(|| maps.insert(id, shape, rapier));
        self.poisoned |= out.is_err();
        out
    }

    /// Removes a map piece; true when it was there.
    pub fn remove_map(&mut self, id: ObjectId) -> Result<bool, Error> {
        self.live()?;
        let maps = &mut self.maps;
        let rapier = &mut self.rapier;
        let out = shape::caught(|| maps.remove(id, rapier));
        self.poisoned |= out.is_err();
        out
    }

    /// Builds the map cells under `region`'s xy extent that are not built yet (at most [`Limits::max_cells_per_query`]);
    /// the count built.
    pub fn ensure_region(
        &mut self,
        region: &Box3,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<usize, Error> {
        self.live()?;
        let limits = self.config.limits;
        let cells = Maps::cells_of(region, &limits).ok_or(Error::NonFinite)?;
        let mut built = 0;
        for cell in cells {
            if !charge(work::CELL) {
                return Err(Error::Exhausted);
            }
            if self.maps.is_built(cell) {
                continue;
            }
            let maps = &mut self.maps;
            let rapier = &mut self.rapier;
            let made = shape::caught(|| maps.build_cell(cell, rapier, &limits, charge));
            match made {
                // A panic caught inside the build (inserting a collider) may have left Rapier half changed, as one caught
                // here may: either stops the world.
                Err(e) | Ok(Err(e @ Error::Panicked)) => {
                    self.poisoned = true;
                    return Err(e);
                }
                Ok(Err(e)) => return Err(e),
                Ok(Ok(true)) => built += 1,
                Ok(Ok(false)) => {}
            }
        }
        Ok(built)
    }

    /// Adds a body. Every value is checked first (a non-finite one refuses the call) and held to the limits; a hull that
    /// cannot be made becomes its box ([`WorldStats::hulls_replaced`]). Insert bodies in id order (or with
    /// [`World::add_bodies`]): two worlds given the same bodies in another order step differently.
    pub fn add_body(
        &mut self,
        id: BodyId,
        desc: &BodyDesc,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), Error> {
        self.live()?;
        if self.bodies.contains_key(&id) {
            return Err(Error::Duplicate(id));
        }
        let l = self.config.limits;
        if self.bodies.len() >= l.max_bodies {
            return Err(Error::Cap(Cap::Bodies));
        }
        if self.rapier.colliders.len() >= l.max_colliders {
            return Err(Error::Cap(Cap::Colliders));
        }
        let position = guard::coords(desc.position, l.max_coordinate).ok_or(Error::NonFinite)?;
        let rotation = guard::rotation(desc.rotation).ok_or(Error::NonFinite)?;
        let linvel = guard::magnitude(desc.linvel, l.max_speed).ok_or(Error::NonFinite)?;
        let angvel = guard::magnitude(desc.angvel, l.max_angular_speed).ok_or(Error::NonFinite)?;
        let density =
            guard::scalar(desc.density, l.min_density, l.max_density).ok_or(Error::NonFinite)?;
        let friction = guard::scalar(desc.friction, 0.0, 10.0).ok_or(Error::NonFinite)?;
        let restitution = guard::scalar(desc.restitution, 0.0, 1.0).ok_or(Error::NonFinite)?;
        let lin_damp = guard::scalar(desc.linear_damping, 0.0, 1_000.0).ok_or(Error::NonFinite)?;
        let ang_damp = guard::scalar(desc.angular_damping, 0.0, 1_000.0).ok_or(Error::NonFinite)?;
        if !charge(work::BODY + work::CONVEX) {
            return Err(Error::Exhausted);
        }
        let built = shape::build(&desc.shape, &l, charge)?;
        let kind = match desc.motion {
            Motion::Dynamic => RigidBodyType::Dynamic,
            // Velocity-based: the velocities toward a target are this crate's ([`World::set_kinematic_target`]), so they are
            // checked like every other value.
            Motion::Kinematic => RigidBodyType::KinematicVelocityBased,
            Motion::Fixed => RigidBodyType::Fixed,
        };
        let body = RigidBodyBuilder::new(kind)
            .pose(Pose::from_parts(v3(position), quat(rotation)))
            .linvel(v3(linvel))
            .angvel(v3(angvel))
            .linear_damping(lin_damp)
            .angular_damping(ang_damp)
            .can_sleep(self.config.sleeping)
            .ccd_enabled(desc.ccd)
            .user_data(u128::from(id.0))
            .build();
        let collider = ColliderBuilder::new(built.shape)
            .translation(v3(built.offset))
            .density(density)
            .friction(friction)
            .restitution(restitution)
            .build();
        let radius = collider.shape().compute_local_bounding_sphere().radius
            + built.offset.iter().map(|v| v * v).sum::<f32>().sqrt();
        let handle = self.guarded(|r| {
            let h = r.insert_body(body);
            r.insert_collider(collider, Some(h));
            h
        })?;
        self.stats.hulls_replaced += usize::from(built.replaced);
        let good = BodyState {
            position,
            rotation,
            linvel,
            angvel,
            sleeping: false,
        };
        self.bodies.insert(
            id,
            Entry {
                handle,
                good,
                radius,
                arriving: false,
            },
        );
        Ok(())
    }

    /// Adds `bodies` in id order (a scratch world built from a set whose order is not the ids').
    pub fn add_bodies(
        &mut self,
        mut bodies: Vec<(BodyId, BodyDesc)>,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), Error> {
        bodies.sort_by_key(|(id, _)| *id);
        for (id, desc) in &bodies {
            self.add_body(*id, desc, charge)?;
        }
        Ok(())
    }

    /// Removes a body and its collider.
    pub fn remove_body(&mut self, id: BodyId) -> Result<(), Error> {
        self.live()?;
        let e = self.bodies.remove(&id).ok_or(Error::Unknown(id))?;
        self.guarded(|r| {
            r.remove_body(e.handle);
        })
    }

    /// A body's state.
    pub fn body(&self, id: BodyId) -> Option<BodyState> {
        let e = self.bodies.get(&id)?;
        let b = self.rapier.bodies.get(e.handle)?;
        let p = b.position();
        Some(BodyState {
            position: a3(p.translation),
            rotation: p.rotation.to_array(),
            linvel: a3(b.linvel()),
            angvel: a3(b.angvel()),
            sleeping: b.is_sleeping(),
        })
    }

    /// Sets a body's pose and velocities (checked and held like [`World::add_body`]'s); a body set moving is woken.
    pub fn set_body(&mut self, id: BodyId, s: &BodyState) -> Result<(), Error> {
        self.live()?;
        let l = self.config.limits;
        let e = *self.bodies.get(&id).ok_or(Error::Unknown(id))?;
        let position = guard::coords(s.position, l.max_coordinate).ok_or(Error::NonFinite)?;
        let rotation = guard::rotation(s.rotation).ok_or(Error::NonFinite)?;
        let linvel = guard::magnitude(s.linvel, l.max_speed).ok_or(Error::NonFinite)?;
        let angvel = guard::magnitude(s.angvel, l.max_angular_speed).ok_or(Error::NonFinite)?;
        let sleep = s.sleeping;
        self.guarded(|r| {
            if let Some(b) = r.bodies.get_mut(e.handle) {
                b.set_position(Pose::from_parts(v3(position), quat(rotation)), !sleep);
                b.set_linvel(v3(linvel), !sleep);
                b.set_angvel(v3(angvel), !sleep);
                if sleep {
                    b.sleep();
                }
            }
        })?;
        if let Some(entry) = self.bodies.get_mut(&id) {
            entry.good = BodyState {
                position,
                rotation,
                linvel,
                angvel,
                sleeping: sleep,
            };
        }
        Ok(())
    }

    /// Adds an impulse (mass times units a second) at the body's center of mass, its magnitude held to
    /// [`Limits::max_impulse`] and the speed it gives to [`Limits::max_speed`]; wakes the body. Refused
    /// ([`Error::NonFinite`], nothing changed) when the speed would not be finite (a body of next to no mass).
    pub fn apply_impulse(&mut self, id: BodyId, impulse: [f32; 3]) -> Result<(), Error> {
        self.live()?;
        let e = *self.bodies.get(&id).ok_or(Error::Unknown(id))?;
        let i =
            guard::magnitude(impulse, self.config.limits.max_impulse).ok_or(Error::NonFinite)?;
        let max_v = self.config.limits.max_speed;
        let made = self.guarded(|r| {
            let Some(b) = r.bodies.get_mut(e.handle) else {
                return true;
            };
            let before = b.linvel();
            b.apply_impulse(v3(i), true);
            // A body of next to no mass takes a speed past any range (or none): the impulse is refused, or the speed held.
            match guard::magnitude(a3(b.linvel()), max_v) {
                Some(v) => {
                    b.set_linvel(v3(v), true);
                    true
                }
                None => {
                    b.set_linvel(before, false);
                    false
                }
            }
        })?;
        if made { Ok(()) } else { Err(Error::NonFinite) }
    }

    /// The pose a kinematic body moves to over the next step (it stops there): the velocities that reach it in one
    /// [`TICK`], held to the speed limits. Without a target a kinematic body keeps the velocities it was given.
    pub fn set_kinematic_target(
        &mut self,
        id: BodyId,
        position: [f32; 3],
        rotation: [f32; 4],
    ) -> Result<(), Error> {
        self.live()?;
        let l = self.config.limits;
        let e = *self.bodies.get(&id).ok_or(Error::Unknown(id))?;
        let p = guard::coords(position, l.max_coordinate).ok_or(Error::NonFinite)?;
        let q = guard::rotation(rotation).ok_or(Error::NonFinite)?;
        let Some(now) = self.body(id) else {
            return Err(Error::Unknown(id));
        };
        let dt = f64::from(TICK);
        let v = [0, 1, 2].map(|k| ((f64::from(p[k]) - f64::from(now.position[k])) / dt) as f32);
        let v = guard::magnitude(v, l.max_speed).ok_or(Error::NonFinite)?;
        let w = guard::magnitude(turn(now.rotation, q, dt), l.max_angular_speed)
            .ok_or(Error::NonFinite)?;
        self.guarded(|r| {
            if let Some(b) = r.bodies.get_mut(e.handle)
                && b.is_kinematic()
            {
                b.set_linvel(v3(v), true);
                b.set_angvel(v3(w), true);
            }
        })?;
        if let Some(entry) = self.bodies.get_mut(&id) {
            entry.arriving = true;
        }
        Ok(())
    }

    /// Solver contact points the narrow phase holds now.
    pub fn contact_count(&self) -> usize {
        self.rapier
            .narrow_phase
            .contact_pairs()
            .filter(|p| p.has_any_active_contact())
            .flat_map(|p| p.manifolds().iter())
            .map(|m| m.data.solver_contacts.len())
            .sum()
    }

    /// One [`TICK`]: the map cells round every awake body built, then the world's sub-steps, each followed by the speed caps
    /// and the contact cap, then every body checked (one that left the finite range or the world's reach is put back at its
    /// last good pose, at rest). The work is charged before the sub-steps run: refused, nothing moves.
    pub fn step(&mut self, charge: &mut dyn FnMut(u64) -> bool) -> Result<StepReport, Error> {
        self.live()?;
        let mut report = StepReport::default();
        let l = self.config.limits;
        // The cells round the moving bodies, in id order.
        let mut regions = Vec::new();
        for e in self.bodies.values() {
            if !charge(work::BODY) {
                return Err(Error::Exhausted);
            }
            let Some(b) = self.rapier.bodies.get(e.handle) else {
                continue;
            };
            if b.is_fixed() || b.is_sleeping() {
                continue;
            }
            let p = b.translation();
            let reach = e.radius + b.linvel().length() * TICK + 1.0;
            if !(p.is_finite() && reach.is_finite()) {
                continue;
            }
            regions.push(Box3::new(
                [p.x - reach, p.y - reach, p.z - reach],
                [p.x + reach, p.y + reach, p.z + reach],
            ));
        }
        for region in &regions {
            match self.ensure_region(region, charge) {
                Ok(n) => report.cells_built += n,
                Err(Error::Exhausted | Error::Cap(_)) => report.map_capped = true,
                Err(e) => return Err(e),
            }
        }
        // The sub-steps' cost, paid before any runs.
        let awake = self
            .bodies
            .values()
            .filter_map(|e| self.rapier.bodies.get(e.handle))
            .filter(|b| b.is_dynamic_or_kinematic() && !b.is_sleeping())
            .count() as u64;
        let pairs = self.rapier.narrow_phase.contact_pairs().count() as u64;
        let per = awake * work::BODY_STEP + pairs * work::PAIR_STEP;
        if !charge(per.saturating_mul(u64::from(self.config.substeps))) {
            return Err(Error::Exhausted);
        }
        let handles: Vec<RigidBodyHandle> = self.bodies.values().map(|e| e.handle).collect();
        for _ in 0..self.config.substeps {
            self.guarded(|r| r.step())?;
            report.substeps += 1;
            let max_w = l.max_angular_speed;
            let max_v = l.max_speed;
            self.guarded(|r| {
                for &h in &handles {
                    let Some(b) = r.bodies.get_mut(h) else {
                        continue;
                    };
                    if !b.is_dynamic() || b.is_sleeping() {
                        continue;
                    }
                    // Held in f64 (a speed of huge components has an infinite f32 length); a non-finite one is left for the
                    // check after the sub-steps.
                    let w = a3(b.angvel());
                    if let Some(h) = guard::magnitude(w, max_w)
                        && h != w
                    {
                        b.set_angvel(v3(h), false);
                    }
                    let v = a3(b.linvel());
                    if let Some(h) = guard::magnitude(v, max_v)
                        && h != v
                    {
                        b.set_linvel(v3(h), false);
                    }
                }
            })?;
            report.contacts = self.contact_count();
            if report.contacts > l.max_contacts {
                report.contacts_capped = true;
                self.stats.contacts_capped += 1;
                break;
            }
        }
        // Kinematic bodies that reached their targets stop.
        let arrived: Vec<RigidBodyHandle> = self
            .bodies
            .values_mut()
            .filter_map(|e| std::mem::take(&mut e.arriving).then_some(e.handle))
            .collect();
        self.guarded(|r| {
            for h in arrived {
                if let Some(b) = r.bodies.get_mut(h) {
                    b.set_linvel(Vector::ZERO, false);
                    b.set_angvel(Vector::ZERO, false);
                }
            }
        })?;
        // Every body checked, in id order.
        let ids: Vec<BodyId> = self.bodies.keys().copied().collect();
        for id in ids {
            let Some(s) = self.body(id) else { continue };
            let ok = guard::finite(&s.position)
                && guard::finite(&s.rotation)
                && guard::finite(&s.linvel)
                && guard::finite(&s.angvel)
                && s.position.iter().all(|v| v.abs() <= l.max_coordinate);
            let entry = self.bodies.get_mut(&id).expect("an id of the map");
            if ok {
                entry.good = s;
                continue;
            }
            let back = BodyState {
                linvel: [0.0; 3],
                angvel: [0.0; 3],
                sleeping: false,
                ..entry.good
            };
            entry.good = back;
            let h = entry.handle;
            self.guarded(|r| {
                if let Some(b) = r.bodies.get_mut(h) {
                    b.set_enabled(true);
                    b.set_position(
                        Pose::from_parts(v3(back.position), quat(back.rotation)),
                        true,
                    );
                    b.set_linvel(Vector::ZERO, true);
                    b.set_angvel(Vector::ZERO, true);
                }
            })?;
            report.restored.push(id);
        }
        self.stats.restored += report.restored.len();
        Ok(report)
    }

    /// Every body's state as words, in id order (the id, then [`BodyState::words`]), for a trace hash.
    pub fn state_words(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.bodies.len() * 15);
        for &id in self.bodies.keys() {
            out.push(id.0);
            match self.body(id) {
                Some(s) => out.extend(s.words()),
                None => out.extend([u32::MAX; 14]),
            }
        }
        out
    }
}
