//! `TSStatic` and `VehicleBlocker` (SceneObject-derived; no mask, no GameBase part): every update is the whole object.
//!
//! ```text
//! TSStatic:       F32 x16 (matrix), F32 x3 (scale), string255 (shape name)
//! VehicleBlocker: F32 x16 (matrix), F32 x3 (scale), F32 x3 (dimensions) = 22 floats, 704 bits
//! ```
//!
//! **View fields:** `matrix` 0..16, `scale` 0..3; TSStatic `shape`; VehicleBlocker `box` 0..3.
//!
//! Credit: these follow TGE 1.4's `game/tsStatic.cc` and `game/vehicles/vehicleBlocker.cc`, carried into the MIT-licensed
//! Torque3D as `T3D/tsStatic.cpp` (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.) and
//! `T3D/vehicles/vehicleBlocker.cpp` (Copyright (c) 2012 GarageGames, LLC); the code here is this crate's own.

use crate::codec::{Codec, F32, Layout, R};
use crate::{ObjectView, raws, text};

/// The object's transform as a 4x4 matrix of 16 floats (`mathWrite(MatrixF)`).
pub fn matrix(view: &dyn ObjectView) -> [F32; 16] {
    raws::<16>(view, "matrix", 0)
}

/// `TSStatic`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TsStatic {
    pub matrix: [F32; 16],
    pub scale: [F32; 3],
    pub shape: Vec<u8>,
}

impl Layout for TsStatic {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.floats("ts.matrix", &mut self.matrix)?;
        c.floats("ts.scale", &mut self.scale)?;
        c.string("ts.shape", &mut self.shape)
    }
}

pub fn pack_ts_static(view: &dyn ObjectView) -> TsStatic {
    TsStatic {
        matrix: matrix(view),
        scale: raws::<3>(view, "scale", 0),
        shape: text(view, "shape", 0),
    }
}

/// `VehicleBlocker`: 22 floats, every update.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VehicleBlocker {
    pub matrix: [F32; 16],
    pub scale: [F32; 3],
    pub size: [F32; 3],
}

impl Layout for VehicleBlocker {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        c.floats("blocker.matrix", &mut self.matrix)?;
        c.floats("blocker.scale", &mut self.scale)?;
        c.floats("blocker.size", &mut self.size)
    }
}

pub fn pack_vehicle_blocker(view: &dyn ObjectView) -> VehicleBlocker {
    VehicleBlocker {
        matrix: matrix(view),
        scale: raws::<3>(view, "scale", 0),
        size: raws::<3>(view, "box", 0),
    }
}
