//! A synthetic session per TGE class: torque-net's ghost manager packs TGE records (`torque-ghosts`' packers and codecs) from a
//! host's object views, torque-net's `GhostClient` reads them with the TGE codec, and the client's world follows the table.

use std::collections::{BTreeMap, HashMap};

use ghosts::{
    GhostCodecSet, Link, NetStr, NetWidths, ObjectView, PackCtx, TgeClass, TgeGhosts, Value,
};
use net::bits::{BitReader, BitWriter};
use net::classes::{Kind, NetClasses};
use net::events::{ConnectionMessage, EventRegistry};
use net::ghost::{
    CamInfo, GhostClient, GhostConfig, GhostManager, MessageOutcome, NoScripts, ObjId,
    ObjectSource, SectionBudget, net_flags,
};
use net::huffman::Table;
use torque_ghost_world::ClientWorld;
use torque_ghost_world::effects::Effect;
use torque_ghost_world::images::{ImageEvent, TICK_MS};
use torque_ghost_world::tge::{TgeClientSet, TgeCodec, TgeDatablock};
use torque_ghost_world::threads::ThreadState;
use torque_ghost_world::world::Pose;

/// An object's fields as the host's getters give them.
#[derive(Debug, Clone, Default)]
struct MapView {
    vals: HashMap<(String, usize), Value>,
    links: HashMap<String, ObjId>,
}

impl MapView {
    fn with(mut self, name: &str, index: usize, v: Value) -> MapView {
        self.vals.insert((name.to_string(), index), v);
        self
    }

    fn floats(mut self, name: &str, xs: &[f32]) -> MapView {
        for (i, x) in xs.iter().enumerate() {
            self.vals.insert((name.to_string(), i), Value::F32(*x));
        }
        self
    }

    fn set(&mut self, name: &str, index: usize, v: Value) {
        self.vals.insert((name.to_string(), index), v);
    }
}

/// A view whose links resolve against the connection's ghosts.
struct Linked<'a> {
    view: &'a MapView,
    ghost_of: &'a dyn Fn(ObjId) -> Option<u32>,
}

impl ObjectView for Linked<'_> {
    fn get(&self, field: &str, index: usize) -> Value {
        self.view
            .vals
            .get(&(field.to_string(), index))
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn link(&self, field: &str) -> Link {
        match self.view.links.get(field) {
            None => Link::Null,
            Some(o) => (self.ghost_of)(*o).map_or(Link::NotGhosted, Link::Ghost),
        }
    }

    fn net_string(&self, _field: &str, _index: usize) -> NetStr {
        NetStr::Null
    }
}

struct Obj {
    class: TgeClass,
    view: MapView,
}

/// The server's objects.
struct Host {
    objs: BTreeMap<ObjId, Obj>,
    classes: NetClasses,
    widths: NetWidths,
    table: Table,
    now: u32,
    always: Vec<ObjId>,
}

impl ObjectSource for Host {
    fn exists(&self, obj: ObjId) -> bool {
        self.objs.contains_key(&obj)
    }

    fn class_id(&self, obj: ObjId) -> Option<u32> {
        let o = self.objs.get(&obj)?;
        self.classes.id_of(Kind::Object, o.class.name())
    }

    fn net_flags(&self, obj: ObjId) -> u32 {
        if self.objs.contains_key(&obj) {
            net_flags::GHOSTABLE
        } else {
            0
        }
    }

    fn pack_update(
        &mut self,
        obj: ObjId,
        mask: u32,
        w: &mut BitWriter,
        ghost_of: &dyn Fn(ObjId) -> Option<u32>,
    ) -> u32 {
        let Some(o) = self.objs.get(&obj) else {
            return 0;
        };
        let view = Linked {
            view: &o.view,
            ghost_of,
        };
        let ctx = PackCtx::for_writer(w, self.widths, self.now);
        let (record, ret) = TgeGhosts::pack(o.class, &view, mask, &ctx);
        TgeGhosts::encode(&record, w, &self.table, self.widths).expect("encode");
        ret
    }

    fn update_priority(&self, _obj: ObjId, _cam: &CamInfo, _mask: u32, _skip: u8) -> f32 {
        1.0
    }

    fn camera_info(&self, _scope: ObjId) -> Option<CamInfo> {
        Some(CamInfo::default())
    }

    fn sky_visible_distance(&self) -> Option<f32> {
        None
    }

