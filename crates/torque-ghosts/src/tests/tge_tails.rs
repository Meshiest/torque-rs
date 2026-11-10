//! TGE's control tails: each packet data's `(field, width)` list from its `writePacketData`, the bit totals, the write and
//! class-directed read through `torque_net::control`, the nested Player data, the compression point they set and the caps.

use torque_net::control::{CameraPacketData, ControlData, ControlReader, checksum};

use super::tge_shapes::{canonical, ghost_bits, later_ctx, with_link};
use super::*;
use crate::tails::{
    MAX_NESTED_TAILS, PlayerTail, ShapeTail, TailClass, TailData, TailFrame, TgePacketData,
    TgeTails, VehicleTail, WheeledTail,
};

const SHAPE: &str = "tail.energy:32 tail.rechargeRate:32";
const VEHICLE: &str = "tail.energy:32 tail.rechargeRate:32 tail.steering:64 tail.position:96 tail.rotation:128 \
     tail.linearMomentum:96 tail.angularMomentum:96 tail.atRest:1 tail.noContacts:1 tail.disableMove:1";

fn player_view() -> MapView {
    with_link(
        MapView::default()
            .with("energy", 0, Value::F32(50.0))
            .with("state", 0, Value::Int(2))
            .with("recover_ticks", 0, Value::Int(12))
            .with("jump_delay", 0, Value::Int(3))
            .floats("position", &[10.0, 20.0, 30.0])
            .with("jump_surface_last_contact", 0, Value::Int(99))
            .floats("head", &[0.1, 0.0, 0.2])
            .floats("rotation", &[0.0, 0.0, 1.5]),
        "control_object",
        Link::Ghost(5),
    )
}

fn wheeled_view() -> MapView {
    let mut v = MapView::default()
        .floats("position", &[1.0, 2.0, 3.0])
        .floats("rotation", &[0.0, 0.0, 0.0, 1.0])
        .with("contact_count", 0, Value::Int(2))
        .with("braking", 0, Value::Bool(true))
        .with("wheel_count", 0, Value::Int(4));
    for i in 0..4 {
        v = v.with("wheel_avel", i, Value::F32(i as f32)).with(
            "wheel_slipping",
            i,
            Value::Bool(i == 3),
        );
    }
    v
}

#[test]
fn the_tails_have_their_fields() {
    let g = ghost_bits();
    let s = ShapeTail::pack(&MapView::default());
    check_log(&s, SHAPE);
    assert_eq!(bit_total(&s, &table()), 64);
    canonical(&s, &later_ctx());
    let p = PlayerTail::pack(&player_view(), widths().ghost_bits);
    assert_eq!(p.recover_ticks, Some(12));
    // The ticks since the last contact are capped at 15.
    assert_eq!(p.motion.unwrap().last_contact, 15);
    assert_eq!(p.control, Some(5));
    check_log(
        &p,
        &format!(
            "{SHAPE} tail.state:3 tail.recover?:1 tail.recover:7 tail.jumpDelay?:1 tail.jumpDelay:7 tail.unmounted?:1 \
             tail.position:96 tail.velocity:96 tail.lastContact:4 tail.headX:32 tail.headZ:32 tail.rotationZ:32 \
             tail.control?:1 tail.control:{g}"
        ),
    );
    canonical(&p, &later_ctx());
    // The default Player: unmounted, no recover, no jump delay, no control object.
    let p = PlayerTail::pack(&MapView::default(), widths().ghost_bits);
    assert_eq!(
        bit_total(&p, &table()),
        64 + 3 + 1 + 1 + (1 + 96 + 96 + 4) + 96 + 1
    );
    // A mounted Player sends no position block.
    let mounted = MapView::default().with("mounted", 0, Value::Bool(true));
    assert!(PlayerTail::pack(&mounted, 12).motion.is_none());
    let v = VehicleTail::pack(&wheeled_view());
    assert!(!v.no_contacts);
    check_log(&v, VEHICLE);
    assert_eq!(bit_total(&v, &table()), 547);
    canonical(&v, &later_ctx());
    let w = WheeledTail::pack(&wheeled_view());
    check_log(
        &w,
        &format!(
            "{VEHICLE} tail.braking:1 {}",
            "tail.wheel.avel:32 tail.wheel.dy:32 tail.wheel.dx:32 tail.wheel.slipping:1 "
                .repeat(4)
                .trim_end()
        ),
    );
    assert_eq!(bit_total(&w, &table()), 547 + 1 + 4 * 97);
}

