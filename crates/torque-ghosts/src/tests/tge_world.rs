//! TGE's world classes outside ShapeBase (ParticleEmitterNode, fxLight, PhysicalZone, Precipitation, Sky, Sun) and the marker
//! bodies over TGE's ShapeBase part: `(field, width)` lists, default initial updates, the pack rules, the caps and round trips.

use super::tge_shapes::{
    ALL, canonical, ghost_bits, initial_ctx, later_ctx, shape_view, with_link,
};
use super::*;
use crate::fx::{FX_LIGHT_ATTACH_MASK, pack_fx_light, pack_particle_emitter_node};
use crate::shapebase::TgeShapeBase;
use crate::sky::{Sky, mask as sky_mask, pack_sky, pack_sun};
use crate::weather::{pack_precipitation, precipitation_mask};
use crate::zone::{PhysicalZone, ZONE_ACTIVE_MASK, pack_physical_zone};

/// The bits of `strings` written one after another into one packet stream (each one's coding depends on the one before).
fn string_bits(strings: &[&[u8]]) -> usize {
    let t = table();
    let mut w = writer();
    for s in strings {
        torque_net::strcoder::write_string(&mut w, &t, s, 255);
    }
    w.bits()
}

#[test]
fn particle_emitter_node_sends_its_matrix_every_update() {
    let db = db_bits();
    let view = MapView::default().floats("scale", &[1.0, 1.0, 1.0]).with(
        "emitter",
        0,
        Value::Id(Some(77)),
    );
    let n = pack_particle_emitter_node(&view, ALL, &pack_ctx());
    assert_eq!(n.emitter, Some(77));
    check_log(
        &n,
        &format!(
            "gb.scale?:1 gb.scale:96 gb.datablock?:1 node.matrix:512 node.scale:96 node.emitter?:1 node.emitter:{db}"
        ),
    );
    canonical(&n, &initial_ctx());
    let n = pack_particle_emitter_node(&MapView::default(), ALL, &pack_ctx());
    assert_eq!(bit_total(&n, &table()), 98 + 512 + 96 + 1);
    // A later update with no bit set still carries the matrix, the scale and the emitter flag.
    let n = pack_particle_emitter_node(&MapView::default(), 0, &pack_ctx());
    assert_eq!(bit_total(&n, &table()), 2 + 512 + 96 + 1);
}

#[test]
fn fx_light_attach_and_config_blocks() {
    let g = ghost_bits();
    let view = with_link(
        MapView::default()
            .with("attach_valid", 0, Value::Bool(true))
            .with("enable", 0, Value::Bool(true))
            .with("icon_size", 0, Value::F32(0.5)),
        "attached",
        Link::Ghost(9),
    );
    let l = pack_fx_light(&view, ALL, &pack_ctx());
    assert_eq!(l.attach, Some(Some(9)));
    check_log(
        &l,
        &format!(
            "gb.scale?:1 gb.scale:96 gb.datablock?:1 fxlight.attach?:1 fxlight.attached:1 fxlight.object:{} \
             fxlight.config?:1 fxlight.transform:193 fxlight.enable:8 fxlight.iconSize:32",
            g + 1
        ),
    );
    canonical(&l, &initial_ctx());
    // Not ghosted yet: `U32(-1)` cut to the field.
    let waiting = with_link(
        MapView::default().with("attach_valid", 0, Value::Bool(true)),
        "attached",
        Link::NotGhosted,
    );
    let l = pack_fx_light(&waiting, FX_LIGHT_ATTACH_MASK, &pack_ctx());
    assert_eq!(l.attach, Some(Some((1 << (g + 1)) - 1)));
    // The config bit is the scale bit: a config change sends the scale too.
    let l = pack_fx_light(&MapView::default(), ALL, &pack_ctx());
    assert!(l.game.scale.is_some() && l.attach.is_none());
    assert_eq!(bit_total(&l, &table()), 98 + 1 + 1 + 193 + 8 + 32);
}

fn box_zone() -> MapView {
    let mut v = MapView::default()
        .with("point_count", 0, Value::Int(8))
        .with("plane_count", 0, Value::Int(6))
        .with("edge_count", 0, Value::Int(12))
        .with("velocity_mod", 0, Value::F32(1.0))
        .with("gravity_mod", 0, Value::F32(0.5))
        .with("active", 0, Value::Bool(true));
    for i in 0..24 {
        v = v.with("point", i, Value::F32(i as f32));
    }
    for i in 0..48 {
        v = v.with("edge", i, Value::Int(i as i64 % 8));
    }
    v
}