    fn mount_of(&self, _scope: ObjId) -> Option<ObjId> {
        None
    }

    fn scope_query(
        &self,
        _scope: ObjId,
        _cam: &CamInfo,
        _radius: f32,
        visit: &mut dyn FnMut(ObjId) -> bool,
    ) {
        for &id in self.objs.keys() {
            if !visit(id) {
                return;
            }
        }
    }

    fn scope_distance(&self, obj: ObjId, _scope: ObjId) -> Option<f32> {
        self.objs.contains_key(&obj).then_some(1.0)
    }

    fn ghost_always(&self) -> &[ObjId] {
        &self.always
    }
}

/// A server and a client joined over one connection, the client's world following its ghost table.
struct Session {
    mgr: GhostManager,
    host: Host,
    client: GhostClient<TgeCodec>,
    world: ClientWorld<TgeClientSet>,
    seq: u32,
}

impl Session {
    fn new() -> Session {
        let classes = NetClasses::derive(&ghosts::tge_reps(&EventRegistry::torque())).unwrap();
        let widths = NetWidths::engine_default();
        let cfg = GhostConfig {
            class_bits: classes.id_bits(Kind::Object),
            ..GhostConfig::standard()
        };
        let names: Vec<&str> = classes
            .classes(Kind::Object)
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        let codec = TgeCodec::new(names, widths, Table::torque());
        let mut client = GhostClient::new(codec, cfg.class_bits, cfg.ghost_bits());
        client.enable_log();
        let mut mgr = GhostManager::new(cfg).unwrap();
        let host = Host {
            objs: BTreeMap::new(),
            classes,
            widths,
            table: Table::torque(),
            now: 0,
            always: Vec::new(),
        };
        mgr.activate(&host).unwrap();
        let seq = mgr.sequence();
        let ready = mgr
            .on_connection_message(
                &ConnectionMessage {
                    seq,
                    message: 1,
                    count: 0,
                },
                &mut NoScripts,
            )
            .unwrap();
        assert_eq!(ready, MessageOutcome::Ready);
        Session {
            mgr,
            host,
            client,
            world: ClientWorld::default(),
            seq: 0,
        }
    }

    fn add(&mut self, id: ObjId, class: TgeClass, view: MapView) {
        self.host.objs.insert(id, Obj { class, view });
        self.mgr.object_in_scope_now(&self.host, id);
    }

    fn view(&mut self, id: ObjId) -> &mut MapView {
        &mut self.host.objs.get_mut(&id).unwrap().view
    }

    /// One packet at `now` ms: the server writes its ghost section, the client reads it and the world takes the changes.
    fn step(&mut self, now: u32) {
        self.seq += 1;
        self.host.now = now;
        let mut w = BitWriter::with_limit(12_000);
        w.enable_string_buffer();
        self.mgr.write_section(
            &mut w,
            8 * 1400,
            self.seq,
            now,
            &mut self.host,
            SectionBudget {
                scan: &mut |_| true,
                records: &mut |_| true,
            },
        );
        let bits = w.bits();
        let bytes = w.into_bytes();
        let mut r = BitReader::with_bit_len(&bytes, bits);
        r.enable_string_buffer();
        self.client.read_section(&mut r).unwrap();
        assert_eq!(r.bits_left(), 0);
        self.mgr.on_notify(self.seq, true);
        let changes = self.client.take_log();
        self.world.set_clock(now);
        self.world
            .apply_ghosts(self.seq as usize, &changes, &self.client, [0.0; 3], &mut ());
    }

    /// The ghost index of host object `id`.
    fn ghost(&self, id: ObjId) -> u32 {
        self.mgr.ghost_index(id).unwrap()
    }
}

