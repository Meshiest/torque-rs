//! TGE's ShapeBase part and the ShapeBase classes: each layout's `(field, width)` list written by hand from the class's
//! `packUpdate`, the bit totals of a default initial update, the pack rules, and round trips in both directions.

use super::*;
use crate::codec::{ClientFacts, ObjCtx};
use crate::player::{NULL_ANIMATION, pack_player};
use crate::projectile::pack_projectile;
use crate::shapebase::{Mount, TgeShapeBase, mask as sb, pack_shapebase};
use crate::shapes::{CAMERA_MOVE_MASK, item_mask, pack_camera, pack_item, pack_static_shape};
use crate::vehicle::{pack_flying_vehicle, pack_hover_vehicle, pack_wheeled_vehicle};

/// Every mask bit: what a new ghost's first update packs with.
pub(super) const ALL: u32 = 0xffff_ffff;

/// The ghost index's width, as a bit count.
pub(super) fn ghost_bits() -> usize {
    usize::from(widths().ghost_bits)
}

/// A ShapeBase object with nothing going on: no script threads, no sound, no image, damage 0 of 1.
pub(super) fn shape_view() -> MapView {
    let mut v = MapView::default().with("max_damage", 0, Value::F32(1.0));
    for i in 0..4 {
        v = v.with("thread_sequence", i, Value::Int(-1));
    }
    v
}

pub(super) fn with_link(mut v: MapView, name: &str, l: Link) -> MapView {
    v.links.insert(name.to_string(), l);
    v
}

fn with_net(mut v: MapView, name: &str, i: usize, s: NetStr) -> MapView {
    v.nets.insert((name.to_string(), i), s);
    v
}

/// The default part's list: the GameBase part (a scale, no datablock), then the part's flag over damage, the empty thread,
/// sound and image groups, the rare group (cloak, null name, shield, invincible, null skin) and the mount's flag.
pub(super) const DEFAULT_PART: &str = "gb.scale?:1 gb.scale:96 gb.datablock?:1 sb.body?:1 sb.damage?:1 sb.damage.level:6 \
     sb.damage.state:2 normal.phi:9 normal.theta:8 sb.threads?:1 sb.sounds?:1 sb.images?:1 sb.rare?:1 sb.cloak?:1 \
     sb.cloaked:1 sb.controlled:1 sb.fading?:1 sb.visible:1 sb.name?:1 ns.present:1 sb.shield?:1 normal.phi:9 \
     normal.theta:8 sb.shield.energy:5 sb.invincible?:1 sb.invincible:64 sb.skin?:1 ns.present:1 sb.mount?:1";

/// The default part's bits: 98 for the GameBase part, 1 + 26 damage + 3 empty groups + 98 rare + 1 mount.
pub(super) const DEFAULT_PART_BITS: usize = 98 + 1 + 26 + 3 + 98 + 1;

/// A client that knows one WheeledVehicleData.
pub(super) struct Wheels(pub usize);

impl ClientFacts for Wheels {
    fn wheel_count(&self, _datablock: u32) -> Option<usize> {
        Some(self.0)
    }
}

/// Writes `rec`, reads it back with `ctx`, checks the record and that writing it again gives the same bits.
pub(super) fn canonical<L: Layout + PartialEq + Debug>(rec: &L, ctx: &ReadCtx) {
    let t = table();
    let mut w = writer();
    encode(rec, &mut w, &t, ctx.widths).unwrap();
    let bits = w.bits();
    let bytes = w.into_bytes();
    let mut r = reader(&bytes, bits);
    let back: L = decode(&mut r, &t, &mut free_charge(), ctx).unwrap();
    assert_eq!(r.bits_left(), 0, "{rec:?}");
    assert_eq!(&back, rec);
    let mut w2 = writer();
    encode(&back, &mut w2, &t, ctx.widths).unwrap();
    assert_eq!(w2.bits(), bits);
    assert_eq!(w2.into_bytes(), bytes);
    // Every prefix is an error.
    for cut in (0..bits).step_by(7) {
        let mut r = reader(&bytes, cut);
        let got: Result<L, _> = decode(&mut r, &t, &mut free_charge(), ctx);
        assert!(got.is_err(), "a prefix of {cut} of {bits} bits decoded");
    }
}