#[test]
fn physical_zone_sends_its_polyhedron_on_the_initial_update() {
    let z = pack_physical_zone(&box_zone(), ALL);
    let init = z.initial.as_ref().unwrap();
    assert_eq!(
        (init.points.len(), init.planes.len(), init.edges.len()),
        (8, 6, 12)
    );
    assert_eq!(init.points[7][2].get(), 23.0);
    check_log(
        &z,
        "zone.initial?:1 zone.matrix:512 zone.scale:96 zone.points:32 zone.point:96*8 zone.planes:32 \
         zone.plane:128*6 zone.edges:32 zone.edge:32*48 zone.velocityMod:32 zone.gravityMod:32 \
         zone.appliedForce:96 zone.active:1",
    );
    canonical(&z, &initial_ctx());
    let z = pack_physical_zone(&MapView::default(), ALL);
    assert_eq!(bit_total(&z, &table()), 1 + 512 + 96 + 3 * 32 + 64 + 96 + 1);
    // A later update: the flag and the active flag.
    let z = pack_physical_zone(&box_zone(), ZONE_ACTIVE_MASK);
    assert_eq!(bit_total(&z, &table()), 2);
    assert!(z.active);
}

#[test]
fn physical_zone_counts_are_capped() {
    let t = table();
    for count in [u32::MAX, 1025, 3] {
        let mut w = writer();
        w.write_flag(true);
        for _ in 0..19 {
            w.write_u32(0);
        }
        w.write_u32(count);
        // Room for 1,025 points: the cap, not the bits left, refuses the second count; the third has too few bits.
        let room = if count == 3 { 2 } else { 1025 };
        for _ in 0..room * 3 {
            w.write_u32(0);
        }
        let n = w.bits();
        let bytes = w.into_bytes();
        let got: Result<PhysicalZone, _> = decode(
            &mut reader(&bytes, n),
            &t,
            &mut free_charge(),
            &initial_ctx(),
        );
        assert!(got.is_err(), "count {count}");
    }
}

#[test]
fn precipitation_sends_the_storm_only_alone() {
    use precipitation_mask::{DATA, PERCENTAGE, STORM};
    let view = MapView::default()
        .with("drops", 0, Value::Int(1024))
        .with("percentage", 0, Value::F32(0.5))
        .with("storm_end_percentage", 0, Value::F32(1.0))
        .with("storm_total_time", 0, Value::Int(3000));
    let p = pack_precipitation(&view, DATA | PERCENTAGE | STORM, &pack_ctx());
    assert_eq!(p.storm.map(|s| s.total_time), Some(3000));
    check_log(
        &p,
        "gb.scale?:1 gb.datablock?:1 precip.data?:1 precip.drops:32 precip.minSpeed:32 precip.maxSpeed:32 \
         precip.boxWidth:32 precip.boxHeight:32 precip.minMass:32 precip.maxMass:32 precip.maxTurbulence:32 \
         precip.turbulenceSpeed:32 precip.useTurbulence:1 precip.rotateWithCamVel:1 precip.doCollision:1 \
         precip.percentage?:1 precip.percentage:32 precip.storm?:1 precip.storm.endPercentage:32 \
         precip.storm.totalTime:32",
    );
    canonical(&p, &later_ctx());
    // Any other bit in the mask holds the storm back (an initial update never sends it).
    let p = pack_precipitation(&view, STORM | 1, &pack_ctx());
    assert!(p.storm.is_none());
    let p = pack_precipitation(&MapView::default(), ALL, &pack_ctx());
    assert_eq!(
        bit_total(&p, &table()),
        98 + (1 + 32 + 256 + 3) + (1 + 32) + 1
    );
}

fn sky_view() -> MapView {
    MapView::default()
        .with("material_list", 0, Value::str("sky.dml"))
        .with("fog_volume_count", 0, Value::Int(2))
        .with("fog_visible_distance", 1, Value::F32(300.0))
        .with("fog_volume_color", 5, Value::F32(0.25))
        .with("storm_fog_active", 1, Value::Bool(true))
        .with("cloud_text", 0, Value::str("cloud1"))
        .with("cloud_text", 2, Value::str("cloud3"))
        .with("storm_fog_start_ms", 0, Value::Int(3000))
        .with("storm_fog_time", 0, Value::F32(10.0))
        .with("fog_percentage", 0, Value::F32(0.5))
        .with("visible_distance", 0, Value::F32(500.0))
}