/// An image datablock: Activate (0.1 s) -> Ready; Ready waits for the trigger; Fire (0.2 s, a sound and an emitter) -> Ready.
fn image_datablock() -> TgeDatablock {
    let mut fields = Vec::new();
    let trans = |timeout: i32, trigger_down: i32| {
        let mut t = [-1; 11];
        t[10] = timeout;
        t[9] = trigger_down;
        t
    };
    let states: [([i32; 11], f32, bool, bool); 3] = [
        (trans(1, -1), 0.1, true, false),
        (trans(-1, 2), 0.0, false, false),
        (trans(1, -1), 0.2, true, true),
    ];
    for (i, (t, timeout, wait, fire)) in states.into_iter().enumerate() {
        let i = i as u32;
        let mut push = |name: &str, index: u32, v: Value| fields.push((name.to_string(), index, v));
        push("stateUsed", i, Value::Bool(true));
        for (k, to) in t.into_iter().enumerate() {
            push(
                "stateTrans",
                11 * i + k as u32,
                Value::Int(i64::from(to + 1)),
            );
        }
        push("stateTimeoutValue", i, Value::F32(timeout));
        push("stateWaitForTimeout", i, Value::Bool(wait));
        push("stateFire", i, Value::Bool(fire));
    }
    fields.push(("stateSound".to_string(), 2, Value::Id(Some(90))));
    fields.push(("stateEmitter".to_string(), 2, Value::Id(Some(91))));
    fields.push(("stateEmitterTime".to_string(), 2, Value::F32(0.5)));
    TgeDatablock {
        class: "ShapeBaseImageData".to_string(),
        fields,
    }
}

#[test]
fn a_player_brings_its_image_thread_sound_and_moves() {
    let mut s = Session::new();
    s.world.apply_datablock(70, image_datablock());
    let player = MapView::default()
        .with("datablock", 0, Value::Id(Some(60)))
        .floats("position", &[10.0, 20.0, 5.0])
        .floats("rotation", &[0.0, 0.0, 1.0])
        .with("max_look_angle", 0, Value::F32(1.4))
        .with("energy", 0, Value::F32(50.0))
        .with("max_energy", 0, Value::F32(100.0))
        .with("image_datablock", 0, Value::Id(Some(70)))
        .with("image_loaded", 0, Value::Bool(true))
        .with("thread_sequence", 0, Value::Int(3))
        .with("thread_sequence", 1, Value::Int(-1))
        .with("thread_sequence", 2, Value::Int(-1))
        .with("thread_sequence", 3, Value::Int(-1))
        .with("sound_play", 1, Value::Bool(true))
        .with("sound_profile", 1, Value::Id(Some(88)));
    s.add(1, TgeClass::Player, player);
    s.step(1000);
    let g = s.ghost(1);
    let ghost = &s.world.ghosts[&g];
    assert_eq!(ghost.class, Some(TgeClass::Player));
    assert_eq!(ghost.datablock(), Some(60));
    let Some(Pose::Euler { position, yaw, .. }) = ghost.pose() else {
        panic!("{:?}", ghost.pose());
    };
    assert!(
        position
            .iter()
            .zip([10.0, 20.0, 5.0])
            .all(|(a, b)| (a - b).abs() < 0.01),
        "{position:?}"
    );
    assert!((yaw - 1.0).abs() < 0.06, "{yaw}");
    // The image entered Activate, the thread plays, the sound slot plays its profile.
    let images = s.world.mounted_images(g);
    assert_eq!(images.len(), 1);
    assert_eq!(
        (images[0].0, images[0].1.datablock, images[0].1.state),
        (0, 70, 0)
    );
    let t = s.world.threads[&g][0].unwrap();
    assert_eq!((t.sequence, t.state), (3, ThreadState::Play));
    assert!(s.world.effects_after(0).any(|e| e.effect
        == Effect::ShapeSound {
            ghost: g,
            slot: 1,
            profile: Some(88),
        }));
    // Ready after Activate's 0.1 s.
    s.step(1000 + 5 * TICK_MS);
    assert_eq!(s.world.mounted_images(g)[0].1.state, 1);
    // A shot: the fire count moves on, the client enters Fire (its sound and emitter).
    let seen = s.world.effect_seq;
    s.view(1).set("image_fire_count", 0, Value::Int(1));
    s.mgr.set_mask(1, ghosts::shapebase::mask::IMAGE_N);
    s.step(1200);
    let m = &s.world.mounted_images(g)[0].1;
    assert_eq!((m.state, m.fire_count), (2, 1));
    let events: Vec<ImageEvent> = s
        .world
        .effects_after(seen)
        .filter_map(|e| match e.effect {
            Effect::Image { ghost, event, .. } if ghost == g => Some(event),
            _ => None,
        })
        .collect();
    assert!(
        events.contains(&ImageEvent::Sound { profile: 90 }),
        "{events:?}"
    );
    assert!(events.iter().any(|e| matches!(
        e,
        ImageEvent::Emitter {
            datablock: Some(91),
            ..
        }
    )));
    // A move: the client keeps it in order and moves the ghost.
    s.view(1).set("position", 0, Value::F32(12.0));
    s.mgr.set_mask(1, ghosts::player::mask::MOVE);
    s.step(1300);
    let last = s.world.player_moves.back().unwrap();
    assert_eq!(last.index, g);
    assert!((last.position[0] - 12.0).abs() < 0.01);
    assert!((s.world.ghosts[&g].position.unwrap()[0] - 12.0).abs() < 0.01);
    assert_eq!(s.world.ghosts[&g].updates, 2);
    // The image, thread and sound blocks merged in the kept record.
    let rec = s.world.ghosts[&g].record.as_ref().unwrap();
    let body = torque_ghost_world::tge::shape_part(rec)
        .unwrap()
        .body
        .as_ref()
        .unwrap();
    assert_eq!(
        body.images.as_ref().unwrap()[0]
            .as_ref()
            .unwrap()
            .fire_count,
        1
    );
    assert!(body.threads.as_ref().unwrap()[0].is_some());
}