pub(super) fn initial_ctx() -> ReadCtx<'static> {
    ReadCtx::new(widths(), true)
}

pub(super) fn later_ctx() -> ReadCtx<'static> {
    ReadCtx::new(widths(), false)
}

#[test]
fn the_default_shapebase_part_is_227_bits() {
    let (part, ret) = pack_shapebase(&shape_view(), ALL, &pack_ctx());
    assert_eq!(ret, 0);
    check_log(&MarkerPart(part.clone()), DEFAULT_PART);
    assert_eq!(bit_total(&MarkerPart(part.clone()), &table()), 227);
    assert_eq!(DEFAULT_PART_BITS, 227);
    canonical(&MarkerPart(part), &initial_ctx());
}

/// The part alone, as a record.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct MarkerPart(TgeShapeBase);

impl Layout for MarkerPart {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.0.layout(c)
    }
}

#[test]
fn every_group_of_the_shapebase_part_has_its_fields() {
    let g = ghost_bits();
    let db = db_bits();
    let mut view = shape_view()
        .with("thread_sequence", 0, Value::Int(5))
        .with("thread_state", 0, Value::Int(1))
        .with("thread_forward", 0, Value::Bool(true))
        .with("sound_play", 1, Value::Bool(true))
        .with("sound_profile", 1, Value::Id(Some(40)))
        .with("image_datablock", 2, Value::Id(Some(50)))
        .with("image_trigger_down", 2, Value::Bool(true))
        .with("image_fire_count", 2, Value::Int(9))
        .with("image_firing", 2, Value::Bool(true))
        .with("fading", 0, Value::Bool(true))
        .with("fade_out", 0, Value::Bool(true))
        .with("fade_time", 0, Value::F32(2.5))
        .with("mount_node", 0, Value::Int(3));
    view = with_net(view, "image_skin", 2, NetStr::Slot(4));
    view = with_net(view, "shape_name", 0, NetStr::Slot(9));
    view = with_link(view, "mount", Link::Ghost(7));
    let mask = sb::DAMAGE
        | sb::THREAD_N
        | (sb::SOUND_N << 1)
        | (sb::IMAGE_N << 2)
        | sb::CLOAK
        | sb::NAME
        | sb::MOUNTED;
    let (part, ret) = pack_shapebase(&view, mask, &pack_ctx());
    assert_eq!(ret, 0);
    let body = part.body.as_ref().unwrap();
    let image = body.images.as_ref().unwrap()[2].as_ref().unwrap();
    // `writeInt(fireCount, 3)`: 9 goes out as 1; a later update carries no firing flag.
    assert_eq!((image.fire_count, image.firing), (1, None));
    assert_eq!(body.sounds.unwrap()[1], Some(Some(40)));
    assert_eq!(body.mount, Some(Some(Mount { object: 7, node: 3 })));
    check_log(
        &MarkerPart(part.clone()),
        &format!(
            "gb.scale?:1 gb.datablock?:1 sb.body?:1 sb.damage?:1 sb.damage.level:6 sb.damage.state:2 normal.phi:9 \
             normal.theta:8 sb.threads?:1 sb.thread?:1 sb.thread.sequence:6 sb.thread.state:2 sb.thread.forward:1 \
             sb.thread.atEnd:1 sb.thread?:1*3 sb.sounds?:1 sb.sound?:1 sb.sound?:1 sb.sound.play:1 \
             sb.sound.profile:{db} sb.sound?:1*2 sb.images?:1 sb.image?:1*2 sb.image?:1 sb.image.datablock?:1 \
             sb.image.datablock:{db} ns.present:1 ns.mapped:1 ns.slot:5 sb.image.wet:1 sb.image.ammo:1 \
             sb.image.loaded:1 sb.image.target:1 sb.image.triggerDown:1 sb.image.fireCount:3 sb.image?:1 sb.rare?:1 \
             sb.cloak?:1 sb.cloaked:1 sb.controlled:1 sb.fading?:1 sb.fadeOut:1 sb.fadeTime:32 sb.name?:1 \
             ns.present:1 ns.mapped:1 ns.slot:5 sb.shield?:1 sb.invincible?:1 sb.skin?:1 sb.mount?:1 sb.mounted:1 \
             sb.mount.object:{g} sb.mount.node:5"
        ),
    );
    canonical(&MarkerPart(part), &later_ctx());
    // On an initial update the image's firing flag follows its fire count.
    let (part, _) = pack_shapebase(&view, ALL, &pack_ctx());
    let image = part.body.as_ref().unwrap().images.as_ref().unwrap()[2].clone();
    assert_eq!(image.unwrap().firing, Some(true));
    canonical(&MarkerPart(part), &initial_ctx());
}

