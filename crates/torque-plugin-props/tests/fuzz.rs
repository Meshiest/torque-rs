//! Hostile input on both sides. `TORQUE_FUZZ_PROPS=<n>` sets the rounds (default 20,000).
//!
//! - A hostile server: records of every shape (decoded from random bytes, and made of random values) reach a client's book, its
//!   collision source and its draw function: nothing panics, the book stays within its cap, every shape it gives is finite. The
//!   same for joints: the book of joints, the tether it gives the predicted Player, the props a joint connects.
//! - Hostile callers: the `prop*` functions and `propCheckPlacement` called with junk on a running server: nothing panics and the
//!   caps hold.

#[path = "net/rig.rs"]
mod rig;

use rig::RigProps as _;

use std::collections::BTreeMap;

use torque_collide::source::{CollisionSource, ShapeKind};
use torque_engine::net_classes::DatablockFields;
use torque_game::player_tether::TetherSource;
use torque_ghosts::Value;
use torque_ghosts::codec::{NetWidths, ReadCtx, decode};
use torque_net::bits::BitReader;
use torque_net::huffman::Table;
use torque_plugin_props::client::{ClientSource, MAX_CLIENT_PROPS, draw};
use torque_plugin_props::data::{MAX_DRAW_SCALE, PropCollision, PropNumbers};
use torque_plugin_props::joints::client::{ClientTethers, MAX_CLIENT_JOINTS};
use torque_plugin_props::joints::records::{
    EndWire, FlagsWire as JointFlagsWire, InitWire as JointInitWire, LengthWire, SpringWire,
};
use torque_plugin_props::joints::{KindId, MAX_LENGTH};
use torque_plugin_props::quant::Quantized;
use torque_plugin_props::records::{FlagsWire, InitWire, StateWire};
use torque_plugin_props::world::MAX_PROPS;
use torque_plugin_props::{ClientJoints, ClientProps, JointGhost, PropGhost};

fn rounds() -> usize {
    std::env::var("TORQUE_FUZZ_PROPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000)
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        (self.next() >> 11) % n.max(1)
    }

    fn int(&mut self) -> i32 {
        match self.below(4) {
            0 => i32::MAX,
            1 => i32::MIN,
            2 => (self.next() as i32) >> (self.below(31) as i32),
            _ => 0,
        }
    }

    fn float(&mut self) -> f32 {
        match self.below(6) {
            0 => f32::NAN,
            1 => f32::INFINITY,
            2 => -1.0e30,
            _ => f32::from_bits(self.next() as u32),
        }
    }
}

/// A client's datablocks as a hostile server could make them.
struct Blocks(BTreeMap<(u32, String, u32), Value>);

impl DatablockFields for Blocks {
    fn field(&self, id: u32, name: &str, index: u32) -> Option<&Value> {
        self.0.get(&(id, name.to_string(), index))
    }
}

fn record(rng: &mut Rng) -> PropGhost {
    PropGhost {
        data_block: torque_ghosts::plugin::DatablockRef(
            (rng.below(2) == 0).then(|| rng.below(8) as u32),
        ),
        init: (rng.below(2) == 0).then(|| InitWire {
            slot: rng.next() as u16,
            mine: rng.below(2) == 0,
            predict: rng.next() as u8,
            hulls: None,
        }),
        state: (rng.below(2) == 0).then(|| {
            let w: [i32; 13] = std::array::from_fn(|_| rng.int());
            // Both the held form a decoder gives and raw words the book must hold itself.
            let q = Quantized::from_words(w);
            StateWire(if rng.below(2) == 0 {
                q
            } else {
                Quantized {
                    rot_index: rng.next() as u8,
                    rot: [rng.next() as u16; 3],
                    pos: [rng.int(); 3],
                    ..q
                }
            })
        }),
        flags: (rng.below(2) == 0).then(|| FlagsWire {
            asleep: rng.below(2) == 0,
            frozen: rng.below(2) == 0,
            held: rng.below(2) == 0,
        }),
    }
}

