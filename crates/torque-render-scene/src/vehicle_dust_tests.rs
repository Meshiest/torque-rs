use ghost_world::tge::TgeClientSet;
use ghost_world::world::{ClientWorld, VehicleLook, WheelLook};
use ghosts::Value;

use super::*;
use crate::world::tests::datablock;

/// A datablock as the server packs it from script values `fields` (name, element, value).
fn sent(
    record: impl Fn(&dyn ghosts::ObjectView) -> ghosts::datablocks::TgeDatablockRecord,
    fields: &[(&str, usize, Value)],
) -> ghost_world::tge::TgeDatablock {
    struct View<'a>(&'a [(&'a str, usize, Value)]);
    impl ghosts::ObjectView for View<'_> {
        fn get(&self, field: &str, index: usize) -> Value {
            self.0
                .iter()
                .find(|(n, i, _)| *n == field && *i == index)
                .map_or(Value::Null, |f| f.2.clone())
        }
    }
    let record = record(&View(fields));
    ghost_world::tge::TgeDatablock {
        class: record.class().name().to_string(),
        fields: ghosts::datablocks::fields::fields(&record, ghosts::codec::NetWidths::new(10, 10)),
    }
}

/// A world with a dust emitter (11, of particles 10, one every 10 ms) and a WheeledVehicleData (30) whose tires throw it.
fn world() -> ClientWorld<TgeClientSet> {
    use ghosts::datablocks::TgeDatablockRecord as R;
    use ghosts::datablocks::effects::{pack_particle_data, pack_particle_emitter_data};
    let mut w = ClientWorld::<TgeClientSet>::default();
    w.datablocks.insert(
        10,
        sent(
            |v| R::ParticleData(pack_particle_data(v)),
            &[("lifetimeMS", 0, Value::Int(800))],
        ),
    );
    w.datablocks.insert(
        11,
        sent(
            |v| R::ParticleEmitterData(pack_particle_emitter_data(v)),
            &[
                ("ejectionPeriodMS", 0, Value::Int(10)),
                ("dataBlockIds", 0, Value::Int(10)),
            ],
        ),
    );
    w.datablocks.insert(
        30,
        datablock(
            "WheeledVehicleData",
            &[
                ("tireEmitter", Value::Id(Some(11))),
                ("maxWheelSpeed", Value::F32(20.0)),
            ],
        ),
    );
    w
}

fn look(speed: f32, contact: bool, slip: f32) -> VehicleLook {
    VehicleLook {
        steering: 0.0,
        velocity: [0.0, speed, 0.0],
        wheels: vec![
            WheelLook {
                fitted: true,
                contact,
                point: [1.0, 0.0, 0.0],
                slip,
                ..WheelLook::default()
            };
            4
        ],
    }
}

/// Runs `frames` frames of 32 ms of a vehicle of `look` moving along y.
fn run(dust: &mut VehicleDust, w: &ClientWorld<TgeClientSet>, look: &VehicleLook, frames: u32) {
    let none = |_: [f32; 3], _: f32| None;
    for _ in 0..frames {
        let v = [DustVehicle {
            index: 1,
            datablock: 30,
            position: [0.0; 3],
            look,
        }];
        dust.update(w, &v, &none, 0.032);
    }
}

#[test]
fn tires_throw_dust_only_on_the_ground_and_moving_by_their_slip() {
    let w = world();
    let mut dust = VehicleDust::default();
    // Still: no dust.
    run(&mut dust, &w, &look(0.5, true, 1.0), 20);
    assert_eq!(dust.particles(), 0);
    // Off the ground: none.
    run(&mut dust, &w, &look(15.0, false, 1.0), 20);
    assert_eq!(dust.particles(), 0);
    // Rolling without slip: none under TGE's rule.
    run(&mut dust, &w, &look(15.0, true, 0.0), 20);
    assert_eq!(dust.particles(), 0);
    // Sliding: a cloud from every wheel, more the more it slips.
    run(&mut dust, &w, &look(15.0, true, 1.0), 10);
    let sliding = dust.particles();
    assert!(sliding > 40, "{sliding}");
    let mut light = VehicleDust::default();
    run(&mut light, &w, &look(15.0, true, 0.2), 10);
    assert!(
        light.particles() > 0 && light.particles() < sliding / 2,
        "{}",
        light.particles()
    );
    // A game's floor under the slip: a rolling wheel throws a little.
    let mut rolling = VehicleDust::new(DustRules { min_slip: 0.3 });
    run(&mut rolling, &w, &look(15.0, true, 0.0), 10);
    assert!(rolling.particles() > 0);
    // The dust settles and its emitters go once the vehicle is no longer run.
    let none = |_: [f32; 3], _: f32| None;
    for _ in 0..60 {
        dust.update(&w, &[], &none, 0.032);
    }
    assert_eq!(dust.particles(), 0);
}