#[test]
fn the_mount_waits_for_its_objects_ghost_and_a_later_update_unmounts() {
    let view = with_link(shape_view(), "mount", Link::NotGhosted);
    let (part, ret) = pack_shapebase(&view, sb::MOUNTED, &pack_ctx());
    assert_eq!(ret, sb::MOUNTED);
    assert_eq!(part.body.unwrap().mount, None);
    // Not mounted: an initial update writes flag 0, a later one flag 1 and flag 0.
    let (part, _) = pack_shapebase(&shape_view(), sb::MOUNTED, &pack_ctx());
    assert_eq!(part.body.as_ref().unwrap().mount, Some(None));
    // The GameBase part's two flags, the part's flag, the five group flags, flag 1 and flag 0.
    assert_eq!(bit_total(&MarkerPart(part), &table()), 2 + 1 + 5 + 2);
    // Nothing of the part's groups: the part ends after its flag.
    let (part, _) = pack_shapebase(&shape_view(), sb::NO_WARP, &pack_ctx());
    assert_eq!(part.body, None);
    assert_eq!(bit_total(&MarkerPart(part), &table()), 3);
}

#[test]
fn camera_sends_its_pose_unless_this_connection_controls_it() {
    let view = shape_view()
        .floats("position", &[1.0, 2.0, 3.0])
        .floats("rotation", &[0.5, 0.0, 0.25]);
    let cam = pack_camera(&view, ALL, &pack_ctx());
    check_log(
        &cam,
        &format!("{DEFAULT_PART} camera.controlled:1 camera.move?:1 camera.pose:160"),
    );
    assert_eq!(bit_total(&cam, &table()), 227 + 1 + 1 + 160);
    assert_eq!(cam.pose.unwrap()[4].get(), 0.25);
    canonical(&cam, &initial_ctx());
    let mine = view.with("controlled_here", 0, Value::Bool(true));
    // An initial update goes out whole even to the controlling connection.
    assert!(!pack_camera(&mine, ALL, &pack_ctx()).controlled);
    let later = pack_camera(&mine, CAMERA_MOVE_MASK, &pack_ctx());
    assert!(later.controlled && later.pose.is_none());
    assert_eq!(bit_total(&later, &table()), 3 + 1);
    canonical(&later, &later_ctx());
}

#[test]
fn static_shape_ends_with_the_powered_flag() {
    let view = shape_view().with("powered", 0, Value::Bool(true));
    let (s, ret) = pack_static_shape(&view, ALL, &pack_ctx());
    assert_eq!(ret, 0);
    check_log(
        &s,
        &format!(
            "{DEFAULT_PART} static.position?:1 static.transform:193 static.scale:96 static.powered:1"
        ),
    );
    assert_eq!(bit_total(&s, &table()), 227 + 1 + 193 + 96 + 1);
    assert!(s.powered);
    canonical(&s, &initial_ctx());
}

