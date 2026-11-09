//! What a script's `setTransform` and the record under TGE's codecs say about an Item, a Camera and a StaticShape.
//!
//! TGE marks the object in each class's own `setTransform` and writes the pose from the object's state when the record is packed:
//! `StaticShape::setTransform` marks `PositionMask`; `Item::setTransform` marks `RotationMask | PositionMask | NoWarpMask` and its
//! `packUpdate` writes the rotation as an angle about z (`AngAxisF(mObjToWorld)`: the sign of the axis' z, then the angle);
//! `Camera::setTransform` marks nothing (only `processTick` with a move marks `MoveMask`), and its `packUpdate` writes the
//! position and `mRot.x` and `mRot.z`.
//!
//! Credit: TGE 1.4's `game/staticShape.cc`, `game/item.cc` and `game/camera.cc` (Copyright (C) GarageGames.com, Inc.), as the
//! MIT-licensed Torque3D carries them in `T3D/staticShape.cpp`, `T3D/item.cpp` and `T3D/camera.cpp` (each Copyright (c) 2012
//! GarageGames, LLC). The code here is this crate's own.

use std::cell::RefCell;
use std::rc::Rc;

use torque_game::scene::container::{Container, ContainerConfig};
use torque_ghosts::codec::{F32, NetWidths};
use torque_ghosts::shapebase::mask::NO_WARP;
use torque_ghosts::shapes::{
    CAMERA_MOVE_MASK, STATIC_POSITION_MASK, item_mask, pack_camera, pack_item,
};
use torque_ghosts::{ObjectView, PackCtx};
use torque_net::bits::BitWriter;
use torque_net::events::ConnectionMessage;
use torque_net::handshake::ConnId;
use torque_net::huffman::Table;
use torque_scene::BinGeometry;
use torque_server::net::ghosts::{GhostHub, SourceEnv};
use torque_server::net::torque::{FieldView, GameView, NetPasses, TgeGhostGame, TorqueNet};
use torque_server::net::transport::{MemoryTransport, SharedTransport};
use torque_server::net::{Host, HostParts, NetClock, attach_host};
use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::vm::Vm;

const GHOST_BITS: u8 = 8;
const CONN: ConnId = ConnId(1);

struct Scene {
    server: Server<HostParts<TorqueNet>>,
    hub: GhostHub<TgeGhostGame>,
    table: Table,
    seq: u32,
}

