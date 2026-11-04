//! `ShapeBase::updateContainer` and its PhysicalZone callback, for a Player.
//! The statics shared across objects are the host's [`LiquidStatics`](super::host::LiquidStatics).
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/shapeBase.cpp` and
//! `T3D/physicalZone.cpp` (each Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is
//! this crate's own.

use torque_collide::math::{F3, IDENTITY};
use torque_collide::poly::point_len;
use torque_collide::{ClippedPolyList, PolyList};

use super::PlayerState;
use super::data::PlayerData;
use super::ext::{PlayerClass, TorquePlayer};
use super::host::{PlayerHost, ZoneView};

/// `Point3F::normalizeSafe`: unchanged when the length is `1e-4` or less.
fn normalize_safe(v: F3) -> F3 {
    let l = point_len(v);
    if l > 1.0e-4 {
        let inv = 1.0 / l;
        [v[0] * inv, v[1] * inv, v[2] * inv]
    } else {
        v
    }
}

/// `Player::buildPolyList` into the zone's list: the subclass's zone box (Torque: the object box) at the position
/// (identity rotation, unit scale), clipped by the zone's planes. True when nothing is left (`isEmpty`).
fn zone_misses_player(
    s: &PlayerState,
    zone: &ZoneView,
    list: &mut ClippedPolyList,
    class: &dyn PlayerClass,
) -> bool {
    list.clear();
    list.base.reset();
    list.planes.clear();
    list.planes.extend_from_slice(&zone.planes);
    let p = s.position();
    let mut m = IDENTITY;
    m[3] = p[0];
    m[7] = p[1];
    m[11] = p[2];
    list.set_transform(&m, [1.0, 1.0, 1.0]);
    list.set_object(Some(s.id));
    list.add_box(&class.zone_box(s));
    list.is_empty()
}

/// The zone callback's coverage: `acc` is the accumulator, `[zb, zt]` the Player's extent, `[bot, top]` the zone's. Above the top
/// (not when equal or unordered): 1.0 when the accumulator is 0.99, else `mMax(acc, (top - zb) / (zt - zb))`. Otherwise, the
/// zone bottom at or below the feet (unordered too): 1.0; else, the feet below the zone, `acc > 0` **and `acc != 0.99`** (taken to
/// the `mMax` when equal): 1.0, else `mMax(acc, 0.99)`.
fn zone_coverage(acc: f32, zb: f32, zt: f32, bot: f32, top: f32) -> f32 {
    let mmax = |a: f32, b: f32| if b < a { a } else { b };
    if zt > top {
        if acc == 0.99 {
            1.0
        } else {
            let frac = (top - zb) / (zt - zb);
            mmax(acc, frac)
        }
    } else if !(bot > zb) || (acc > 0.0 && acc != 0.99) {
        1.0
    } else {
        mmax(acc, 0.99)
    }
}

pub(super) fn update_container(
    s: &mut PlayerState,
    db: &PlayerData,
    zones: &mut Vec<ZoneView>,
    list: &mut ClippedPolyList,
    host: &mut dyn PlayerHost,
) {
    {
        let st = host.liquid_statics();
        st.coverage = 0.0;
        st.color = [0.2, 0.6, 0.6, 0.3];
    }
    s.drag = db.drag;
    s.buoyancy = 0.0;
    s.gravity_mod = 1.0;
    s.applied_force = [0.0; 3];
    s.drag2 = 0.0;
    if s.mount.is_some() {
        s.water_coverage = 0.0;
        return;
    }
    // The query box: the subclass's (Torque: the world box).
    let class = host.player_class();
    let class: &dyn PlayerClass = class.as_deref().unwrap_or(&TorquePlayer);
    let query = class.container_box(s);
    zones.clear();
    host.zones(&query, zones);
    for zone in zones.iter() {
        if !zone.active || zone_misses_player(s, zone, list, class) || !zone.enabled {
            continue;
        }
        s.gravity_mod *= zone.gravity_mod;
        s.drag2 = zone.drag;
        if zone.water {
            let st = host.liquid_statics();
            st.density = zone.density;
            st.viscosity = zone.viscosity;
            st.liquid_height = zone.world_box.max[2];
            st.color = zone.color;
            st.liquid_type = zone.liquid_type;
            // The vertical extent the coverage is measured over.
            let (zb, zt) = class.liquid_extent(s);
            let res = zone_coverage(
                st.coverage,
                zb,
                zt,
                zone.world_box.min[2],
                zone.world_box.max[2],
            );
            st.coverage = res;
            if res <= 0.1 {
                continue;
            }
        }
        // The force merge.
        let a = normalize_safe(s.applied_force);
        let bz = normalize_safe(zone.applied_force);
        let dot = a[0] * bz[0] + a[1] * bz[1] + a[2] * bz[2];
        if !(dot >= 0.98) {
            for i in 0..3 {
                s.applied_force[i] += zone.applied_force[i];
            }
        } else {
            let la = point_len(s.applied_force);
            let lb = point_len(zone.applied_force);
            if lb > la {
                s.applied_force = zone.applied_force;
            }
        }
    }
    let st = *host.liquid_statics();
    let c = st.coverage;
    let cov = if c > 1.0 {
        1.0
    } else if c > 0.0 {
        c
    } else {
        0.0
    };
    s.water_coverage = cov;
    s.liquid_color = st.color;
    host.liquid_statics().coverage = cov;
    s.liquid_type = st.liquid_type;
    s.liquid_height = st.liquid_height;
    if cov < 0.1 {
        return;
    }
    s.drag = db.drag * st.viscosity * cov;
    s.buoyancy = (st.density / db.density) * cov;
}

#[cfg(test)]
#[path = "container_tests.rs"]
mod tests;
