//! The caps a world holds to and the magnitudes values are clamped to, and a world's fixed settings.

/// Caps and magnitudes. Every count a call could grow is capped here; every float that reaches Rapier is held to one of these
/// magnitudes first ([`crate::guard`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// Bodies in the world (dynamic, kinematic and fixed).
    pub max_bodies: usize,
    /// Colliders in the world: one a body, plus every map piece's.
    pub max_colliders: usize,
    /// Points a convex hull is made from; a hull of more is replaced by its box.
    pub max_hull_vertices: usize,
    /// Hulls one compound shape holds; a compound of more is replaced by the box of its points.
    pub max_compound_hulls: usize,
    /// Solver contact points after a sub-step; past it the tick's remaining sub-steps are not run
    /// ([`crate::StepReport::contacts_capped`]).
    pub max_contacts: usize,
    /// Map cells one region query may build or look at (a box wider than this many cells is cut to the cells nearest its
    /// middle).
    pub max_cells_per_query: usize,
    /// How far from the origin (units, each axis) a position may be; a body that goes past it is put back.
    pub max_coordinate: f32,
    /// A body's linear speed (units a second).
    pub max_speed: f32,
    /// A body's angular speed (radians a second).
    pub max_angular_speed: f32,
    /// A shape's half-extent: a box's half-size, a ball's radius, a hull point's distance from the body's origin.
    pub max_half_extent: f32,
    /// The thinnest half-extent a shape keeps (thinner boxes and balls are thickened to it; flatter hulls become boxes).
    pub min_half_extent: f32,
    /// Density (mass a cubic unit), at least [`Limits::min_density`].
    pub max_density: f32,
    pub min_density: f32,
    /// An impulse's magnitude (mass times units a second).
    pub max_impulse: f32,
    /// Joints in the world.
    pub max_joints: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_bodies: 1_024,
            max_colliders: 16_384,
            max_hull_vertices: torque_collide::mesh::MAX_HULL_VERTS,
            max_compound_hulls: 64,
            max_contacts: 16_384,
            max_cells_per_query: 64,
            max_coordinate: 100_000.0,
            max_speed: 500.0,
            max_angular_speed: 200.0,
            max_half_extent: 512.0,
            min_half_extent: 0.005,
            max_density: 100_000.0,
            min_density: 0.001,
            max_impulse: 10_000_000.0,
            max_joints: 4_096,
        }
    }
}

/// The most sub-steps a tick may be cut into.
pub const MAX_SUBSTEPS: u8 = 8;

/// A world's fixed settings: chosen when it is made, the same on every peer that simulates it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    /// The sub-steps of one [`crate::TICK`], 1 to [`MAX_SUBSTEPS`] (held to that range).
    pub substeps: u8,
    /// Gravity (units a second squared); Torque's is 20 down the z axis.
    pub gravity: [f32; 3],
    /// Rapier's solver iterations a sub-step, 1 to 16 (held to that range).
    pub solver_iterations: u8,
    /// Whether bodies may sleep. A persistent world keeps it on; a scratch world made for one tick turns it off.
    pub sleeping: bool,
    pub limits: Limits,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            substeps: 4,
            gravity: [0.0, 0.0, -20.0],
            solver_iterations: 4,
            sleeping: true,
            limits: Limits::default(),
        }
    }
}
