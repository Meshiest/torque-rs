//! A plugin's ghost class drawn as a shape, end to end: a server makes a crate as a StaticShape and, in another run, as a plugin's
//! `Prop`; the client of an engine with the plugin draws each by the engine's own ghost path. The two pictures are the same
//! pixels, the frame's list of ghosts holds the plugin's ghost (so a pass can anchor to it), and a moved `Prop` is blended from where
//! it was drawn. `TORQUE_DRAWN_SHAPES_PICTURES=<folder>` also writes the pictures there as PNGs.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use torque_app::GameFiles;
use torque_app::client::{ClientBuilder, ClientNet, GameClient};
use torque_app::config::{EngineConfig, GameConfig};
use torque_app::ghosts::{GhostOptions, GhostScene};
use torque_app::plugins::DefaultPlugins;
use torque_app::render::{EngineRender, Stage};
use torque_app::renderers::{RenderFeatures, RendererEntry, RendererSlot};
use torque_app::server::{GameServer, Listen, ServerStart};
use torque_app::shots::write_png;
use torque_app::view::{Daylight, WorldView};
use torque_client::net::{DatagramQueue, MemoryDatagrams, NetPolicy};
use torque_engine::net_classes::{DatablockRef, ShapeDraw, Xf};
use torque_engine::{
    DatablockCodec, Engine, EngineBuilder, EngineClass, GhostRecord, Plugin, PluginId, PluginType,
    Replicate,
};
use torque_render_core::{Frame, Gpu};
use torque_render_scene::Assets;
use torque_render_scene::hd::DynamicObject;
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::moving::ShapeSelect;
use torque_render_scene::source::Eye;

const SIZE: (u32, u32) = (960, 540);
/// Frames each picture settles for (the anti-aliasing's history).
const FRAMES: u32 = 10;
const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);
const CRATE: &str = "art/crate.dts";

// ---- the plugin: a `Prop` is a shape its datablock names

/// What a client knows of a prop: where it is and which datablock it is.
#[derive(Debug, Clone, PartialEq, Default, GhostRecord)]
struct PropGhost {
    #[net(name = "transform")]
    pose: Xf,
    data_block: DatablockRef,
}

/// A prop's datablock: the shape file it is drawn as, and whether its props rest ("1").
#[derive(Debug, Clone, PartialEq, Default, EngineClass, DatablockCodec)]
#[class(name = "PropData", parent = "SimDataBlock")]
struct PropData {
    #[net(max_len = 64)]
    model: String,
    #[net(max_len = 4)]
    rests: String,
}

/// The server's prop: the object the ghost is packed from.
#[allow(dead_code)]
#[derive(EngineClass)]
#[class(name = "Prop", parent = "SceneObject")]
struct Prop {
    data_block: i32,
}

struct Props;

impl PluginType for Props {
    const ID: PluginId = PluginId::new("test.props");
}

impl Plugin for Props {
    fn net_version(&self) -> Option<u32> {
        Some(1)
    }

    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .net()
            .ghost::<PropGhost>("Prop", Replicate::Interpolated)
            .datablock::<PropData>("PropData")
            .drawn_shape::<PropGhost>("Prop", |prop, blocks| {
                let id = prop.data_block.0?;
                let model = blocks.text(id, "model")?;
                Some(ShapeDraw::new(model).resting(blocks.text(id, "rests") == Some("1")))
            });
        engine.server_script().class::<PropData>().class::<Prop>();
        engine.client_script().class::<PropData>().class::<Prop>();
    }
}

// ---- a server and a client over memory