/// Ghost 1 is a Player, 5 a WheeledVehicle with four wheels, 7 a Camera, 9 a StaticShape.
fn class_of(index: u32) -> Option<TailClass> {
    match index {
        1 => Some(TailClass::Player),
        5 => Some(TailClass::Wheeled { wheels: 4 }),
        7 => Some(TailClass::Camera),
        9 => Some(TailClass::Shape),
        _ => None,
    }
}

fn written(data: &dyn ControlData) -> (Vec<u8>, usize) {
    let mut w = BitWriter::new();
    data.write(&mut w, widths().ghost_bits);
    let n = w.bits();
    (w.into_bytes(), n)
}

#[test]
fn a_player_driving_a_vehicle_writes_both_and_reads_back_by_class() {
    let gb = widths().ghost_bits;
    let vehicle = TgePacketData::new(TailData::Wheeled(WheeledTail::pack(&wheeled_view())));
    let mut player = TgePacketData::new(TailData::Player(PlayerTail::pack(&player_view(), gb)));
    assert_eq!(player.position, [10.0, 20.0, 30.0]);
    player.nested = Some(Box::new(vehicle.clone()));
    let (bytes, n) = written(&player);
    let player_bits = 64 + 3 + 8 + 8 + 197 + 96 + 1 + usize::from(gb);
    assert_eq!(n, player_bits + 547 + 1 + 4 * 97);
    let classes = class_of;
    let mut tails = TgeTails::new(&classes);
    let mut r = BitReader::with_bit_len(&bytes, n);
    let back = tails
        .read_control(1, &mut r, gb, &mut free_charge())
        .unwrap();
    assert_eq!(r.bits_left(), 0);
    assert!(back.same_as(&player));
    // The vehicle's data sets the stream's compression point last.
    assert_eq!(r.compression_point(), [1.0, 2.0, 3.0]);
    assert_eq!(checksum(back.as_ref(), gb), checksum(&player, gb));
    let mut other = player.clone();
    other.nested = None;
    assert!(!other.same_as(&player));
    assert_ne!(checksum(&other, gb), checksum(&player, gb));
}

#[test]
fn the_reader_picks_the_camera_and_plain_shapes_by_class() {
    let gb = widths().ghost_bits;
    let classes = class_of;
    let mut tails = TgeTails::new(&classes);
    let cam = CameraPacketData::fly([1.0, 2.0, 3.0], 0.5, 0.25);
    let (bytes, n) = written(&cam);
    let back = tails
        .read_control(
            7,
            &mut BitReader::with_bit_len(&bytes, n),
            gb,
            &mut free_charge(),
        )
        .unwrap();
    assert!(back.same_as(&cam));
    let shape = TgePacketData::new(TailData::Shape(ShapeTail::pack(&MapView::default().with(
        "energy",
        0,
        Value::F32(3.0),
    ))));
    let (bytes, n) = written(&shape);
    assert_eq!(n, 64);
    let back = tails
        .read_control(
            9,
            &mut BitReader::with_bit_len(&bytes, n),
            gb,
            &mut free_charge(),
        )
        .unwrap();
    assert!(back.same_as(&shape));
    // An index the client has no class for is an error.
    let got = tails.read_control(
        3,
        &mut BitReader::with_bit_len(&bytes, n),
        gb,
        &mut free_charge(),
    );
    assert!(got.is_err());
}

