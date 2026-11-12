//! The render origin: the point of the world the GPU's coordinates are measured from, and [`Shift`], which moves every position
//! a frame holds by one vector.
//!
//! Positions are `f32`. At 50 km from the world's origin a step is 3.9 mm, and a vertex that far transformed by a view matrix
//! whose translation is as large lands about half a pixel off its neighbors, differently each frame as the camera moves (the
//! tests have the numbers: 0.18 px at 10 km, 0.5 px at 50 km, 1.5 px at 100 km, for a 1 m box 3 m ahead at 1920 x 1080). Measured
//! from an origin near the eye the same arithmetic is exact to a few thousandths of a pixel, and the renderer needs no change:
//! the frame is shifted by the origin's [`RenderOrigin::shift`] as it is built and the camera is built from the shifted eye.
//! The simulation's own `f32` positions still step by 3.9 mm at 50 km; that is the whole picture moving at once, not vertices
//! against each other.
//!
//! An origin sits on a grid of [`ORIGIN_GRID`] units on x and y (a multiple of the cells' side, so a cell's key moves by whole
//! cells) and stays 0 on z: heights stay the world's, as the sky, the clouds and the water read them.
//!
//! Limit: the volumetric clouds and the opt-in ground variation pattern their noise over the GPU's x and y, so a change of
//! origin moves their patterns by the origin's step unless the origin is also given to the frame (a job of whoever applies the
//! origin, not of this module).

use std::collections::BTreeMap;

use crate::hd::cells::{CELL_SIZE, Cell, StaticCells};
use crate::hd::sprites::GpuSprites;
use crate::hd::{
    DynamicObject, HdAreaLight, HdDraw, HdLight, HdScene, HdSpot, NodePlace, WaterZone, material,
};
use crate::math::Mat;
use crate::source::Eye;

/// The origin grid (units): 32 cells.
pub const ORIGIN_GRID: f32 = 32.0 * CELL_SIZE;

/// Where the GPU's coordinates are measured from: a world position whose x and y are multiples of [`ORIGIN_GRID`] and whose z
/// is 0.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RenderOrigin {
    pub position: [f32; 3],
}

impl RenderOrigin {
    /// The world's own origin: nothing moves.
    pub const WORLD: RenderOrigin = RenderOrigin { position: [0.0; 3] };

    /// The grid point nearest `p` on x and y.
    #[must_use]
    pub fn near(p: [f32; 3]) -> RenderOrigin {
        let snap = |v: f32| (v / ORIGIN_GRID).round() * ORIGIN_GRID;
        RenderOrigin {
            position: [snap(p[0]), snap(p[1]), 0.0],
        }
    }

    /// Whether `eye` is more than `reach` from this origin on x or y: time to pick a new one ([`RenderOrigin::near`]).
    #[must_use]
    pub fn is_far(&self, eye: [f32; 3], reach: f32) -> bool {
        let d = self.relative(eye);
        d[0].abs() > reach || d[1].abs() > reach
    }

    /// The vector that takes a world position to one measured from this origin.
    #[must_use]
    pub fn shift(&self) -> [f32; 3] {
        self.position.map(|v| -v)
    }

    /// `p` measured from this origin.
    #[must_use]
    pub fn relative(&self, p: [f32; 3]) -> [f32; 3] {
        shifted(p, self.shift())
    }

    /// A freshly built `s` measured from this origin, its eye too, so a camera built from the eye draws it as it was. For a
    /// scene the renderer has not drawn yet only: nothing here bumps a generation, clears a stamp or reaches the vertices of a
    /// resident cell that were dropped after upload, so a live scene shifted here would be drawn in two spaces at once.
    pub fn apply<G: Shift>(&self, s: &mut HdScene<G>) {
        s.shift(self.shift());
    }
}

/// `p + by`, in double precision: exact wherever the result is small, which is where it matters.
#[must_use]
pub fn shifted(p: [f32; 3], by: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|k| (f64::from(p[k]) + f64::from(by[k])) as f32)
}