#[test]
fn a_camera_the_client_does_not_control_moves_and_poses() {
    let mut s = Session::new();
    let cam = MapView::default()
        .floats("position", &[1.0, 2.0, 3.0])
        .floats("rotation", &[0.25, 0.0, 0.5]);
    s.add(1, TgeClass::Camera, cam);
    s.step(0);
    let g = s.ghost(1);
    assert_eq!(
        s.world.ghosts[&g].pose(),
        Some(Pose::Euler {
            position: [1.0, 2.0, 3.0],
            pitch: 0.25,
            yaw: 0.5,
        })
    );
    s.view(1).set("position", 0, Value::F32(4.0));
    s.mgr.set_mask(1, ghosts::shapes::CAMERA_MOVE_MASK);
    s.step(100);
    let (_, index, m) = *s.world.camera_moves.back().unwrap();
    assert_eq!((index, m), (g, [4.0, 2.0, 3.0, 0.25, 0.5]));
}

#[test]
fn an_item_a_static_shape_and_a_tsstatic_pose_from_their_records() {
    let mut s = Session::new();
    s.add(
        1,
        TgeClass::Item,
        MapView::default()
            .floats("position", &[5.0, 6.0, 7.0])
            .floats("scale", &[1.0, 1.0, 1.0])
            .with("rotate", 0, Value::Bool(true)),
    );
    s.add(
        2,
        TgeClass::StaticShape,
        MapView::default()
            .with(
                "transform",
                0,
                Value::Xform(Some([
                    1.0f32.to_bits(),
                    2.0f32.to_bits(),
                    3.0f32.to_bits(),
                    0,
                    0,
                    0,
                    0,
                ])),
            )
            .floats("scale", &[2.0, 2.0, 2.0]),
    );
    let mut matrix = [0.0; 16];
    for i in [0, 5, 10, 15] {
        matrix[i] = 1.0;
    }
    matrix[3] = 9.0;
    s.add(
        3,
        TgeClass::TsStatic,
        MapView::default()
            .floats("matrix", &matrix)
            .floats("scale", &[1.0, 1.0, 1.0])
            .with("shape", 0, Value::Str(b"shapes/rock.dts".to_vec())),
    );
    s.step(0);
    let census = s.world.census();
    assert_eq!(
        (census["Item"], census["StaticShape"], census["TSStatic"]),
        (1, 1, 1)
    );
    let Some(Pose::Item { position, .. }) = s.world.ghosts[&s.ghost(1)].pose() else {
        panic!("item");
    };
    assert_eq!(position, [5.0, 6.0, 7.0]);
    let Some(Pose::Affine { xf, scale }) = s.world.ghosts[&s.ghost(2)].pose() else {
        panic!("static shape");
    };
    assert_eq!(
        (xf.position.map(|f| f.get()), scale),
        ([1.0, 2.0, 3.0], Some([2.0; 3]))
    );
    let Some(Pose::Matrix { m, .. }) = s.world.ghosts[&s.ghost(3)].pose() else {
        panic!("tsstatic");
    };
    assert_eq!(m, matrix);
    // A delete: the ghost and its log entries go.
    s.host.objs.remove(&3);
    s.mgr.detach_object(3);
    s.step(100);
    assert_eq!(s.world.ghosts.len(), 2);
    assert!(
        s.world
            .effects_after(0)
            .any(|e| matches!(e.effect, Effect::GhostGone { .. }))
    );
}