#[test]
fn a_hostile_servers_records_never_panic_the_client() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let book = ClientProps::default();
    let source = ClientSource {
        book: book.clone(),
        joints: Default::default(),
    };
    let table = Table::torque();
    let widths = NetWidths::engine_default();
    let mut blocks = Blocks(BTreeMap::new());
    let names = ["shapeFile", "drawScale", "size"];
    for id in 0..8u32 {
        for name in names {
            for k in 0..3 {
                let v = match rng.below(4) {
                    0 => Value::F32(rng.float()),
                    1 => Value::Str((0..rng.below(400)).map(|_| rng.next() as u8).collect()),
                    2 => Value::Int(rng.next() as i64),
                    _ => Value::Null,
                };
                blocks.0.insert((id, name.to_string(), k), v);
            }
        }
    }
    for round in 0..rounds() {
        let r = if round % 2 == 0 {
            let len = rng.below(48) as usize;
            let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            let mut reader = BitReader::new(&bytes);
            reader.enable_string_buffer();
            let ctx = ReadCtx::new(widths, round % 4 == 0);
            match decode::<PropGhost>(&mut reader, &table, &mut torque_ghosts::free_charge(), &ctx)
            {
                Ok(r) => r,
                Err(_) => record(&mut rng),
            }
        } else {
            record(&mut rng)
        };
        let index = rng.below(10_000) as u32;
        let mut params = |id: u32| {
            (!id.is_multiple_of(3)).then(|| {
                PropNumbers {
                    collision: if id.is_multiple_of(2) {
                        PropCollision::Shape
                    } else {
                        PropCollision::Box
                    },
                    size: [rng.float(), rng.float(), rng.float()],
                    radius: rng.float(),
                    mass: rng.float(),
                    density: rng.float(),
                    friction: rng.float(),
                    restitution: rng.float(),
                    linear_damping: rng.float(),
                    angular_damping: rng.float(),
                    max_speed: rng.float(),
                    sleep_linear: rng.float(),
                    sleep_angular: rng.float(),
                    can_freeze: true,
                    max_per_player: rng.int(),
                }
                .held()
            })
        };
        let p = params(round as u32);
        book.0.borrow_mut().heard(index, &r, |_| p);
        if rng.below(5) == 0 {
            book.0.borrow_mut().removed(rng.below(10_000) as u32);
        }
        if let Some(shape) = draw(&r, &blocks) {
            assert!(
                shape
                    .scale
                    .iter()
                    .all(|s| s.is_finite() && s.abs() <= MAX_DRAW_SCALE)
            );
            assert!(!shape.shape.is_empty() && shape.shape.len() <= 96);
        }
        if round % 64 == 0 {
            let mut out = Vec::new();
            source.shapes(&mut out);
            for s in &out {
                assert!(s.key < MAX_PROPS);
                assert!(s.pose.iter().all(|v| v.is_finite()), "{:?}", s.pose);
                let ShapeKind::Box { half } = s.kind else {
                    panic!("a prop is a box");
                };
                assert!(half.iter().all(|h| h.is_finite() && *h > 0.0));
            }
        }
        assert!(book.0.borrow().len() <= MAX_CLIENT_PROPS);
    }
}

#[test]
fn hostile_commands_and_calls_never_panic_the_server_and_the_caps_hold() {
    let (mut rig, _files) = rig::clients(1, &[("$Test::Players", "1")], &[]);
    let client = rig.eval("return ClientGroup.getObject(0);");
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let junk = |rng: &mut Rng| -> String {
        let words = [
            "Crate",
            "Slider",
            "FloorData",
            "EyeData",
            "",
            "0",
            "-1",
            "1e40",
            "nan",
            "Crate Crate",
            "\\\\x00",
        ];
        match rng.below(3) {
            0 => words[rng.below(words.len() as u64) as usize].to_string(),
            1 => format!("{} {} {}", rng.float(), rng.float(), rng.float()),
            _ => (0..rng.below(80))
                .map(|_| char::from(b' ' + (rng.next() % 94) as u8))
                .filter(|c| *c != '"' && *c != '\\')
                .collect(),
        }
    };
    let calls = (rounds() / 20).max(500);
    for k in 0..calls {
        let (a, b, c) = (junk(&mut rng), junk(&mut rng), junk(&mut rng));
        let id = rng.int();
        let code = match k % 8 {
            0 => format!("propCheckPlacement(\"{a}\", \"{b}\", \"{c}\", {id});"),
            1 => format!("propCheckPlacement(Crate, \"{b}\", \"{c}\", {client});"),
            2 => format!("propSpawn(\"{a}\", \"{b}\", \"{c}\");"),
            3 => format!("propSpawn(Slider, \"{b}\", {client}, \"{c}\");"),
            4 => {
                format!("propApplyImpulse({id}, \"{a}\"); propSetVelocity({id}, \"{b}\", \"{c}\");")
            }
            5 => format!(
                "propFreeze({id}); propUnfreeze({id}); propSleep({id}); propGetTransform({id}); propGetOwner({id});"
            ),
            6 => format!(
                "propDelete({id}); propCount(\"{a}\"); propIsAsleep({id}); propRayCast(\"{a}\", \"{b}\", {id});"
            ),
            _ => {
                rig.step();
                continue;
            }
        };
        // A refusal is a script error: what matters is that nothing panics and the server keeps running.
        let _ = rig.server.vm_mut().eval(code.as_bytes());
        // Props a call made get pushed about by junk too.
        if k % 16 == 0 {
            let _ = rig.server.vm_mut().eval(
                format!(
                    "for (%i = 0; %i < MissionCleanup.getCount(); %i++) {{ %o = MissionCleanup.getObject(%i); if (%o.getClassName() $= \"Prop\") propSetVelocity(%o, \"{a}\", \"{b}\"); }}"
                )
                .as_bytes(),
            );
        }
    }
    rig.run(30);
    let props = rig.props();
    let s = props.0.borrow();
    let world = s.world.as_ref().expect("a world");
    assert!(world.len() as u32 <= MAX_PROPS);
    assert!(!world.physics().is_poisoned());
    for p in world.props().values() {
        let st = p.q.state();
        assert!(st.position.iter().chain(&st.linvel).all(|v| v.is_finite()));
    }
    assert!(rig.peers[0].ghosts() > 0, "the client is still there");
}

