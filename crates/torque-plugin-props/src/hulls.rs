//! Props that collide as their shape's collision meshes ([`PropData::collision`](crate::PropData) `= "shape"`): the convex hulls
//! a `.dts`'s `Collision-N` details hold, taken once from the shape, held to caps, and given to the physics world as one
//! compound shape.
//!
//! **Where the hulls come from.** Every collision detail the shape names (up to eight, as the engine's own shape classes keep
//! them) has one mesh an object; each mesh is one convex hull, its points through its object's node at the shape's rest pose,
//! times the datablock's `drawScale`. A server reads the shape through the engine's capped loader
//! ([`torque_game::shapes::Shapes::load_shape`], [`HullBook::load`]); a predicting client reads the same file through its
//! connection's shape cache ([`PropHulls::of_client`]). Both end in [`PropHulls::of_parts`], so for one shape both hold the same
//! hulls to the bit (`a_server_and_a_client_hold_the_same_hulls_for_a_shape`).
//!
//! **Caps** ([`HullCaps`]). At most [`MAX_HULLS`] hulls a shape, [`MAX_HULL_VERTICES`] distinct points a hull, every point within
//! [`MAX_COORD`] of the shape's origin once scaled, and on a server [`MAX_SERVER_VERTICES`] points in all the shapes it has loaded.
//! A shape past a cap is refused ([`Refusal`]): its props collide as the datablock's box and the server prints a note. A shape
//! with no collision mesh collides as its bounds (one box hull) and the server prints a note.
//!
//! **Work.** Reading the points and making the volume are charged to the caller's budget ([`PropHulls::of_parts`]: the server's
//! script budget, a client's tick for parts), and a refusal is [`Refusal::Budget`], which is never kept: the build is tried again.
//!
//! **The client agrees with the server.** Whatever the server ends up colliding a prop as (the hulls, or the datablock's box
//! because of a cap, its total, a file it cannot read) goes to each client with the prop, as a [`HullDigest`] (none: the box). A
//! client predicts the hulls only if its own build of the shape has that digest, the box when the digest is none, and leaves the
//! prop to the server (not predicted, drawn as sent) when its build differs or fails ([`choose`]): another file under the path,
//! a file it lacks, its own budget or caps, a shape the server refused that it took. The server's total, which the client does
//! not hold, is one such case.

use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

use torque_collide::math::mul_p;
use torque_collide::mesh::MeshPart;
use torque_game::shapes::Loaded;
use torque_physics_rapier::{Limits, Shape, work};
use torque_ts_vm::vm::Vm;

/// The collision details a shape's hulls are taken from (the engine's `MaxCollisionShapes`), as a client's shape cache keeps them.
const SLOTS: usize = 8;

/// The most hulls one shape gives.
pub const MAX_HULLS: usize = 32;
/// The most distinct points one hull may have.
pub const MAX_HULL_VERTICES: usize = 64;
/// How far from the shape's origin a point may be (units, after the draw scale).
pub const MAX_COORD: f32 = 256.0;
/// The most points all the shapes a server has loaded may have between them.
pub const MAX_SERVER_VERTICES: usize = 65_536;

/// The caps a shape's hulls are held to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HullCaps {
    /// Hulls a shape.
    pub hulls: usize,
    /// Distinct points a hull.
    pub vertices: usize,
    /// Points in all the shapes a [`HullBook`] holds.
    pub total: usize,
}

impl Default for HullCaps {
    /// [`MAX_HULLS`], [`MAX_HULL_VERTICES`] and [`MAX_SERVER_VERTICES`].
    fn default() -> HullCaps {
        HullCaps {
            hulls: MAX_HULLS,
            vertices: MAX_HULL_VERTICES,
            total: MAX_SERVER_VERTICES,
        }
    }
}