#[test]
fn nested_control_objects_are_capped() {
    let gb = widths().ghost_bits;
    // Every ghost is a Player whose control object is ghost 1: a chain as deep as the writer makes it.
    let all_players = |_: u32| Some(TailClass::Player);
    let view = with_link(MapView::default(), "control_object", Link::Ghost(1));
    let chain = |depth: usize| {
        let leaf = TgePacketData::new(TailData::Player(PlayerTail::pack(&MapView::default(), gb)));
        let mut data = leaf;
        for _ in 1..depth {
            let mut outer = TgePacketData::new(TailData::Player(PlayerTail::pack(&view, gb)));
            outer.nested = Some(Box::new(data));
            data = outer;
        }
        data
    };
    for (depth, ok) in [(MAX_NESTED_TAILS, true), (MAX_NESTED_TAILS + 1, false)] {
        let (bytes, n) = written(&chain(depth));
        let mut tails = TgeTails::new(&all_players);
        let got = tails.read_control(
            1,
            &mut BitReader::with_bit_len(&bytes, n),
            gb,
            &mut free_charge(),
        );
        assert_eq!(got.is_ok(), ok, "depth {depth}");
    }
    // A wheel count above a WheeledVehicle's most is refused.
    let many = |_: u32| Some(TailClass::Wheeled { wheels: 9 });
    let mut tails = TgeTails::new(&many);
    let bytes = vec![0u8; 256];
    assert!(
        tails
            .read_control(1, &mut BitReader::new(&bytes), gb, &mut free_charge())
            .is_err()
    );
}

#[test]
fn hostile_tails_never_panic() {
    let gb = widths().ghost_bits;
    let classes = class_of;
    let mut rng = Rng(0x1234_5678_9abc_def0);
    for _ in 0..2000 {
        let len = (rng.next() % 200) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        for index in [1, 5, 7, 9] {
            let mut tails = TgeTails::new(&classes);
            let _ = tails.read_control(index, &mut BitReader::new(&bytes), gb, &mut free_charge());
        }
    }
}

/// A small deterministic generator (xorshift64).
pub(super) struct Rng(pub u64);

impl Rng {
    pub(super) fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

/// A frame ends the data on a connection that carries frames: one bit for the world's, the parent's ghost and node for a
/// parent's; a reader of such a connection reads it back, and the checksum covers a parent's.
#[test]
fn a_frame_ends_the_data_and_reads_back_where_frames_are_on() {
    let gb = widths().ghost_bits;
    let classes = class_of;
    // A Player that controls nothing: nothing nested comes before the frame.
    let plain = TgePacketData::new(TailData::Player(PlayerTail::pack(&MapView::default(), gb)));
    let (_, plain_bits) = written(&plain);
    let frames = [
        (TailFrame::World, 1),
        (
            TailFrame::Parent {
                ghost: 5,
                node: None,
            },
            2 + usize::from(gb),
        ),
        (
            TailFrame::Parent {
                ghost: 5,
                node: Some(300),
            },
            2 + usize::from(gb) + 16,
        ),
    ];
    for (frame, bits) in frames {
        let mut data = plain.clone();
        data.frame = frame;
        let (bytes, n) = written(&data);
        assert_eq!(n, plain_bits + bits, "{frame:?}");
        let mut tails = TgeTails::new(&classes).with_frames(true);
        let mut r = BitReader::with_bit_len(&bytes, n);
        let back = tails
            .read_control(1, &mut r, gb, &mut free_charge())
            .unwrap();
        assert_eq!(r.bits_left(), 0, "{frame:?}");
        assert!(back.same_as(&data), "{frame:?}");
        if frame != TailFrame::World {
            assert_ne!(checksum(&data, gb), checksum(&plain, gb), "{frame:?}");
        }
    }
}