#[test]
fn item_blocks_follow_their_mask_bits() {
    let g = ghost_bits();
    let view = with_link(
        shape_view()
            .with("static", 0, Value::Bool(true))
            .with("collidable", 0, Value::Bool(true))
            .floats("scale", &[1.0, 1.0, 1.0])
            .with("axis_z", 0, Value::F32(-1.0))
            .with("angle", 0, Value::F32(0.75))
            .floats("velocity", &[0.0, 0.0, -2.0]),
        "collision_object",
        Link::Ghost(11),
    );
    let (item, _) = pack_item(&view, ALL, &pack_ctx());
    let initial = item.initial.unwrap();
    // A unit scale is not sent.
    assert!(initial.is_static && initial.collidable && initial.scale.is_none());
    assert_eq!(item.collision_object, Some(11));
    assert_eq!(item.rotation.map(|r| (r.0, r.1.get())), Some((true, 0.75)));
    check_log(
        &item,
        &format!(
            "{DEFAULT_PART} item.initial?:1 item.rotate:1 item.static:1 item.collidable:1 item.scale?:1 \
             item.collision?:1 item.collision:{g} item.rotation?:1 item.negativeZ:1 item.angle:32 item.position?:1 \
             item.position:96 item.atRest:1 item.velocity:96 item.warp:1"
        ),
    );
    canonical(&item, &initial_ctx());
    // The default initial update: no collision object, a zero scale (sent), no rotation flag set, moving.
    let (item, _) = pack_item(&shape_view(), ALL, &pack_ctx());
    assert_eq!(bit_total(&item, &table()), 227 + 101 + 1 + 34 + 195);
    // A rotating item sends no rotation; at rest it sends no velocity; NoWarp clears the warp flag.
    let still =
        shape_view()
            .with("rotate", 0, Value::Bool(true))
            .with("at_rest", 0, Value::Bool(true));
    let (item, _) = pack_item(
        &still,
        item_mask::ROTATION | item_mask::POSITION | sb::NO_WARP,
        &pack_ctx(),
    );
    assert!(item.rotation.is_none());
    let p = item.position.unwrap();
    assert!(p.velocity.is_none() && !p.warp);
    canonical(&item, &later_ctx());
}

#[test]
fn projectile_initial_block_and_its_source() {
    let g = ghost_bits();
    let view = with_link(
        MapView::default()
            .floats("position", &[1.0, 2.0, 3.0])
            .floats("velocity", &[0.0, 20.0, 0.0])
            .with("current_tick", 0, Value::Int(5000))
            .with("source_slot", 0, Value::Int(2))
            .with("hidden", 0, Value::Bool(true)),
        "source_object",
        Link::Ghost(4),
    );
    let (p, ret) = pack_projectile(&view, ALL, &pack_ctx());
    assert_eq!(ret, 0);
    let init = p.initial.unwrap();
    // `ranged(tick, 0, 4095)`: 5000 wraps into the field.
    assert_eq!(init.tick, 5000 & 0xfff);
    assert_eq!(init.velocity.unwrap().speed, 640);
    check_log(
        &p,
        &format!(
            "gb.scale?:1 gb.scale:96 gb.datablock?:1 proj.initial?:1 cp.form:2 cp.sign:1 cp.magnitude:15 cp.sign:1 \
             cp.magnitude:15 cp.sign:1 cp.magnitude:15 proj.velocity?:1 vel.phi:11 vel.theta:10 vel.speed:13 \
             proj.tick:12 proj.source?:1 proj.source:{} proj.slot:2 proj.explosion?:1 proj.explosion.position:96 \
             proj.explosion.normal:96 proj.explosion.hitType:32 proj.bounce?:1 proj.bounce.position:96 \
             proj.bounce.velocity:96",
            g + 1
        ),
    );
    canonical(&p, &initial_ctx());
    // A source not ghosted yet keeps the initial bit.
    let waiting = with_link(MapView::default(), "source_object", Link::NotGhosted);
    let (p, ret) = pack_projectile(&waiting, ALL, &pack_ctx());
    assert_eq!(ret, crate::parts::mask::INITIAL);
    assert!(p.initial.unwrap().source.is_none());
    // The default initial update: the GameBase part, 65 bits of initial block, no explosion, the bounce block.
    let (p, _) = pack_projectile(&MapView::default(), ALL, &pack_ctx());
    assert_eq!(bit_total(&p, &table()), 98 + 65 + 1 + 193);
}