/// Why a shape gave no hulls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The shape has more collision meshes than [`HullCaps::hulls`].
    TooManyHulls { found: usize },
    /// A collision mesh has more distinct points than [`HullCaps::vertices`].
    TooManyVertices { found: usize },
    /// A point is not a number or lies past [`MAX_COORD`] (a hostile file, or a draw scale that stretches it).
    BadPoint,
    /// The shape has neither a collision mesh nor usable bounds.
    NoBounds,
    /// The shapes the server holds would have more than [`HullCaps::total`] points.
    ServerTotal,
    /// The file is not there, is not a shape the engine reads whole, or failed to load (its own error line says why).
    Unreadable,
    /// The script's work budget ran out in the read.
    Budget,
    /// The script engine has no shape loader.
    NoLoader,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::TooManyHulls { found } => {
                write!(f, "{found} collision meshes (at most {MAX_HULLS})")
            }
            Refusal::TooManyVertices { found } => {
                write!(
                    f,
                    "a collision mesh has {found} points (at most {MAX_HULL_VERTICES})"
                )
            }
            Refusal::BadPoint => write!(f, "a collision point is not a number or is too far out"),
            Refusal::NoBounds => write!(f, "no collision mesh and no bounds"),
            Refusal::ServerTotal => write!(f, "the server holds its most collision points"),
            Refusal::Unreadable => write!(f, "the shape is not there or does not load"),
            Refusal::Budget => write!(f, "the work budget ran out"),
            Refusal::NoLoader => write!(f, "no shape loader"),
        }
    }
}

/// A shape's collision hulls, in the shape's space (the draw scale applied): what a prop of `collision = "shape"` collides as.
#[derive(Clone, Debug, PartialEq)]
pub struct PropHulls {
    hulls: Rc<[Vec<[f32; 3]>]>,
    volume: f32,
    bounds_only: bool,
}

impl PropHulls {
    /// The hulls of the collision meshes `slots` (each the parts of one collision detail, the shape's own `bounds` as
    /// `[min, max]`, `scale` the draw scale), held to `caps`. A shape with no point in any part gets the box of `bounds`.
    ///
    /// # Errors
    /// A [`Refusal`] for a shape past a cap or with a point that is not usable, or [`Refusal::Budget`] when `charge` (which is
    /// paid the work of reading every point and of making the hulls' volume, before it is done) refuses.
    pub fn of_parts(
        slots: &[&[MeshPart]],
        bounds: [f32; 6],
        scale: [f32; 3],
        caps: &HullCaps,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<PropHulls, Refusal> {
        let mut hulls: Vec<Vec<[f32; 3]>> = Vec::new();
        for part in slots.iter().flat_map(|parts| parts.iter()) {
            if part.verts.is_empty() {
                continue;
            }
            if !charge(work::VERTEX.saturating_mul(part.verts.len() as u64)) {
                return Err(Refusal::Budget);
            }
            if hulls.len() >= caps.hulls {
                return Err(Refusal::TooManyHulls {
                    found: slots.iter().map(|p| p.len()).sum(),
                });
            }
            let points = distinct_points(part, scale)?;
            if points.len() > caps.vertices {
                return Err(Refusal::TooManyVertices {
                    found: points.len(),
                });
            }
            hulls.push(points);
        }
        let bounds_only = hulls.is_empty();
        if bounds_only {
            hulls.push(bounds_box(bounds, scale)?);
        }
        let hulls: Rc<[Vec<[f32; 3]>]> = hulls.into();
        let volume = Shape::Compound {
            hulls: Rc::clone(&hulls),
        }
        .volume(&Limits::default(), charge)
        .map_err(|e| match e {
            torque_physics_rapier::Error::Exhausted => Refusal::Budget,
            _ => Refusal::BadPoint,
        })?;
        Ok(PropHulls {
            hulls,
            volume,
            bounds_only,
        })
    }

    /// The hulls of a shape a predicting client read ([`torque_client::predict::drive::ClientShape`]), at `scale`.
    ///
    /// # Errors
    /// As [`PropHulls::of_parts`].
    pub fn of_client(
        shape: &torque_client::predict::drive::ClientShape,
        scale: [f32; 3],
        caps: &HullCaps,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<PropHulls, Refusal> {
        let slots: Vec<&[MeshPart]> = shape
            .hulls
            .iter()
            .flatten()
            .map(|hull| hull.parts.as_slice())
            .collect();
        PropHulls::of_parts(&slots, shape.shape.bounds, scale, caps, charge)
    }

    /// The hulls, each its distinct points.
    pub fn hulls(&self) -> &[Vec<[f32; 3]>] {
        &self.hulls
    }

    /// What the hulls hold, as the physics world makes them (units cubed).
    pub fn volume(&self) -> f32 {
        self.volume
    }

    /// Points in all the hulls.
    pub fn vertex_count(&self) -> usize {
        self.hulls.iter().map(Vec::len).sum()
    }

    /// Whether the shape had no collision mesh and these are its bounds.
    pub fn is_bounds_only(&self) -> bool {
        self.bounds_only
    }

    /// The physics world's shape (it shares the hulls).
    pub fn shape(&self) -> Shape {
        Shape::Compound {
            hulls: Rc::clone(&self.hulls),
        }
    }

    /// The hulls in a few numbers, which a server sends with each prop so a client can tell whether it built the same ones
    /// ([`choose`]).
    pub fn digest(&self) -> HullDigest {
        HullDigest {
            hulls: u8::try_from(self.hulls.len()).unwrap_or(u8::MAX),
            points: u16::try_from(self.vertex_count()).unwrap_or(u16::MAX),
            volume: self.volume.to_bits(),
        }
    }
}

/// How many hulls, how many points and the bits of the volume of a [`PropHulls`]: what the server's effective collision came to,
/// as a client is told it. Two builds of one shape agree to the bit, so equal digests mean the same hulls for all it matters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct HullDigest {
    pub hulls: u8,
    pub points: u16,
    pub volume: u32,
}

/// What a client does about a prop whose server collides it as `expected` (the digest of its hulls; none: as the datablock's box),
/// given what the client's own build of the shape came to (`built`: none while the shape is not read yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Predict it as the box.
    Box,
    /// Predict it with the hulls it built.
    Hulls,
    /// The client's hulls are not the server's (it read another file, was refused, ran out of budget, or the server refused a
    /// shape the client took): the prop is not predicted, and is drawn as the server sends it.
    Unfit,
    /// The client has not read the shape yet and may.
    Wait,
}