#[test]
fn a_projectile_that_exploded_brings_its_explosion_once() {
    let mut s = Session::new();
    s.world.apply_datablock(
        80,
        TgeDatablock {
            class: "ProjectileData".to_string(),
            fields: vec![("explosion".to_string(), 0, Value::Id(Some(81)))],
        },
    );
    s.add(
        1,
        TgeClass::Projectile,
        MapView::default()
            .with("datablock", 0, Value::Id(Some(80)))
            .floats("position", &[1.0, 1.0, 1.0])
            .floats("velocity", &[0.0, 10.0, 0.0])
            .with("hidden", 0, Value::Bool(true))
            .floats("explosion_position", &[1.0, 2.0, 0.0])
            .floats("explosion_normal", &[0.0, 0.0, 1.0]),
    );
    s.step(0);
    let g = s.ghost(1);
    assert!((s.world.ghosts[&g].position.unwrap()[1] - 1.0).abs() < 0.01);
    let explosions = |s: &Session| {
        s.world
            .effects_after(0)
            .filter(|e| {
                e.effect
                    == Effect::Explosion {
                        ghost: g,
                        datablock: 81,
                        position: [1.0, 2.0, 0.0],
                        normal: [0.0, 0.0, 1.0],
                    }
            })
            .count()
    };
    assert_eq!(explosions(&s), 1);
    s.mgr.set_mask(1, ghosts::projectile::EXPLOSION_MASK);
    s.step(32);
    assert_eq!(explosions(&s), 1, "one explosion per ghost");
}

