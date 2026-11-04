//! A WheeledVehicle's tires, drawn after its body ([`tire_draws`]).
//!
//! Credit: follows TGE 1.4's `WheeledVehicle::renderImage` and `extendWheels` (`game/vehicles/wheeledVehicle.cc`, Copyright (C)
//! GarageGames.com, Inc.), as Torque3D carries them under the MIT licence (`T3D/vehicles/wheeledVehicle.cpp`, Copyright (c)
//! 2012 GarageGames, LLC); the code here is this crate's own.

use std::collections::BTreeMap;

use ghost_world::world::VehicleLook;
use shapes::instance::ShapeInstance;
use shapes::shape::Model;

use crate::math::Mat;
use crate::shapes::{
    DetailPolicy, Models, ShapeParts, detail_for, mat3_scale, shape_draws_keyed, shape_key_bytes,
};
use crate::textures::TextureSet;
use crate::world::{ShapeWorld, Wheels};
use crate::{Assets, Draw, PointLight};

/// What the tires of a WheeledVehicle are drawn from.
pub struct Tires<'a, W: ShapeWorld + ?Sized> {
    pub world: &'a W,
    pub assets: &'a dyn Assets,
    /// The wheels its record set up.
    pub wheels: Wheels,
    /// The client's own state of a vehicle it runs (its steering, the wheels' spin and extension), else the record's.
    pub look: Option<&'a VehicleLook>,
    pub vehicle: &'a Model,
    pub instance: &'a ShapeInstance,
    /// The vehicle's render transform (no scale) and its scale.
    pub xform: &'a Mat,
    pub scale: [f32; 3],
    pub eye: [f32; 3],
    pub pixels_per_meter: f32,
    pub point: &'a [PointLight],
    pub policy: DetailPolicy,
    /// The tyres' kept builds stay in their meshes' own space ([`crate::shapes::MeshPlace`]).
    pub nodes: bool,
}