#[test]
fn player_layout_and_its_control_test() {
    let view = shape_view()
        .with("action", 0, Value::Int(20))
        .with("action_position", 0, Value::F32(0.5))
        .with("arm_action", 0, Value::Int(3))
        .with("state", 0, Value::Int(2))
        .with("recover_ticks", 0, Value::Int(40))
        .floats("velocity", &[3.0, 0.0, 0.0])
        .floats("rotation", &[0.0, 0.0, 1.0])
        .floats("head", &[0.2, 0.0, -0.2])
        .with("max_look_angle", 0, Value::F32(1.0))
        .with("move_angles", 0, Value::Int(0x1234))
        .with("move_axes", 1, Value::Int(32))
        .with("move_triggers", 5, Value::Bool(true))
        .with("energy", 0, Value::F32(30.0))
        .with("max_energy", 0, Value::F32(60.0));
    let (p, _) = pack_player(&view, ALL, &pack_ctx());
    let a = p.action.unwrap();
    assert_eq!((a.action, a.position.is_some()), (20, true));
    assert_eq!(p.arm, Some(3));
    let m = p.motion.unwrap();
    assert_eq!(m.recover_ticks, Some(40));
    assert_eq!(m.mv.angles[0], Some(0x1234));
    assert!(m.mv.triggers[5]);
    // energy 30 / 60 in 5 bits: (S32)(0.5 * 31) = 15.
    assert_eq!(p.energy, 15);
    check_log(
        &p,
        &format!(
            "{DEFAULT_PART} player.impact?:1 player.action?:1 player.action:9 player.holdAtEnd:1 player.atEnd:1 \
             player.firstPerson:1 player.actionPosition?:1 player.actionPosition:6 player.arm?:1 player.arm:9 \
             player.controlled:1 player.move?:1 player.falling:1 player.state:3 player.recover?:1 player.recover:7 \
             cp.form:2 cp.sign:1 cp.magnitude:15 cp.sign:1 cp.magnitude:15 cp.sign:1 cp.magnitude:15 \
             player.velocity?:1 vel.phi:11 vel.theta:10 vel.speed:13 player.yaw:7 player.headX:6 player.headZ:6 \
             move.angle?:1 move.angle:16 move.angle?:1 move.angle?:1 move.axis:6*3 move.freeLook:1 move.trigger:1*6 \
             player.warp:1 player.energy:5"
        ),
    );
    canonical(&p, &initial_ctx());
    // The table actions and a null arm action are not sent; the impact goes out on a later update only.
    let quiet = shape_view()
        .with("action", 0, Value::Int(7))
        .with("arm_action", 0, Value::Int(i64::from(NULL_ANIMATION)))
        .with("impact_sound", 0, Value::Int(5));
    let (p, _) = pack_player(&quiet, ALL, &pack_ctx());
    assert!(p.action.is_none() && p.arm.is_none() && p.impact.is_none());
    let (p, _) = pack_player(&quiet, crate::player::mask::IMPACT, &pack_ctx());
    assert_eq!(p.impact, Some(5));
    // The controlling connection gets nothing after the flag, not even the energy.
    let mine = quiet.with("controlled_here", 0, Value::Bool(true));
    let (p, _) = pack_player(&mine, crate::player::mask::MOVE, &pack_ctx());
    assert!(p.controlled && p.motion.is_none());
    assert_eq!(bit_total(&p, &table()), 3 + 3 + 1);
    canonical(&p, &later_ctx());
    // The default initial update: the part, the three animation flags and the control flag, a 105-bit move block, the energy.
    let (p, _) = pack_player(&shape_view(), ALL, &pack_ctx());
    assert_eq!(bit_total(&p, &table()), 227 + 4 + 105 + 5);
}

/// A vehicle with four wheels that all have a tire and a spring.
fn wheeled_view() -> MapView {
    let mut v = shape_view()
        .with("wheel_count", 0, Value::Int(4))
        .with("max_steering_angle", 0, Value::F32(0.5))
        .floats("steering", &[0.25, -0.5])
        .floats("rotation", &[0.0, 0.0, 0.0, 1.0])
        .with("passenger_here", 0, Value::Bool(true))
        .with("energy_value", 0, Value::F32(2.0));
    for i in 0..4 {
        v = v
            .with("wheel_tire", i, Value::Id(Some(60)))
            .with("wheel_spring", i, Value::Id(Some(61)))
            .with("wheel_avel", i, Value::F32(i as f32));
    }
    v
}