/// Moves every world position by a vector, on a freshly built value only (see [`RenderOrigin::apply`]: no generation, stamp
/// or dropped vertex is touched). Each implementation takes its type apart field by field, so a new position field does not
/// compile until it is placed: the renderer draws what a frame holds, and a field left out would stay at the world's place
/// while the camera moved.
pub trait Shift {
    fn shift(&mut self, by: [f32; 3]);
}

impl Shift for [f32; 3] {
    fn shift(&mut self, by: [f32; 3]) {
        *self = shifted(*self, by);
    }
}

impl Shift for () {
    fn shift(&mut self, _by: [f32; 3]) {}
}

impl<T: Shift> Shift for Option<T> {
    fn shift(&mut self, by: [f32; 3]) {
        if let Some(v) = self {
            v.shift(by);
        }
    }
}

impl<T: Shift> Shift for Vec<T> {
    fn shift(&mut self, by: [f32; 3]) {
        for v in self {
            v.shift(by);
        }
    }
}

/// A draw at its eye distance (a corona, a particle set): the distance stays.
impl<T: Shift> Shift for (f32, T) {
    fn shift(&mut self, by: [f32; 3]) {
        self.1.shift(by);
    }
}

impl Shift for Eye {
    fn shift(&mut self, by: [f32; 3]) {
        let Eye {
            position,
            pitch: _,
            yaw: _,
            fov: _,
        } = self;
        position.shift(by);
    }
}

/// A placement: its translation (TGE row-major, the last column) moves; its rotation stays.
impl Shift for Mat {
    fn shift(&mut self, by: [f32; 3]) {
        let Mat { m } = self;
        let t = shifted([m[3], m[7], m[11]], by);
        m[3] = t[0];
        m[7] = t[1];
        m[11] = t[2];
    }
}

/// Every vertex in the world moves; one in its mesh's own space ([`material::NODE`]) is placed by its node, which moves.
impl Shift for HdDraw {
    fn shift(&mut self, by: [f32; 3]) {
        let HdDraw {
            label: _,
            vertices,
            indices: _,
            texture: _,
            blend: _,
            cull: _,
            depth_test: _,
            depth_write: _,
            // Tangents are directions: a shift leaves them.
            pbr: _,
        } = self;
        for v in vertices {
            if v.material & material::NODE == 0 {
                v.position.shift(by);
            }
        }
    }
}

/// The node's placements move; how its normals turn and its sphere in its own space stay.
impl Shift for NodePlace {
    fn shift(&mut self, by: [f32; 3]) {
        let NodePlace {
            now,
            before,
            normal: _,
            sphere: _,
            build: _,
        } = self;
        now.shift(by);
        before.shift(by);
    }
}

impl Shift for DynamicObject {
    fn shift(&mut self, by: [f32; 3]) {
        let DynamicObject {
            ghost: _,
            transform,
            prev_transform,
            translucent: _,
            centre,
            radius: _,
            draws,
            previous,
            stamp: _,
            nodes,
            takes_decals: _,
        } = self;
        transform.shift(by);
        prev_transform.shift(by);
        centre.shift(by);
        draws.shift(by);
        previous.shift(by);
        nodes.shift(by);
    }
}

impl Shift for HdLight {
    fn shift(&mut self, by: [f32; 3]) {
        let HdLight {
            position,
            color: _,
            radius: _,
            carrier: _,
        } = self;
        position.shift(by);
    }
}

impl Shift for HdSpot {
    fn shift(&mut self, by: [f32; 3]) {
        let HdSpot {
            light,
            direction: _,
            inner: _,
            outer: _,
            shadows: _,
        } = self;
        light.shift(by);
    }
}

impl Shift for HdAreaLight {
    fn shift(&mut self, by: [f32; 3]) {
        let HdAreaLight {
            light,
            shape: _,
            shadows: _,
        } = self;
        light.shift(by);
    }
}