fn files() -> GameFiles {
    let mut files = GameFiles::default();
    files.insert("server.cs", include_bytes!("drawn_shapes/server.cs"));
    files.insert("main.cs", include_bytes!("drawn_shapes/main.cs"));
    files.insert("game.toml", include_bytes!("drawn_shapes/game.toml"));
    // A box 2 across.
    let b = [-1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
    files.insert(
        CRATE,
        &torque_shapes::synth::solid_dts(b, &[("detail32", b)], [0, 0, 0, 32767], [0.0; 3], false),
    );
    files
}

fn engine(files: &GameFiles, seen: Option<Arc<Mutex<Vec<u32>>>>) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins((DefaultPlugins, Props)).config(&config);
    if let Some(seen) = seen {
        // A pass that draws nothing and keeps the ghosts the frame listed, as a rope's pass finds the one it hangs from.
        builder.render().pass(Stage::Opaque, move || {
            let seen = Arc::clone(&seen);
            move |cx: &mut torque_app::render::PassCx<'_>| {
                *seen.lock().unwrap() = cx.ghosts.iter().map(|o| o.ghost).collect();
            }
        });
    }
    builder.build().expect("the engine builds")
}

struct Session {
    server: GameServer,
    client: GameClient,
    engine: Engine,
    addr: SocketAddrV4,
    sent: DatagramQueue,
    incoming: DatagramQueue,
}

impl Session {
    fn new(files: &GameFiles, seen: Option<Arc<Mutex<Vec<u32>>>>) -> Session {
        let engine = engine(files, seen);
        let server = GameServer::start_engine(
            &engine,
            files.mem_files(),
            &ServerStart {
                listen: Listen::Memory(SERVER),
                globals: &[],
                start: "startProps",
                args: &[],
            },
        )
        .expect("the server starts");
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 50_001);
        let mem = MemoryDatagrams {
            local_port: 50_001,
            ..MemoryDatagrams::default()
        };
        let (sent, incoming) = (mem.sent.clone(), mem.incoming.clone());
        let mut builder =
            ClientBuilder::from_engine(&engine, Some(files.mem_files()), ClientNet::External)
                .expect("the client builds");
        builder.set_socket(Box::new(mem), NetPolicy::Loopback);
        builder.exec_entry().expect("the client's script");
        let mut client: GameClient = builder.build();
        client
            .call("connectTo", &[&SERVER.to_string()])
            .expect("connect");
        let mut session = Session {
            server,
            client,
            engine,
            addr,
            sent,
            incoming,
        };
        assert!(
            session.run_until(400, |s| s.client.ghost_count() >= 1),
            "{:?}",
            session.client.client_log
        );
        session
    }

    fn carry(&mut self) {
        let Some(end) = self.server.memory_end() else {
            return;
        };
        for (to, data) in self.sent.borrow_mut().drain(..) {
            if SocketAddr::V4(to) == SERVER {
                end.inject(SocketAddr::V4(self.addr), &data);
            }
        }
        for (to, data) in end.take_sent() {
            if SocketAddr::V4(self.addr) == to {
                let SocketAddr::V4(from) = SERVER else {
                    continue;
                };
                self.incoming.borrow_mut().push_back((from, data));
            }
        }
    }

    /// One tick of the server, then of the client.
    fn step(&mut self) {
        self.carry();
        self.server.tick();
        self.carry();
        self.client.step(32);
        self.carry();
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.step();
        }
    }

    fn run_until(&mut self, limit: usize, mut done: impl FnMut(&Session) -> bool) -> bool {
        for _ in 0..limit {
            if done(self) {
                return true;
            }
            self.step();
        }
        done(self)
    }

    fn eval(&mut self, code: &str) -> String {
        self.server
            .vm_mut()
            .eval(code.as_bytes())
            .map(|v| String::from_utf8_lossy(&v).into_owned())
            .unwrap_or_else(|e| panic!("{code}: {e}"))
    }
}

// ---- the pictures

struct Kit(GameFiles);

impl Assets for Kit {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.file(path).map(<[u8]>::to_vec)
    }
}