#[test]
fn the_vehicle_part_and_wheeled_vehicle() {
    let db = db_bits();
    let (v, _) = pack_wheeled_vehicle(&wheeled_view(), ALL, &pack_ctx());
    let s = v.vehicle.state.unwrap();
    // (0.25 + 0.5) / 1.0 and (-0.5 + 0.5) / 1.0 in 9 bits; the energy is clamped to 1.
    assert_eq!(s.steering, [383, 0]);
    assert_eq!(s.energy, Some(255));
    assert_eq!(v.setup.as_ref().unwrap().len(), 4);
    check_log(
        &v,
        &format!(
            "{DEFAULT_PART} vehicle.jetting:1 vehicle.controlled:1 vehicle.yaw:9 vehicle.pitch:9 move.angle?:1*3 \
             move.axis:6*3 move.freeLook:1 move.trigger:1*6 vehicle.position?:1 cp.form:2 cp.sign:1 cp.magnitude:15 \
             cp.sign:1 cp.magnitude:15 cp.sign:1 cp.magnitude:15 vehicle.rotation:128 vehicle.linearMomentum:96 \
             vehicle.angularMomentum:96 vehicle.atRest:1 vehicle.energy?:1 vehicle.energy:8 wheeled.wheels?:1 \
             {} wheeled.controlled:1 wheeled.braking:1 wheeled.position?:1 wheeled.spin:96*4",
            format!(
                "wheeled.wheel?:1 wheeled.tire:{db} wheeled.spring:{db} wheeled.powered:1 wheeled.steering:32 "
            )
            .repeat(4)
            .trim_end()
        ),
    );
    let facts = Wheels(4);
    let object = ObjCtx {
        controlled_here: false,
        datablock: Some(100),
    };
    let ctx = ReadCtx {
        facts: Some(&facts),
        object,
        ..initial_ctx()
    };
    canonical(&v, &ctx);
    // A reader that does not know the datablock cannot read the wheels.
    let t = table();
    let mut w = writer();
    encode(&v, &mut w, &t, widths()).unwrap();
    let n = w.bits();
    let bytes = w.into_bytes();
    let got: Result<crate::vehicle::WheeledVehicle, _> = decode(
        &mut reader(&bytes, n),
        &t,
        &mut free_charge(),
        &initial_ctx(),
    );
    assert!(got.is_err());
    // More wheels than a WheeledVehicle can have is refused.
    let nine = Wheels(9);
    let ctx = ReadCtx {
        facts: Some(&nine),
        object,
        ..initial_ctx()
    };
    let got: Result<crate::vehicle::WheeledVehicle, _> =
        decode(&mut reader(&bytes, n), &t, &mut free_charge(), &ctx);
    assert!(got.is_err());
    // The default initial update of a vehicle with no wheels: the part (648 bits) and four flags.
    let (v, _) = pack_wheeled_vehicle(&shape_view(), ALL, &pack_ctx());
    assert_eq!(bit_total(&v, &table()), 648 + 4);
}

#[test]
fn flying_vehicle_fields_follow_the_readers_control_object_and_hover_always_sends_its_thrust() {
    let view = shape_view()
        .with("create_height_on", 0, Value::Bool(true))
        .with("thrust_direction", 0, Value::Int(2));
    let (f, _) = pack_flying_vehicle(&view, ALL, &pack_ctx());
    assert_eq!(
        f.tail.map(|t| (t.create_height_on, t.thrust)),
        Some((true, 2))
    );
    assert_eq!(bit_total(&f, &table()), 648 + 1 + 3);
    canonical(&f, &initial_ctx());
    // The controlling connection's later update: the Vehicle part ends at its flag and the two fields are not written; its
    // reader, whose control object this is, does not read them.
    let mine = view.with("controlled_here", 0, Value::Bool(true));
    let (f, _) = pack_flying_vehicle(&mine, CAMERA_MOVE_MASK, &pack_ctx());
    assert!(f.vehicle.controlled && f.tail.is_none());
    assert_eq!(bit_total(&f, &table()), 3 + 1 + 1);
    let ctx = ReadCtx {
        object: ObjCtx {
            controlled_here: true,
            datablock: None,
        },
        ..later_ctx()
    };
    canonical(&f, &ctx);
    let (h, _) = pack_hover_vehicle(
        &shape_view().with("thrust_direction", 0, Value::Int(2)),
        ALL,
        &pack_ctx(),
    );
    assert_eq!((h.thrust, bit_total(&h, &table())), (2, 648 + 3));
    canonical(&h, &initial_ctx());
}