impl Shift for WaterZone {
    fn shift(&mut self, by: [f32; 3]) {
        let WaterZone { min, max, tint: _ } = self;
        min.shift(by);
        max.shift(by);
    }
}

/// The cells' side in `by`, which must be whole cells on every axis (a key is whole cells).
fn cells_of(by: [f32; 3]) -> [i32; 3] {
    by.map(|v| {
        let cells = v / CELL_SIZE;
        debug_assert_eq!(cells.fract(), 0.0, "a cell shift must be whole cells: {v}");
        cells.round() as i32
    })
}

/// A cell moves with its key (`by` whole cells on every axis), its box, its vertices and its clusters' boxes.
impl<X> Shift for Cell<X> {
    fn shift(&mut self, by: [f32; 3]) {
        let Cell {
            key,
            bounds,
            vertices,
            indices: _,
            batches: _,
            clusters,
            generation: _,
            resident: _,
            ext: _,
        } = self;
        for (key, cells) in key.iter_mut().zip(cells_of(by)) {
            *key += cells;
        }
        bounds[0].shift(by);
        bounds[1].shift(by);
        for v in vertices {
            v.position.shift(by);
        }
        for c in clusters {
            c.bounds[0].shift(by);
            c.bounds[1].shift(by);
        }
    }
}

impl Shift for GpuSprites {
    fn shift(&mut self, by: [f32; 3]) {
        for s in &mut self.spawns {
            s.pos.shift(by);
        }
        for (o, b) in self.offset.iter_mut().zip(by) {
            *o += f64::from(b);
        }
    }
}

impl<X> Shift for StaticCells<X> {
    fn shift(&mut self, by: [f32; 3]) {
        let StaticCells { cells, slots: _ } = self;
        *cells = std::mem::take(cells)
            .into_values()
            .map(|mut c| {
                c.shift(by);
                (c.key, c)
            })
            .collect::<BTreeMap<_, _>>();
    }
}

impl<G: Shift> Shift for HdScene<G> {
    fn shift(&mut self, by: [f32; 3]) {
        let HdScene {
            eye,
            fov: _,
            visible_distance: _,
            fog_distance: _,
            sun: _,
            sky_sh: _,
            fog: _,
            shadow_distance: _,
            sun_radius: _,
            ao_radius: _,
            ao_sun: _,
            smooth_occlusion: _,
            ground_variation: _,
            cloud_time: _,
            clear: _,
            sky,
            sky_eye,
            sky_choice: _,
            atmosphere: _,
            painted_sun: _,
            atmosphere_sun: _,
            painted_key: _,
            follow_painted_sun: _,
            planes,
            water,
            water_zone,
            planes_generation: _,
            cells,
            objects,
            dissolves: _,
            drawn_objects,
            coronas,
            flare,
            particles,
            particles_generation: _,
            particle_light: _,
            sprites,
            textures_pending: _,
            lights,
            spots,
            area_lights,
            glow_lights: _,
            lights_generation: _,
            textures: _,
            materials: _,
            notes: _,
            generation: _,
            control_generation: _,
            clouds: _,
            cloud_look: _,
            size: _,
            gpu_nodes: _,
            lightmap: _,
            lightmap_generation: _,
            distorters,
            decals,
        } = self;
        eye.shift(by);
        sky.shift(by);
        sky_eye.shift(by);
        planes.shift(by);
        water.shift(by);
        water_zone.shift(by);
        cells.shift(by);
        objects.shift(by);
        drawn_objects.shift(by);
        coronas.shift(by);
        flare.shift(by);
        particles.shift(by);
        sprites.shift(by);
        lights.shift(by);
        spots.shift(by);
        area_lights.shift(by);
        distorters.shift(by);
        decals.shift(by);
    }
}

#[cfg(test)]
#[path = "origin_tests.rs"]
mod tests;