fn end(rng: &mut Rng) -> EndWire {
    match rng.below(4) {
        0 => EndWire::Free,
        1 => EndWire::Point([rng.float(), rng.float(), rng.float()]),
        2 => EndWire::Ghost {
            ghost: rng.below(16) as u32,
            offset: [rng.float(), rng.float(), rng.float()],
        },
        _ => EndWire::Waiting,
    }
}

fn joint_record(rng: &mut Rng) -> JointGhost {
    JointGhost {
        init: (rng.below(2) == 0).then(|| JointInitWire {
            kind: KindId::of(rng.below(8) as u32),
            a: end(rng),
            b: end(rng),
        }),
        length: (rng.below(2) == 0).then(|| LengthWire {
            length: rng.float(),
            min: rng.float(),
        }),
        spring: (rng.below(2) == 0).then(|| SpringWire {
            stiffness: rng.float(),
            damping: rng.float(),
        }),
        flags: (rng.below(2) == 0).then(|| JointFlagsWire {
            at_limit: rng.below(2) == 0,
            broken: rng.below(2) == 0,
        }),
    }
}

#[test]
fn a_hostile_servers_joint_records_never_panic_the_client() {
    let mut rng = Rng(0x1405_7b7e_f767_814f);
    let joints = ClientJoints::default();
    let props = ClientProps::default();
    let tethers = ClientTethers {
        joints: joints.clone(),
        props: props.clone(),
    };
    let table = Table::torque();
    let widths = NetWidths::engine_default();
    let mut decoded = 0;
    for round in 0..rounds() {
        let r = if round % 2 == 0 {
            let len = rng.below(48) as usize;
            let bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            let mut reader = BitReader::new(&bytes);
            reader.enable_string_buffer();
            let ctx = ReadCtx::new(widths, round % 4 == 0);
            match decode::<JointGhost>(&mut reader, &table, &mut torque_ghosts::free_charge(), &ctx)
            {
                Ok(r) => {
                    decoded += 1;
                    // What a decoder gives is held to its ranges.
                    if let Some(l) = r.length {
                        assert!(l.length.is_finite() && (0.0..=MAX_LENGTH).contains(&l.length));
                        assert!(l.min.is_finite() && l.min <= l.length);
                    }
                    if let Some(s) = r.spring {
                        assert!(s.stiffness.is_finite() && s.damping.is_finite());
                    }
                    r
                }
                Err(_) => joint_record(&mut rng),
            }
        } else {
            joint_record(&mut rng)
        };
        let index = rng.below(10_000) as u32;
        joints.0.borrow_mut().heard(index, &r);
        // Props the joints may name, with hostile states.
        if round % 8 == 0 {
            props
                .0
                .borrow_mut()
                .heard(rng.below(16) as u32, &record(&mut rng), |_| None);
        }
        if rng.below(5) == 0 {
            joints.0.borrow_mut().removed(rng.below(10_000) as u32);
        }
        for key in [rng.below(16) as u32, index] {
            if let Some(t) = tethers.tether(key) {
                assert!(t.anchor.iter().chain(&t.offset).all(|v| v.is_finite()));
                assert!(t.length.is_finite() && t.stiffness.is_finite() && t.damping.is_finite());
            }
            let _ = joints.0.borrow().connected(key);
        }
        assert!(joints.0.borrow().len() <= MAX_CLIENT_JOINTS);
    }
    assert!(decoded > 0, "some random bytes decode");
}