/// The [`Choice`] for a prop whose server says `expected` and a client that built `built`.
pub fn choose(expected: Option<HullDigest>, built: Option<&Made>) -> Choice {
    match (expected, built) {
        (None, _) => Choice::Box,
        (Some(_), None) => Choice::Wait,
        (Some(want), Some(Ok(hulls))) if hulls.digest() == want => Choice::Hulls,
        (Some(_), Some(_)) => Choice::Unfit,
    }
}

/// A mesh part's points through its node, scaled, each distinct one once, in a fixed order (by the bits of x, y and z).
fn distinct_points(part: &MeshPart, scale: [f32; 3]) -> Result<Vec<[f32; 3]>, Refusal> {
    let mut points = Vec::with_capacity(part.verts.len());
    for &v in &part.verts {
        let p = mul_p(&part.node, v);
        // Adding 0.0 turns -0.0 into 0.0, so the two do not count as two points.
        let p: [f32; 3] = std::array::from_fn(|k| p[k] * scale[k] + 0.0);
        if p.iter().any(|c| !c.is_finite() || c.abs() > MAX_COORD) {
            return Err(Refusal::BadPoint);
        }
        points.push(p);
    }
    points.sort_by(|a, b| {
        a[0].total_cmp(&b[0])
            .then(a[1].total_cmp(&b[1]))
            .then(a[2].total_cmp(&b[2]))
    });
    points.dedup_by(|a, b| {
        a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.to_bits() == y.to_bits())
    });
    Ok(points)
}

/// The eight corners of the shape's bounds `[min, max]`, scaled.
fn bounds_box(bounds: [f32; 6], scale: [f32; 3]) -> Result<Vec<[f32; 3]>, Refusal> {
    let [lo, hi] = [[0, 1, 2], [3, 4, 5]].map(|ks| ks.map(|k| bounds[k]));
    let usable = (0..3).all(|k| lo[k].is_finite() && hi[k].is_finite() && lo[k] <= hi[k]);
    if !usable {
        return Err(Refusal::NoBounds);
    }
    let corner = |i: usize| -> [f32; 3] {
        std::array::from_fn(|k| (if i & (1 << k) == 0 { lo[k] } else { hi[k] }) * scale[k] + 0.0)
    };
    let corners: Vec<[f32; 3]> = (0..8).map(corner).collect();
    if corners
        .iter()
        .flatten()
        .any(|c| !c.is_finite() || c.abs() > MAX_COORD)
    {
        return Err(Refusal::BadPoint);
    }
    Ok(corners)
}