#[test]
fn sky_init_block_and_the_storm_under_way() {
    let ctx = PackCtx {
        now_ms: 5000,
        ..pack_ctx()
    };
    let s = pack_sky(&sky_view(), ALL, &ctx);
    let init = s.init.as_ref().unwrap();
    assert_eq!(init.fog_volumes.len(), 2);
    assert_eq!(init.fog_volumes[1].storm_active, 1);
    assert_eq!(init.fog_volumes[1].color[2].get(), 0.25);
    // 2,000 ms of a 10 s storm: the storm block goes out.
    assert_eq!(init.storm.map(|p| p.elapsed), Some(2000));
    // On an initial update the storm fog flags are held back.
    assert!(s.storm_fog_on.is_none() && s.storm_fog.is_none());
    let volume = "sky.fog.visibleDistance:32 sky.fog.minHeight:32 sky.fog.maxHeight:32 sky.fog.color:96 \
                  sky.fog.stormActive:8 ";
    check_log(
        &s,
        &format!(
            "sky.init?:1 sky.materialList:s sky.fogColor:96 sky.fogVolumeCount:32 sky.texturesOn:8 \
             sky.renderBoxBottom:8 sky.solidFillColor:96 sky.effectPrecip:8 sky.noRenderBans:1 {} {} sky.wind:96 \
             sky.fogVolume:32 sky.storm?:1 sky.storm.fogPercentage:32 sky.storm.time:32 sky.storm.state:32 \
             sky.storm.elapsed:32 sky.storm.endPercentage:32 sky.stormCloudsOn?:1 sky.stormCloudsOn:8 \
             sky.stormFogOn?:1 sky.visibility?:1 sky.visibility:64 sky.stormCloud?:1 sky.stormCloud.state:32 \
             sky.stormCloud.time:32 sky.stormFog?:1 sky.realFog?:1 sky.realFog:32 sky.realFog.maxMinSpeed:96 \
             sky.wind?:1 sky.wind:96",
            volume.repeat(2).trim_end(),
            "sky.cloud.texture:s sky.cloud.height:32 sky.cloud.speed:32 "
                .repeat(3)
                .trim_end()
        ),
    );
    canonical(&s, &initial_ctx());
    // The default initial update: 938 bits besides the four empty strings; no storm under way.
    let s = pack_sky(&MapView::default(), ALL, &pack_ctx());
    assert!(s.init.as_ref().unwrap().storm.is_none());
    assert_eq!(
        bit_total(&s, &table()),
        938 + string_bits(&[b"", b"", b"", b""])
    );
    // A later storm fog update.
    let s = pack_sky(
        &sky_view(),
        sky_mask::STORM_FOG | sky_mask::STORM_FOG_ON,
        &ctx,
    );
    assert!(s.storm_fog.is_some() && s.storm_fog_on.is_some());
    canonical(&s, &later_ctx());
}

#[test]
fn sky_refuses_more_fog_volumes_than_it_holds() {
    let t = table();
    for (count, ok) in [(3u32, true), (4, false), (u32::MAX, false)] {
        let mut w = writer();
        w.write_flag(true);
        torque_net::strcoder::write_string(&mut w, &t, b"", 255);
        for _ in 0..3 {
            w.write_u32(0);
        }
        w.write_u32(count);
        for _ in 0..80 {
            w.write_u32(0);
        }
        let n = w.bits();
        let bytes = w.into_bytes();
        let got: Result<Sky, _> = decode(
            &mut reader(&bytes, n),
            &t,
            &mut free_charge(),
            &initial_ctx(),
        );
        assert_eq!(got.is_ok(), ok, "count {count}");
    }
}

#[test]
fn sun_sends_direction_colour_and_ambient() {
    let view = MapView::default()
        .floats("direction", &[0.0, 0.0, -1.0])
        .floats("color", &[1.0, 0.9, 0.8, 1.0])
        .floats("ambient", &[0.3, 0.3, 0.3, 1.0]);
    let s = pack_sun(&view, ALL);
    check_log(
        &s,
        "sun.update?:1 sun.direction:96 sun.color:128 sun.ambient:128",
    );
    assert_eq!(bit_total(&s, &table()), 353);
    canonical(&s, &initial_ctx());
    assert_eq!(bit_total(&pack_sun(&view, 0), &table()), 1);
}

#[test]
fn the_marker_bodies_over_tges_shapebase_part() {
    let view = shape_view()
        .with("name", 0, Value::str("spot"))
        .with("team", 0, Value::Int(2));
    let (m, _) = crate::markers::pack_mission_marker::<TgeShapeBase>(&view, ALL, &pack_ctx());
    assert_eq!(bit_total(&m, &table()), 227 + 1 + 193 + 96);
    canonical(&m, &initial_ctx());
    let (w, _) = crate::markers::pack_waypoint::<TgeShapeBase>(&view, ALL, &pack_ctx());
    assert_eq!(
        bit_total(&w, &table()),
        227 + 1 + 193 + 96 + 1 + string_bits(&[b"spot"]) + 33 + 2
    );
    canonical(&w, &initial_ctx());
    let (s, _) = crate::markers::pack_spawn_sphere::<TgeShapeBase>(&view, ALL, &pack_ctx());
    assert_eq!(bit_total(&s, &table()), 227 + 1 + 193 + 96 + 1 + 128);
    canonical(&s, &initial_ctx());
}