#[test]
fn hostile_joint_calls_never_panic_the_server_and_the_caps_hold() {
    let (mut rig, _files) = rig::clients(1, &[("$Test::Players", "1")], &[]);
    let client = rig.eval("return ClientGroup.getObject(0);");
    let player = rig.eval(&format!("return {client}.getControlObject();"));
    for k in 0..12 {
        rig.eval(&format!("propSpawn(Crate, \"{} 4 {}\");", k * 2, 1 + k % 3));
    }
    let things = rig.eval(
        "%s = \"\"; for (%i = 0; %i < MissionCleanup.getCount(); %i++) { %o = MissionCleanup.getObject(%i); \
         if (%o.getClassName() $= \"Prop\") %s = %s SPC %o; } return %s;",
    );
    let props: Vec<String> = things.split_whitespace().map(String::from).collect();
    let mut rng = Rng(0x5851_f42d_4c95_7f2d);
    let pick = |rng: &mut Rng| -> String {
        match rng.below(6) {
            0 => props[rng.below(props.len() as u64) as usize].clone(),
            1 => format!("{} {} {}", rng.float(), rng.float(), rng.float()),
            2 => format!(
                "{} {} {} {}",
                props[rng.below(props.len() as u64) as usize],
                rng.float(),
                rng.float(),
                rng.float()
            ),
            3 => player.clone(),
            4 => String::new(),
            _ => rng.int().to_string(),
        }
    };
    let calls = (rounds() / 20).max(500);
    for k in 0..calls {
        let (a, b, c) = (pick(&mut rng), pick(&mut rng), pick(&mut rng));
        let (x, y) = (rng.float(), rng.float());
        let j = rng.int();
        let code = match k % 9 {
            0 => format!("jointTether(\"{a}\", \"{b}\", {x}, {client});"),
            1 => format!("jointGrapple(\"{a}\", \"{b}\", {x});"),
            2 => format!("jointWeld(\"{a}\", \"{b}\", {client});"),
            3 => format!("jointHinge(\"{a}\", \"{b}\", \"{c}\", \"{c}\");"),
            4 => format!("jointWinch({j}, {x}, {y}, {x}); jointSetLength({j}, {x}, {y});"),
            5 => format!(
                "jointSetSpring({j}, {x}, {y}); jointSetBreakForce({j}, {x}); jointSetLimits({j}, {x}, {y}); jointSetMotor({j}, {x}, {y});"
            ),
            6 => {
                let kind = [
                    "", "tether", "winch", "weld", "hinge", "grapple", "hold", "rope",
                ][rng.below(8) as usize];
                format!(
                    "jointCheck({client}, \"{a}\", \"{b}\", \"{c}\", {x}, \"{kind}\", \"{c}\", {j}, {j});"
                )
            }
            7 => format!(
                "jointDelete({j}); jointGetLength({j}); jointCount({client}); jointHold(\"{a}\", \"{b}\", \"{c}\", {client});"
            ),
            _ => {
                rig.step();
                continue;
            }
        };
        let _ = rig.server.vm_mut().eval(code.as_bytes());
        if k % 32 == 0 {
            // The joints the calls made get their own junk too.
            let _ = rig.server.vm_mut().eval(
                format!(
                    "for (%i = 0; %i < MissionCleanup.getCount(); %i++) {{ %o = MissionCleanup.getObject(%i); \
                     if (%o.getClassName() $= \"Joint\") {{ jointSetLength(%o, {x}, {y}); jointWinch(%o, {y}, {x}); jointSetSpring(%o, {x}, {y}); }} }}"
                )
                .as_bytes(),
            );
        }
    }
    rig.run(60);
    let props = rig.props();
    let s = props.0.borrow();
    let world = s.world.as_ref().expect("a world");
    let limits = world.joint_limits;
    assert!(world.joints().len() as u32 <= limits.per_server);
    for p in world.props().keys() {
        assert!(world.joints_on(*p) <= limits.per_prop);
    }
    assert!(!world.physics().is_poisoned());
    for j in world.joints().values() {
        assert!(j.status.distance.is_finite() && j.status.force.is_finite());
    }
    for p in world.props().values() {
        let st = p.q.state();
        assert!(st.position.iter().chain(&st.linvel).all(|v| v.is_finite()));
    }
    assert!(rig.peers[0].ghosts() > 0, "the client is still there");
}