impl Scene {
    fn new() -> Scene {
        let mut vm = Vm::new();
        torque_game::specs::install(&mut vm);
        let files = MemFiles::new();
        let b = [-1.0, -2.0, 0.0, 1.0, 2.0, 1.0];
        let car = torque_shapes::synth::hull_shape(b, &[("detail2", b)], &[], &[]);
        files.insert("car.dts", &torque_shapes::write::write_dts(&car));
        vm.set_file_host(files);
        let state = Rc::new(RefCell::new(ServerState::<HostParts<TorqueNet>>::default()));
        torque_server::game::install(&mut vm, &state);
        let mut server = Server::with_state(vm, Rc::clone(&state));
        let (transport, _end) = MemoryTransport::new();
        let transport: SharedTransport = Rc::new(RefCell::new(transport));
        let container = Rc::new(Container::new(ContainerConfig {
            bins: BinGeometry::default(),
            indexes: Vec::new(),
            candidate_order: None,
        }));
        let shared = attach_host(
            &mut server.vm,
            Host {
                transport,
                game: TorqueNet::default(),
                engine: Rc::clone(&state),
                clock: NetClock::Sim,
                ghosts: TgeGhostGame,
                container,
                static_writes: None,
            },
        );
        server
            .attach_net(Rc::new(NetPasses(Rc::clone(&shared))))
            .expect("a net host");
        server.run(TICK_MS, |_| false);
        let script = "datablock CameraData(EyeData) {};
datablock StaticShapeData(PropData) { shapeFile = \"car.dts\"; };
datablock ItemData(CoinData) { shapeFile = \"car.dts\"; };
new Camera(Eye) { dataBlock = EyeData; };
new Camera(Spare) { dataBlock = EyeData; };
new StaticShape(Prop) { dataBlock = PropData; };
new Item(Coin) { dataBlock = CoinData; };
new ScriptObject(Client);
";
        server.vm.eval(script.as_bytes()).expect("the scene");
        let hub = shared.borrow().ghosts.clone().expect("a hub");
        let client = server.vm.objects().find(b"Client").expect("client");
        let eye = server.vm.objects().find(b"Eye").expect("eye");
        hub.connect(CONN, client, GHOST_BITS);
        hub.set_control(CONN, Some(eye));
        let env = SourceEnv {
            vm: &server.vm,
            table: None,
            ghost_bits: GHOST_BITS,
        };
        hub.activate(env, CONN).expect("activates");
        let seq = hub.with(|h| h.conns[&CONN].mgr.sequence());
        hub.on_message(
            CONN,
            &ConnectionMessage {
                seq,
                message: 1,
                count: 0,
            },
        )
        .expect("ready");
        Scene {
            server,
            hub,
            table: Table::torque(),
            seq: 1,
        }
    }

    fn pass(&mut self) -> usize {
        self.hub.collapse();
        self.hub.begin_pass();
        let env = SourceEnv {
            vm: &self.server.vm,
            table: Some(&self.table),
            ghost_bits: GHOST_BITS,
        };
        let mut w = BitWriter::with_limit(8_000);
        w.enable_string_buffer();
        let stats = self
            .hub
            .write_section(env, CONN, &mut w, 8_000, self.seq)
            .expect("a section");
        self.hub.on_notify(CONN, self.seq, true);
        self.hub.end_pass(0);
        self.seq += 1;
        stats.records
    }

    fn settle(&mut self) {
        for _ in 0..50 {
            if self.pass() == 0 {
                return;
            }
        }
        panic!("never settled");
    }

    /// The record bits marked on `name` that no pass has taken yet.
    fn pending(&self, name: &str) -> u32 {
        let obj = self
            .server
            .vm
            .objects()
            .find(name.as_bytes())
            .expect("the object");
        self.hub.with(|h| h.reg.pending(obj))
    }

    /// Runs `script` on a settled scene.
    fn run(&mut self, script: &str) {
        self.settle();
        self.server.vm.eval(script.as_bytes()).expect("the script");
    }

    /// The view the ghost pass packs `name` from.
    fn view(&self, name: &str) -> GameView<'_> {
        let vm = &self.server.vm;
        let obj = vm.objects().find(name.as_bytes()).expect("the object");
        GameView {
            fields: FieldView { vm, obj },
            controlled: false,
            ghost_of: None,
            connection: None,
        }
    }
}

fn pack_ctx() -> PackCtx {
    PackCtx {
        widths: NetWidths::new(GHOST_BITS, 10),
        now_ms: 0,
        compression_point: [0.0; 3],
    }
}

/// A matrix text for `setTransform`: a position and a rotation of `angle` radians about z.
const TURNED: &str = "1 2 3 0 0 1 1";

#[test]
fn an_items_set_transform_marks_its_rotation_position_and_warp() {
    let mut s = Scene::new();
    s.run(&format!("Coin.setTransform(\"{TURNED}\");"));
    assert_eq!(
        s.pending("Coin"),
        item_mask::ROTATION | item_mask::POSITION | NO_WARP
    );
}

#[test]
fn a_static_shapes_set_transform_marks_its_position() {
    let mut s = Scene::new();
    s.run(&format!("Prop.setTransform(\"{TURNED}\");"));
    assert_eq!(s.pending("Prop"), STATIC_POSITION_MASK);
}

#[test]
fn a_cameras_set_transform_marks_nothing() {
    let mut s = Scene::new();
    s.run(&format!("Spare.setTransform(\"{TURNED}\");"));
    assert_eq!(s.pending("Spare"), 0);
}

#[test]
fn an_items_record_carries_its_rotation_about_z() {
    let mut s = Scene::new();
    s.run(&format!("Coin.setTransform(\"{TURNED}\");"));
    let (item, _) = pack_item(&s.view("Coin"), item_mask::ROTATION, &pack_ctx());
    let (negative, angle) = item.rotation.expect("the rotation block");
    assert!(!negative, "the axis points up");
    assert!((angle.get() - 1.0).abs() < 1e-5, "angle {}", angle.get());
}

#[test]
fn a_cameras_record_carries_its_pitch_and_yaw() {
    let mut s = Scene::new();
    s.run(&format!("Spare.setTransform(\"{TURNED}\");"));
    let camera = pack_camera(&s.view("Spare"), CAMERA_MOVE_MASK, &pack_ctx());
    let pose = camera.pose.expect("the move block");
    assert_eq!(
        [pose[0].get(), pose[1].get(), pose[2].get()],
        [1.0, 2.0, 3.0]
    );
    assert!(pose[3].get().abs() < 1e-5, "pitch {}", pose[3].get());
    assert!((pose[4].get() - 1.0).abs() < 1e-5, "yaw {}", pose[4].get());
}

#[test]
fn the_view_has_a_position_for_each_of_the_three() {
    let mut s = Scene::new();
    s.run(&format!(
        "Prop.setTransform(\"{TURNED}\"); Coin.setTransform(\"{TURNED}\"); Spare.setTransform(\"{TURNED}\");"
    ));
    for name in ["Prop", "Coin", "Spare"] {
        let v = s.view(name);
        let position: Vec<_> = (0..3)
            .map(|i| format!("{:?}", v.get("position", i)))
            .collect();
        assert_eq!(position, ["F32(1.0)", "F32(2.0)", "F32(3.0)"], "{name}");
    }
}

#[test]
fn an_items_position_block_is_a_rest_flag_or_the_velocity() {
    let mut s = Scene::new();
    s.run("Coin.setVelocity(\"1 2 3\");");
    let (moving, _) = pack_item(&s.view("Coin"), item_mask::POSITION, &pack_ctx());
    let position = moving.position.expect("the position block");
    assert_eq!(position.position, [0.0, 0.0, 0.0].map(F32::new));
    assert_eq!(position.velocity, Some([1.0, 2.0, 3.0].map(F32::new)));

    let obj = s.server.vm.objects().find(b"Coin").expect("the item");
    torque_game::sim::item::state_mut(&mut s.server.vm, obj)
        .expect("the item's state")
        .at_rest = true;
    let (resting, _) = pack_item(&s.view("Coin"), item_mask::POSITION, &pack_ctx());
    assert_eq!(resting.position.expect("the position block").velocity, None);
}

#[test]
fn an_item_turned_about_minus_z_writes_the_axis_flag_and_a_positive_angle() {
    let mut s = Scene::new();
    s.run("Coin.setTransform(\"1 2 3 0 0 -1 1\");");
    let (item, _) = pack_item(&s.view("Coin"), item_mask::ROTATION, &pack_ctx());
    let (negative, angle) = item.rotation.expect("the rotation block");
    assert!(negative, "the axis points down");
    assert!((angle.get() - 1.0).abs() < 1e-5, "angle {}", angle.get());
}