/// What making the hulls of a shape came to.
pub type Made = Result<Rc<PropHulls>, Refusal>;

/// The hulls a server has made, by shape and scale: each shape read once whatever the number of props, and the points of all
/// of them held to [`HullCaps::total`].
#[derive(Debug, Default)]
pub struct HullBook {
    caps: HullCaps,
    sets: BTreeMap<(Vec<u8>, [u32; 3]), Made>,
    vertices: usize,
}

impl HullBook {
    /// A book that holds shapes to `caps`.
    pub fn new(caps: HullCaps) -> HullBook {
        HullBook {
            caps,
            ..HullBook::default()
        }
    }

    /// The caps the book holds shapes to.
    pub fn caps(&self) -> &HullCaps {
        &self.caps
    }

    /// Points in all the shapes the book holds.
    pub fn vertices(&self) -> usize {
        self.vertices
    }

    /// The hulls of the shape file `path` at draw scale `scale`: the book's if it has made them (a refusal is kept too, a file
    /// that is not there as well), else read through the script engine's loader and charged to its budget. A budget that ran
    /// out is refused but not kept, so a later call tries again.
    ///
    /// # Errors
    /// A [`Refusal`].
    pub fn load(&mut self, vm: &mut Vm, path: &str, scale: [f32; 3]) -> Made {
        let key = (
            path.to_ascii_lowercase().into_bytes(),
            scale.map(f32::to_bits),
        );
        if let Some(made) = self.sets.get(&key) {
            return made.clone();
        }
        let made = read(vm, path.as_bytes(), scale, &self.caps).and_then(|hulls| {
            if self.vertices.saturating_add(hulls.vertex_count()) > self.caps.total {
                return Err(Refusal::ServerTotal);
            }
            Ok(Rc::new(hulls))
        });
        match &made {
            Ok(hulls) => self.vertices += hulls.vertex_count(),
            Err(Refusal::Budget | Refusal::NoLoader) => return made,
            Err(_) => {}
        }
        self.sets.insert(key, made.clone());
        made
    }
}

/// The hulls of the shape at `path`, read as a shape datablock's preload reads one.
fn read(vm: &mut Vm, path: &[u8], scale: [f32; 3], caps: &HullCaps) -> Result<PropHulls, Refusal> {
    let shapes = torque_game::shapes::shapes_of(vm).ok_or(Refusal::NoLoader)?;
    let kept = match shapes.load_shape(vm, path) {
        Ok(Loaded::Shape(kept)) => kept,
        Ok(_) => return Err(Refusal::Unreadable),
        Err(_) => return Err(Refusal::Budget),
    };
    // A shape the engine keeps header-only (an old version) has no body to take hulls from.
    let (Some(shape), Some(facts)) = (&kept.shape, &kept.facts) else {
        return Err(Refusal::Unreadable);
    };
    let mut pay = |units: u64| vm.charge_work(units);
    let nodes =
        torque_game::scene::container_world::rest_nodes(&kept, &mut pay).ok_or(Refusal::Budget)?;
    let mut parts = Vec::new();
    for &detail in facts
        .slots
        .collision
        .iter()
        .take(SLOTS)
        .filter(|&&d| d >= 0)
    {
        let slot = torque_game::scene::rays::detail_parts(shape, &nodes, detail, &mut pay)
            .map_err(|_| Refusal::Budget)?;
        parts.push(slot);
    }
    let slots: Vec<&[MeshPart]> = parts.iter().map(Vec::as_slice).collect();
    PropHulls::of_parts(&slots, shape.bounds, scale, caps, &mut pay)
}

#[cfg(test)]
#[path = "hulls_tests.rs"]
mod tests;