/// Hostile ghost sequences, not just hostile bytes. Structured records first: a server whose objects are of random TGE
/// classes, with random datablocks (the image's among them), image fire counts, positions (huge and NaN too) and mount links
/// to random objects (itself, gone ones), added, changed and removed at random, writes real records. Then sections written
/// by hand: random ghost indexes, delete flags and class ids (past the table too) over random record bits. Both are fed to
/// a fresh client in a seeded order (replayed, dropped, reordered, so deletes come before creates and links and mounts name
/// ghosts the client never had), some with bits flipped, cut short or spliced. The client may refuse a packet; nothing
/// panics, and the world follows only what was read. `TORQUE_FUZZ_GHOSTS` sets the number of packets fed (default 3,000).
#[test]
fn hostile_ghost_sequences_never_panic_the_client_or_its_world() {
    use net::testrng::TestRng;
    let cases: u64 = std::env::var("TORQUE_FUZZ_GHOSTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3_000);
    let mut rng = TestRng::new(0x9e37_79b9_7f4a_7c15);
    // The classes a server of TGE's table ghosts (the engine's own and plugins' are not in it).
    let classes: Vec<TgeClass> = TgeClass::ALL
        .into_iter()
        .filter(|c| !matches!(c, TgeClass::Plugin(_)))
        .collect();
    let mut s = Session::new();
    s.world.apply_datablock(70, image_datablock());
    let mut packets = Vec::new();
    let mut next_id: ObjId = 1;
    for tick in 0..96u32 {
        for _ in 0..1 + rng.index(4) {
            match rng.below(4) {
                0 | 1 => {
                    let class = classes[rng.index(classes.len())];
                    let view = random_view(&mut rng, next_id);
                    s.add(next_id, class, view);
                    next_id += 1;
                }
                2 if next_id > 1 => {
                    let id = 1 + rng.below(u64::from(next_id - 1)) as ObjId;
                    s.host.objs.remove(&id);
                }
                _ if next_id > 1 => {
                    let id = 1 + rng.below(u64::from(next_id - 1)) as ObjId;
                    if let Some(o) = s.host.objs.get_mut(&id) {
                        o.view = random_view(&mut rng, next_id);
                    }
                }
                _ => {}
            }
        }
        s.seq += 1;
        s.host.now = tick * 32;
        let mut w = BitWriter::with_limit(12_000);
        w.enable_string_buffer();
        s.mgr.write_section(
            &mut w,
            8 * 1400,
            s.seq,
            tick * 32,
            &mut s.host,
            SectionBudget {
                scan: &mut |_| true,
                records: &mut |_| true,
            },
        );
        s.mgr.on_notify(s.seq, true);
        packets.push((w.bits(), w.into_bytes()));
    }
    // Sections by hand: random indexes, deletes and class ids over random record bits.
    let class_bits = s.host.classes.id_bits(Kind::Object);
    for _ in 0..32 {
        let mut w = BitWriter::with_limit(12_000);
        w.write_flag(true);
        let id_bits = 3 + rng.below(13) as u8;
        w.write_bits(u32::from(id_bits - 3), 5);
        for _ in 0..1 + rng.index(8) {
            w.write_flag(true);
            w.write_bits(rng.below(1 << id_bits) as u32, id_bits);
            let delete = rng.below(4) == 0;
            w.write_flag(delete);
            if !delete {
                w.write_bits(rng.below(1 << class_bits) as u32, class_bits);
                for _ in 0..rng.index(64) {
                    w.write_bits(rng.next_u64() as u32, 32);
                }
            }
        }
        w.write_flag(false);
        packets.push((w.bits(), w.into_bytes()));
    }
    // A fresh client, fed in a hostile order.
    let fresh = Session::new();
    let (mut client, mut world) = (fresh.client, fresh.world);
    world.apply_datablock(70, image_datablock());
    let (mut read, mut refused) = (0u64, 0u64);
    for case in 0..cases {
        let (bits, bytes) = &packets[rng.index(packets.len())];
        let mut bytes = bytes.clone();
        let mut bits = *bits;
        match rng.below(5) {
            0 | 1 => {}
            2 => {
                for _ in 0..1 + rng.below(4) {
                    let at = rng.index(bytes.len());
                    if let Some(b) = bytes.get_mut(at) {
                        *b ^= 1 << rng.below(8);
                    }
                }
            }
            3 => bits = rng.index(bits.max(1)),
            _ => {
                // The head of one packet and the tail of another.
                let (other_bits, other) = &packets[rng.index(packets.len())];
                let at = rng.index(bytes.len().min(other.len()));
                bytes.truncate(at);
                bytes.extend_from_slice(&other[at..]);
                bits = *other_bits;
            }
        }
        let bits = bits.min(bytes.len() * 8);
        let mut r = BitReader::with_bit_len(&bytes, bits);
        r.enable_string_buffer();
        client
            .codec_mut()
            .set_budget(Some(ghosts::datagram_budget(bytes.len())));
        match client.read_section(&mut r) {
            Ok(_) => read += 1,
            Err(_) => refused += 1,
        }
        let changes = client.take_log();
        world.set_clock(case as u32 * 32);
        world.apply_ghosts(case as usize, &changes, &client, [0.0; 3], &mut ());
        // Now and then the server resets ghosting, and the client forgets its table.
        if rng.below(200) == 0 {
            client.clear();
            let changes = client.take_log();
            world.apply_ghosts(case as usize, &changes, &client, [0.0; 3], &mut ());
        }
    }
    assert!(read > 0 && refused > 0, "{read} read, {refused} refused");
}

/// An object's fields at random: datablocks (the image's among them), an image with a fire count, a position that may be
/// huge or NaN, a scale, a thread, and a mount link to a random object of the `past` made so far (itself and gone ones too).
fn random_view(rng: &mut net::testrng::TestRng, past: ObjId) -> MapView {
    let f = |rng: &mut net::testrng::TestRng| match rng.below(6) {
        0 => f32::NAN,
        1 => 1.0e30,
        _ => (rng.below(2_000) as f32) - 1_000.0,
    };
    let mut view = MapView::default()
        .with("datablock", 0, Value::Id(Some(rng.below(100) as u32)))
        .with(
            "image_datablock",
            0,
            Value::Id(Some(if rng.below(2) == 0 {
                70
            } else {
                rng.below(100) as u32
            })),
        )
        .with("image_loaded", 0, Value::Bool(rng.below(2) == 0))
        .with("image_fire_count", 0, Value::Int(rng.below(16) as i64))
        .with("thread_sequence", 0, Value::Int(rng.below(8) as i64 - 1))
        .floats("position", &[f(rng), f(rng), f(rng)])
        .floats("rotation", &[0.0, 0.0, 1.0])
        .floats("scale", &[1.0, f(rng), 1.0]);
    if rng.below(2) == 0 {
        view.links
            .insert("mount".into(), 1 + rng.below(u64::from(past)) as ObjId);
    }
    view
}