fn gpu() -> Option<Gpu> {
    match torque_app::window::offscreen_gpu() {
        Ok(gpu) => Some(gpu),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn save(frame: &Frame, name: &str) {
    if let Some(dir) = std::env::var_os("TORQUE_DRAWN_SHAPES_PICTURES").map(PathBuf::from) {
        std::fs::create_dir_all(&dir).unwrap();
        write_png(frame, &dir.join(format!("{name}.png"))).unwrap();
    }
}

fn view(gpu: &Gpu, engine: &Engine) -> WorldView {
    let mut view = WorldView::new(gpu);
    view.set_plain_floor(120.0, [128, 150, 110]);
    view.set_daylight(&Daylight::default());
    view.set_effect_frame_seconds(Some(1.0 / 60.0));
    let features = engine
        .resource::<RenderFeatures>()
        .as_deref()
        .copied()
        .unwrap_or(RenderFeatures::ALL);
    let (slot, warnings) = RendererSlot::new(engine.list::<RendererEntry>(), features);
    assert!(warnings.is_empty(), "{warnings:?}");
    let adds = torque_app::render::RenderAdds::of_engine(engine).kept(|w| panic!("{w}"));
    view.attach_slot(Rc::new(slot.with_adds(adds)))
        .expect("the engine's renderer is made");
    view
}

fn eye() -> Eye {
    look_at([0.0, -12.0, 3.0], [0.0, 0.0, 1.5])
}

/// The ghosts the client draws, as the engine's ghost path makes them (every shape of the world, whether it moves or not).
fn objects(session: &Session, scene: &mut GhostScene, view: &mut WorldView) -> Vec<DynamicObject> {
    let kit = Kit(files());
    session
        .client
        .with_world(|w| scene.objects(view, &kit, &w.records, None))
        .expect("connected")
}

fn scene() -> GhostScene {
    GhostScene::new(GhostOptions {
        select: ShapeSelect::All,
        ..GhostOptions::default()
    })
}

fn shot(view: &mut WorldView, gpu: &Gpu, objects: Vec<DynamicObject>) -> Frame {
    view.reset_history();
    let mut frame = None;
    for _ in 0..FRAMES {
        view.set_objects(objects.clone());
        frame = Some(
            view.render_world(gpu, SIZE, eye())
                .expect("the world is drawn"),
        );
    }
    frame.unwrap()
}

/// The pixels the two pictures differ in, by the largest channel's difference over `threshold`.
fn differing(a: &Frame, b: &Frame, threshold: i32) -> usize {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.rgba
        .chunks(4)
        .zip(b.rgba.chunks(4))
        .filter(|(p, q)| {
            (0..3)
                .map(|c| (i32::from(p[c]) - i32::from(q[c])).abs())
                .max()
                .unwrap()
                > threshold
        })
        .count()
}

const AT: &str = "\"3 0 1 0 0 1 0.7\"";

#[test]
fn a_plugin_ghost_is_drawn_as_a_static_shape_at_the_same_pose_is_and_a_pass_finds_it() {
    let Some(gpu) = gpu() else { return };
    let files = files();
    let seen = Arc::new(Mutex::new(Vec::new()));
    // The crate as a plugin's Prop.
    let mut prop = Session::new(&files, Some(Arc::clone(&seen)));
    let id = prop.eval(&format!("return makeProp({AT});"));
    assert!(
        prop.run_until(200, |s| s.client.with_world(|w| {
            w.records
                .ghosts
                .values()
                .any(|g| g.class.is_some_and(|c| c.name() == "Prop"))
        }) == Some(true)),
        "the prop is ghosted (object {id})"
    );
    prop.run(8);
    let mut view_a = view(&gpu, &prop.engine);
    let mut scene_a = scene();
    let drawn = objects(&prop, &mut scene_a, &mut view_a);
    let index = prop
        .client
        .with_world(|w| {
            w.records
                .ghosts
                .iter()
                .find(|(_, g)| g.class.is_some_and(|c| c.name() == "Prop"))
                .map(|(i, _)| *i)
        })
        .flatten()
        .expect("a Prop ghost");
    assert_eq!(
        drawn.iter().filter(|o| o.ghost == index).count(),
        1,
        "the frame's ghosts hold the prop: {:?}",
        drawn.iter().map(|o| o.ghost).collect::<Vec<_>>()
    );
    let object = drawn.iter().find(|o| o.ghost == index).unwrap();
    assert!(!object.draws.is_empty() && object.radius > 1.0);
    assert!(
        (object.centre[0] - 3.0).abs() < 0.01 && (object.centre[2] - 1.0).abs() < 0.01,
        "{:?}",
        object.centre
    );
    let plugin_shot = shot(&mut view_a, &gpu, drawn);
    save(&plugin_shot, "prop");
    // The pass that draws the frame's ghosts saw it, so a rope can hang from it.
    assert!(seen.lock().unwrap().contains(&index), "{:?}", seen.lock());

    // The same crate as a StaticShape.
    let mut tge = Session::new(&files, None);
    tge.eval(&format!("return makeStatic({AT});"));
    assert!(tge.run_until(200, |s| s.client.with_world(|w| {
        w.records
            .ghosts
            .values()
            .any(|g| g.class.is_some_and(|c| c.name() == "StaticShape"))
    }) == Some(true)));
    tge.run(8);
    let mut view_b = view(&gpu, &tge.engine);
    let mut scene_b = scene();
    let drawn = objects(&tge, &mut scene_b, &mut view_b);
    assert!(drawn.iter().any(|o| !o.draws.is_empty()));
    let static_shot = shot(&mut view_b, &gpu, drawn);
    save(&static_shot, "static-shape");

    // The shape is in the picture, and the two pictures are the same.
    let floor = {
        let mut empty = view(&gpu, &tge.engine);
        shot(&mut empty, &gpu, Vec::new())
    };
    let crate_pixels = differing(&floor, &static_shot, 24);
    assert!(
        crate_pixels > 1500,
        "the crate covers {crate_pixels} pixels"
    );
    let apart = differing(&plugin_shot, &static_shot, 0);
    assert!(
        apart <= 32 && differing(&plugin_shot, &static_shot, 1) == 0,
        "{apart} pixels differ between the plugin ghost and the StaticShape"
    );
}

#[test]
fn a_moved_prop_is_shown_between_its_updates_and_ends_where_the_server_put_it() {
    let files = files();
    let mut session = Session::new(&files, None);
    let object = session.eval("return makeProp(\"-3 0 1 0 0 1 0\");");
    session.run_until(200, |s| {
        s.client.with_world(|w| {
            w.records
                .ghosts
                .values()
                .any(|g| g.class.is_some_and(|c| c.name() == "Prop"))
        }) == Some(true)
    });
    session.run(8);
    // The drawn x of the prop, read the way a frame reads it.
    let drawn_x = |session: &Session| {
        let kit = Kit(files.clone());
        session
            .client
            .with_world(|w| {
                use torque_render_scene::moving::{ShapeSelect, draws_culled};
                use torque_render_scene::textures::TextureSet;
                let mut textures = TextureSet::new(&kit);
                let out = draws_culled(
                    &w.records,
                    &(),
                    &kit,
                    &mut textures,
                    &mut Vec::new(),
                    torque_render_scene::shapes::DetailPolicy::Highest,
                    ShapeSelect::All,
                    None,
                );
                out.transforms.values().map(|m| m.column(3)[0]).next()
            })
            .flatten()
    };
    let at_rest = drawn_x(&session).expect("drawn");
    assert!((at_rest + 3.0).abs() < 0.01, "{at_rest}");
    // The server moves it 6 m at once, a while after the last frame: the client draws it on the way for three ticks, not there at
    // once, and then there.
    session.run(4);
    session.eval(&format!("moveProp({object}, \"3 0 1 0 0 1 0\");"));
    let mut xs = Vec::new();
    for _ in 0..8 {
        session.step();
        xs.extend(drawn_x(&session));
    }
    assert!(
        xs.iter().any(|x| *x > -2.9 && *x < 2.9),
        "never between the two places: {xs:?}"
    );
    assert!(xs.windows(2).all(|w| w[1] >= w[0] - 1e-4), "{xs:?}");
    session.run(20);
    let last = drawn_x(&session).expect("drawn");
    assert!((last - 3.0).abs() < 0.01, "{last}");
}

#[test]
fn a_cut_to_a_far_view_draws_its_ghosts_on_that_frame() {
    let Some(gpu) = gpu() else { return };
    let game = files();
    let mut session = Session::new(&game, None);
    session.eval(&format!("return makeStatic({AT});"));
    assert!(session.run_until(200, |s| s.client.with_world(|w| {
        w.records
            .ghosts
            .values()
            .any(|g| g.class.is_some_and(|c| c.name() == "StaticShape"))
    }) == Some(true)));
    session.run(8);
    let mut view = view(&gpu, &session.engine);
    let mut scene = scene();
    let kit = Kit(files());
    let mut drawn_from = |view: &mut WorldView, eye: Eye| {
        session
            .client
            .with_world(|w| scene.objects(view, &kit, &w.records, Some(&eye)))
            .expect("connected")
            .iter()
            .any(|o| !o.draws.is_empty())
    };
    // Past the visible distance behind the crate, looking away from it, its shadow falling away from the view.
    let away = look_at([0.0, -260.0, 3.0], [0.0, -400.0, 1.0]);
    assert!(drawn_from(&mut view, away), "the first frame is not culled");
    view.set_objects(Vec::new());
    view.render_world(&gpu, SIZE, away)
        .expect("the world is drawn");
    let mut step = away;
    step.position[1] -= 0.5;
    assert!(!drawn_from(&mut view, step), "out of the view: left out");
    // A cut to a far view of the crate draws it on the cut's frame.
    assert!(drawn_from(&mut view, eye()), "a cut to a view of it");
    // A cut builds every ghost, even ones out of its view.
    let elsewhere = look_at([0.0, -600.0, 3.0], [0.0, -800.0, 1.0]);
    assert!(drawn_from(&mut view, elsewhere), "a cut is not culled");
    assert_eq!(gpu.device_errors().validation, 0);
}

/// A resting plugin ghost is drawn as a still copy of its shape's static batch (the frame's moving shapes leave it out) and
/// looks as the same ghost does drawn every frame; once it stops resting, its batch goes and it is a moving shape again; and
/// one whose batch the view's budget would cut short is drawn every frame instead.
#[test]
fn a_resting_plugin_ghost_is_a_still_copy_that_looks_as_the_moving_one() {
    let Some(gpu) = gpu() else { return };
    let files = files();
    let mut session = Session::new(&files, None);
    let mut view = view(&gpu, &session.engine);
    let mut scene = GhostScene::new(GhostOptions::default());
    let place = |session: &mut Session, data: &str| {
        session.eval(&format!(
            "if (isObject($Prop)) $Prop.delete(); $Prop = makeProp({AT}, {data});"
        ));
        session.run(30);
    };
    place(&mut session, "CrateProp");
    let moving = objects(&session, &mut scene, &mut view);
    assert_eq!((moving.len(), scene.resting_count()), (1, 0));
    assert_eq!(view.static_batch_triangles(), 0);
    let a = shot(&mut view, &gpu, moving);
    save(&a, "moving");
    place(&mut session, "CrateResting");
    let still = objects(&session, &mut scene, &mut view);
    assert_eq!((still.len(), scene.resting_count()), (0, 1));
    assert!(view.static_batch_triangles() > 0);
    let b = shot(&mut view, &gpu, still);
    save(&b, "resting");
    let off = differing(&a, &b, 8);
    assert!(off < 40, "the resting copy differs in {off} pixels");
    place(&mut session, "CrateProp");
    let again = objects(&session, &mut scene, &mut view);
    assert_eq!((again.len(), scene.resting_count()), (1, 0));
    assert_eq!(view.static_batch_triangles(), 0, "its batch went");
    // A batch the view's triangle budget would cut short: the resting crate is drawn every frame instead, never left out.
    view.set_static_batch_budget(1);
    place(&mut session, "CrateResting");
    let short = objects(&session, &mut scene, &mut view);
    assert_eq!((short.len(), scene.resting_count()), (1, 0));
    assert_eq!(view.static_batch_triangles(), 0);
}