/// `WheeledVehicle::renderImage`: after the body, each wheel with a tire shape at its hub (the vehicle's `hub<i>` node in the
/// pose the body is drawn in; TGE's `WheeledVehicleData::preload` takes it in the default pose), turned by the steering
/// (`EulerF(0, 0, steering * the wheel's steering)`), let down its spring by `length * extension`, spun by
/// `EulerF(apos * 2pi, 0, 0)` and turned to face out (`EulerF(0, 0, pi/2)` for a hub at positive x, `-pi/2` else); TGE's tyres
/// and hubs take no scale of the vehicle (a game's may: [`ShapeWorld::scaled_tires`]). A vehicle the client runs
/// (`ClientWorld::vehicle_look`) is drawn with its own steering, its wheels' spin (`apos`, advanced each frame) and the extension
/// its `extendWheels` found; any other with the record's steering, unspun, its extension a ray from the hub down the vehicle's z
/// axis, the spring's length plus the tire's radius long, cast against the world's ground ([`ShapeWorld::ground_height`]; none:
/// the springs at full length). Each tyre's detail is picked by its distance as a body's is (this crate's choice: TGE draws a
/// tyre's detail 0).
pub fn tire_draws<W: ShapeWorld + ?Sized>(
    w: &Tires<'_, W>,
    models: &mut Models,
    textures: &mut TextureSet<'_>,
    notes: &mut Vec<String>,
) -> Vec<Draw> {
    let wheel_dbs = &w.wheels.wheels;
    let s = &w.vehicle.shape;
    let hubs: Vec<[f32; 3]> = (0..8)
        .filter_map(|i| {
            let name = s.find_name(format!("hub{i}").as_bytes())?;
            let node = s.nodes.iter().position(|n| n.name == name)?;
            w.instance.node_transform(node).map(|m| [m[3], m[7], m[11]])
        })
        .collect();
    let ground = w.world.ground_height();
    // The steering as the record's update unpacked it; the client's own when it runs the vehicle.
    let steering = match w.look {
        Some(l) if l.steering.is_finite() => l.steering,
        _ => w.wheels.steering,
    };
    let scale = if w.world.scaled_tires() {
        w.scale
    } else {
        [1.0; 3]
    };
    let [sx, sy, sz] = scale;
    let mut out = Vec::new();
    for (i, wd) in wheel_dbs.iter().enumerate() {
        let (Some(wd), Some(pos)) = (wd, hubs.get(i)) else {
            continue;
        };
        let Some(file) = w.world.text(wd.tire, "shapeFile") else {
            continue;
        };
        let radius = w.world.float(wd.tire, "radius", 0).unwrap_or(0.0);
        let length = w.world.float(wd.spring, "length", 0).unwrap_or(0.0);
        // extendWheels: the hit fraction along the ray, then the extension.
        let own = w.look.and_then(|l| l.wheels.get(i)).filter(|l| l.fitted);
        let mut extension = 1.0f32;
        if let Some(l) = own {
            extension = if l.extension.is_finite() {
                l.extension.clamp(0.0, 1.0)
            } else {
                1.0
            };
        } else if length > 0.0 {
            let start = w.xform.mul_p([sx * pos[0], sy * pos[1], sz * pos[2]]);
            let kk = radius * sz / length;
            let down = w.xform.mul_v([0.0, 0.0, -length * (1.0 + kk)]);
            let end = [start[0] + down[0], start[1] + down[1], start[2] + down[2]];
            if let Some(g) = ground
                && start[2] >= g
                && end[2] < g
            {
                let t = (start[2] - g) / (start[2] - end[2]);
                let kf = kk / (kk + 1.0);
                extension = if kf <= t { (t - kf) / (1.0 - kf) } else { 0.0 };
            }
        }
        let mut hub = Mat::euler_z(steering * wd.steering);
        hub.set_column3([sx * pos[0], sy * pos[1], sz * pos[2] - length * extension]);
        let apos = own.map_or(0.0, |l| if l.apos.is_finite() { l.apos } else { 0.0 });
        let spin = Mat::euler_x(apos * std::f32::consts::TAU);
        let face = Mat::euler_z(if pos[0] > 0.0 {
            std::f32::consts::FRAC_PI_2
        } else {
            -std::f32::consts::FRAC_PI_2
        });
        let rotation = Mat::mul(&Mat::mul(w.xform, &hub), &Mat::mul(&spin, &face));
        let full = mat3_scale(&rotation, scale);
        let path = String::from_utf8_lossy(file).replace('\\', "/");
        let loaded = models.get(
            &|p| w.world.records().constructor_sequences(p),
            w.assets,
            &path,
            notes,
        );
        let Some(loaded) = loaded else {
            notes.push(format!("tire {path}: not loaded"));
            continue;
        };
        let model = &loaded.model;
        let mut charge = |_: u64| true;
        let Ok(instance) = ShapeInstance::new(&model.shape, &mut charge) else {
            continue;
        };
        let centre = full.mul_p(model.shape.center);
        let d = [0, 1, 2].map(|k| centre[k] - w.eye[k]);
        let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            / scale.iter().copied().fold(0.0, f32::max).max(1e-6);
        let Some(dl) = detail_for(model, distance, w.pixels_per_meter, w.policy) else {
            continue;
        };
        let detail = model.shape.details[dl];
        let none = BTreeMap::new();
        let parts = || ShapeParts {
            sub_shape: detail.sub_shape,
            object_detail: detail.object_detail,
            hidden: &[],
            colors: None,
            ifl_frames: &none,
            hidden_objects: None,
            skin: None,
            color_all: None,
        };
        let unscaled = scale == [1.0; 3];
        let nodes = loaded.node_spheres(w.nodes);
        let key = crate::shapecache::on().then(|| {
            shape_key_bytes(
                model,
                &instance,
                &parts(),
                &full,
                &rotation,
                unscaled,
                w.point,
                nodes.is_some(),
            )
        });
        let (draws, _) = shape_draws_keyed(
            model,
            &instance,
            parts(),
            &full,
            &rotation,
            unscaled,
            w.point,
            textures,
            &format!("tire {i} {path}"),
            key,
            nodes,
        );
        out.extend(draws);
    }
    out
}

#[cfg(test)]
#[path = "wheels_tests.rs"]
mod tests;