/// A ShapeBase dissolving away, 500 ms into a 2 s dissolve at Sim time 1500.
fn dissolving_view() -> MapView {
    shape_view()
        .with("dissolve", 0, Value::Bool(true))
        .with("dissolve_serial", 0, Value::Int(259))
        .with("dissolve_out", 0, Value::Bool(true))
        .with("dissolve_ms", 0, Value::F32(2000.0))
        .with("dissolve_start", 0, Value::Int(1000))
        .with("dissolve_color", 0, Value::F32(1.0))
        .with("dissolve_color", 1, Value::F32(0.5))
        .with("dissolve_glow", 0, Value::F32(6.0))
        .with("dissolve_edge", 0, Value::F32(0.04))
        .with("dissolve_emitter", 0, Value::Id(Some(60)))
        .with("dissolve_tint", 0, Value::Bool(true))
        .with("dissolve_cell", 0, Value::F32(0.25))
        .with("dissolve_sweep", 0, Value::F32(-0.5))
        .with("dissolve_density", 0, Value::F32(64.0))
}

fn cloak_of(part: &TgeShapeBase) -> crate::shapebase::Cloak {
    part.body
        .as_ref()
        .and_then(|b| b.rare.as_ref())
        .and_then(|r| r.cloak)
        .expect("the cloak group")
}

#[test]
fn a_dissolve_rides_the_cloak_group_on_the_engines_protocol_alone() {
    let ext = widths().with_extensions(true);
    let at = |widths| PackCtx {
        widths,
        now_ms: 1500,
        ..PackCtx::new(widths)
    };
    let (part, _) = pack_shapebase(&dissolving_view(), sb::CLOAK, &at(ext));
    let d = cloak_of(&part).dissolve.expect("the dissolve");
    assert_eq!(
        (d.serial, d.out, d.duration_ms, d.elapsed_ms),
        (3, true, 2000, 500)
    );
    assert_eq!((d.edge, d.glow, d.edge_share), ([255, 128, 0], 48, 10));
    assert_eq!(
        (d.emitter, d.tint, d.cell, d.sweep, d.density),
        (Some(60), true, 16, 64, 64)
    );
    // The engine's protocol: written and read back as it was, at every prefix an error.
    canonical(&MarkerPart(part.clone()), &ReadCtx::new(ext, false));
    // Torque's protocol: packed without it, and its bits are a ShapeBase's that runs none.
    let (plain, _) = pack_shapebase(&dissolving_view(), sb::CLOAK, &at(widths()));
    assert_eq!(cloak_of(&plain).dissolve, None);
    let (none, _) = pack_shapebase(&shape_view(), sb::CLOAK, &at(widths()));
    assert_eq!(plain, none);
    let bytes = |rec: &MarkerPart, w: NetWidths| {
        let mut out = writer();
        encode(rec, &mut out, &table(), w).unwrap();
        (out.bits(), out.into_bytes())
    };
    // A record holding one is written for Torque's protocol without it, bit for bit.
    assert_eq!(
        bytes(&MarkerPart(part.clone()), widths()),
        bytes(&MarkerPart(none.clone()), widths())
    );
    // The engine's protocol gives a ShapeBase without one a flag more.
    let (engine_none, _) = pack_shapebase(&shape_view(), sb::CLOAK, &at(ext));
    assert_eq!(
        bytes(&MarkerPart(engine_none), ext).0,
        bytes(&MarkerPart(none), widths()).0 + 1
    );
}

#[test]
fn a_hostile_dissolve_is_held_to_its_ranges_as_it_is_read() {
    let ext = widths().with_extensions(true);
    let ctx = PackCtx {
        now_ms: 1500,
        ..PackCtx::new(ext)
    };
    let (mut part, _) = pack_shapebase(&dissolving_view(), sb::CLOAK, &ctx);
    // An elapsed time past the dissolve and a sweep past its last code, as a server could write them.
    if let Some(c) = part
        .body
        .as_mut()
        .and_then(|b| b.rare.as_mut())
        .and_then(|r| r.cloak.as_mut())
    {
        c.dissolve = c.dissolve.map(|d| crate::shapebase::DissolveField {
            elapsed_ms: 900_000,
            sweep: 255,
            ..d
        });
    }
    let mut w = writer();
    encode(&MarkerPart(part), &mut w, &table(), ext).unwrap();
    let bits = w.bits();
    let bytes = w.into_bytes();
    let mut r = reader(&bytes, bits);
    let back: MarkerPart = decode(
        &mut r,
        &table(),
        &mut free_charge(),
        &ReadCtx::new(ext, false),
    )
    .unwrap();
    let d = cloak_of(&back.0).dissolve.expect("the dissolve");
    assert_eq!((d.elapsed_ms, d.sweep), (d.duration_ms, 254));
}
