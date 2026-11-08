//! GUI control classes that plugins add: several plugins each add controls, a name taken twice or one of the engine's own is a
//! build error, a control of a plugin's class draws, lays itself out and takes the mouse and the keys by its class, and the
//! engine's own controls are left as they were. The picture test draws a row of tiles offscreen (skipped with a note when the
//! machine has no GPU); `TORQUE_GUI_PICTURES=<folder>` also writes the picture there as a PNG.

use std::path::PathBuf;

use torque_app::plugins::DefaultPlugins;
use torque_app::{ClientBuilder, ClientNet, GameClient, GameServer};
use torque_client::{Key, MouseButton};
use torque_engine::{BuildError, Engine, EngineBuilder, EngineClass, Plugin, PluginId};
use torque_gui::custom::{
    ControlCx, GuiBuilderExt, GuiControlClass, KeyEvent, Place, PointerEvent,
};
use torque_gui::draw::DrawCtx;
use torque_render_core::GuiPass;
use torque_ts_vm::vm::VmError;

/// A block of one color. A click tells the script (`onClick(count)`), and every click so far is a bar along its top.
#[derive(EngineClass)]
#[class(name = "GuiTileCtrl", parent = "GuiControl")]
struct Tile {
    /// The fill.
    #[field(default = "255 128 0 255")]
    color: [u8; 4],
}

#[derive(Default)]
struct TileState {
    clicks: u32,
    hovered: bool,
}

impl GuiControlClass for Tile {
    type State = TileState;

    fn draw(&self, state: &mut TileState, d: &mut DrawCtx<'_, '_>, at: &Place) {
        d.fill(at.rect(), self.color);
        if state.hovered {
            let [x, y, w, h] = at.rect();
            d.outline((x, y), (x + w - 1, y + h - 1), [255, 255, 255, 255]);
        }
        for bar in 0..state.clicks.min(8) as i32 {
            d.fill(
                [at.offset.0 + 3 + bar * 6, at.offset.1 + 3, 4, 4],
                [0, 0, 0, 255],
            );
        }
    }

    fn mouse_down(
        &self,
        state: &mut TileState,
        cx: &mut ControlCx<'_>,
        _ev: &PointerEvent,
    ) -> Result<(), VmError> {
        state.clicks += 1;
        cx.call("onClick", &[state.clicks.to_string().into_bytes()])?;
        Ok(())
    }

    fn mouse_enter(
        &self,
        state: &mut TileState,
        _cx: &mut ControlCx<'_>,
        _ev: &PointerEvent,
    ) -> Result<(), VmError> {
        state.hovered = true;
        Ok(())
    }

    fn mouse_leave(
        &self,
        state: &mut TileState,
        _cx: &mut ControlCx<'_>,
        _ev: &PointerEvent,
    ) -> Result<(), VmError> {
        state.hovered = false;
        Ok(())
    }
}

#[derive(PluginId)]
#[plugin(id = "test.tiles")]
struct TilePlugin;

impl Plugin for TilePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_gui_control::<Tile>();
    }
}

/// Writes down everything done to it in `$log`, as `name:detail`, and takes the mouse on a press.
#[derive(EngineClass)]
#[class(name = "GuiRecorderCtrl", parent = "GuiControl")]
struct Recorder {
    /// Whether a press takes the mouse for the control.
    capture: bool,
}

fn note(cx: &mut ControlCx<'_>, line: &str) {
    let mut log = cx.vm.global("$log").unwrap_or_default();
    log.extend_from_slice(line.as_bytes());
    log.push(b' ');
    cx.vm.set_global("$log", &String::from_utf8_lossy(&log));
}

impl GuiControlClass for Recorder {
    type State = ();

    fn resize(&self, _: &mut (), cx: &mut ControlCx<'_>, position: (i32, i32), extent: (i32, i32)) {
        note(cx, &format!("resize:{}x{}", extent.0, extent.1));
        cx.base_resize(position, extent);
    }

    fn wake(&self, _: &mut (), cx: &mut ControlCx<'_>) -> Result<(), VmError> {
        note(cx, "wake");
        Ok(())
    }

    fn sleep(&self, _: &mut (), cx: &mut ControlCx<'_>) -> Result<(), VmError> {
        note(cx, "sleep");
        Ok(())
    }

    fn mouse_down(
        &self,
        _: &mut (),
        cx: &mut ControlCx<'_>,
        ev: &PointerEvent,
    ) -> Result<(), VmError> {
        note(
            cx,
            &format!("down:{},{}x{}", ev.local.0, ev.local.1, ev.clicks),
        );
        if self.capture {
            cx.lock_mouse()?;
        }
        Ok(())
    }

    fn mouse_up(
        &self,
        _: &mut (),
        cx: &mut ControlCx<'_>,
        _ev: &PointerEvent,
    ) -> Result<(), VmError> {
        note(cx, "up");
        cx.unlock_mouse()
    }

    fn mouse_dragged(
        &self,
        _: &mut (),
        cx: &mut ControlCx<'_>,
        ev: &PointerEvent,
    ) -> Result<(), VmError> {
        note(cx, &format!("drag:{},{}", ev.local.0, ev.local.1));
        Ok(())
    }

    fn key_down(&self, _: &mut (), cx: &mut ControlCx<'_>, ev: &KeyEvent) -> Result<bool, VmError> {
        note(cx, &format!("key:{}", ev.key_code));
        Ok(true)
    }
}

/// A second plugin with a control below the first plugin's class.
#[derive(EngineClass)]
#[class(name = "GuiBarCtrl", parent = "GuiTileCtrl")]
struct Bar {
    thickness: i32,
}

impl GuiControlClass for Bar {
    type State = ();
}

#[derive(PluginId)]
#[plugin(id = "test.recorder")]
struct RecorderPlugin;

impl Plugin for RecorderPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .add_gui_control::<Recorder>()
            .add_gui_control::<Bar>();
    }
}

fn engine() -> Engine {
    EngineBuilder::new()
        .add_plugins((DefaultPlugins, TilePlugin, RecorderPlugin))
        .build()
        .expect("the engine builds")
}

/// A client with a canvas of `size` and a `Content` of 640 x 480 holding `script`'s controls.
fn client(size: (i32, i32), script: &str) -> GameClient<GameServer> {
    let mut builder =
        ClientBuilder::from_engine(&engine(), None, ClientNet::None).expect("the client");
    builder.gui.force_canvas_size(Some(size));
    builder
        .vm
        .eval(
            b"createCanvas(\"test\");\
              new GuiControlProfile(GuiDefaultProfile) { };              new GuiControlProfile(PanelProfile) { opaque = 1; fillColor = \"40 44 60 255\"; };\
              new GuiControl(Content) { profile = \"PanelProfile\"; extent = \"640 480\"; };\
              Canvas.setContent(Content);",
        )
        .expect("the canvas");
    builder.vm.eval(script.as_bytes()).expect("the controls");
    let mut client = builder.build::<GameServer>();
    client.host.run(100, |_| false);
    client
}

fn id(client: &GameClient<GameServer>, name: &str) -> torque_ts_vm::sim::ObjId {
    client
        .vm()
        .objects()
        .find(name.as_bytes())
        .unwrap_or_else(|| panic!("no object {name}"))
}

fn log(client: &GameClient<GameServer>) -> String {
    String::from_utf8_lossy(&client.vm().global("$log").unwrap_or_default()).into_owned()
}

fn click(client: &mut GameClient<GameServer>, at: (i32, i32)) {
    client.host.mouse_move(at);
    client.host.mouse_button(MouseButton::Left, true, at);
    client.host.mouse_button(MouseButton::Left, false, at);
}

fn build_error(plugins: impl FnOnce(&mut EngineBuilder)) -> BuildError {
    let mut builder = EngineBuilder::new();
    plugins(&mut builder);
    builder.build().expect_err("the build is refused")
}

#[test]
fn every_plugin_adds_its_controls_and_a_client_has_them_all() {
    let engine = engine();
    let classes: Vec<_> = engine.classes().map(|c| c.name).collect();
    assert_eq!(classes, ["GuiTileCtrl", "GuiRecorderCtrl", "GuiBarCtrl"]);
    let mut client = client(
        (320, 200),
        "new GuiTileCtrl(S) { color = \"1 2 3 255\"; };\
         new GuiBarCtrl(B) { thickness = 4; };\
         new GuiRecorderCtrl(P) { capture = 1; };\
         Content.add(S); Content.add(B); Content.add(P);",
    );
    let (s, b) = (id(&client, "S"), id(&client, "B"));
    for control in ["S", "B", "P"] {
        assert!(client.host.gui.is_custom(id(&client, control)), "{control}");
        assert!(client.host.gui.is_awake(id(&client, control)), "{control}");
    }
    // The fields are the class's fields, written and read by the console's rules.
    let tile = <Tile as torque_engine::ScriptClass>::read(client.vm(), s).expect("a tile");
    assert_eq!(tile.color, [1, 2, 3, 255]);
    let bar = <Bar as torque_engine::ScriptClass>::read(client.vm(), b).expect("a bar");
    assert_eq!(bar.thickness, 4);
    let fresh = client
        .vm_mut()
        .eval(b"return (new GuiTileCtrl()).color;")
        .unwrap();
    assert_eq!(fresh, b"255 128 0 255", "the default");
    // Active, so they take the mouse; the engine's plain GuiControl does not.
    assert!(client.host.gui.with(s, |c| c.active).unwrap());
    assert!(
        !client
            .host
            .gui
            .with(id(&client, "Content"), |c| c.active)
            .unwrap()
    );
}

#[test]
fn a_class_two_plugins_both_add_fails_the_build_naming_both() {
    #[derive(EngineClass)]
    #[class(name = "guitilectrl", parent = "GuiControl")]
    struct Rival {
        #[allow(dead_code)]
        weight: i32,
    }
    impl GuiControlClass for Rival {
        type State = ();
    }
    #[derive(PluginId)]
    #[plugin(id = "test.rival")]
    struct RivalPlugin;
    impl Plugin for RivalPlugin {
        fn build(&self, engine: &mut EngineBuilder) {
            engine.add_gui_control::<Rival>();
        }
    }
    let error = build_error(|e| {
        e.add_plugins((TilePlugin, RivalPlugin));
    });
    let BuildError::DuplicateScriptName { first, second, .. } = error else {
        panic!("{error:?}");
    };
    assert_eq!(
        (first.as_str(), second.as_str()),
        ("test.tiles", "test.rival")
    );
    let message = error.to_string();
    assert!(
        message.contains("test.tiles") && message.contains("test.rival"),
        "{message}"
    );
}

#[test]
fn an_engine_class_name_cannot_be_taken_over() {
    #[derive(EngineClass)]
    #[class(name = "GuiButtonCtrl", parent = "GuiControl")]
    struct Button {
        #[allow(dead_code)]
        weight: i32,
    }
    impl GuiControlClass for Button {
        type State = ();
    }
    #[derive(EngineClass)]
    #[class(name = "guicontrol", parent = "GuiControl")]
    struct Plain {
        #[allow(dead_code)]
        weight: i32,
    }
    impl GuiControlClass for Plain {
        type State = ();
    }
    #[derive(PluginId)]
    #[plugin(id = "test.taker")]
    struct TakerPlugin(bool);
    impl Plugin for TakerPlugin {
        fn build(&self, engine: &mut EngineBuilder) {
            if self.0 {
                engine.add_gui_control::<Button>();
            } else {
                engine.add_gui_control::<Plain>();
            }
        }
    }
    for (plain, name) in [(true, "GuiButtonCtrl"), (false, "guicontrol")] {
        let error = build_error(|e| {
            e.add_plugins(TakerPlugin(plain));
        });
        let BuildError::Rejected { plugin, message } = &error else {
            panic!("{error:?}");
        };
        assert_eq!(plugin.as_str(), "test.taker");
        assert!(message.contains(name), "{message}");
    }
}

#[test]
fn a_parent_that_is_not_a_control_class_is_refused() {
    #[derive(EngineClass)]
    #[class(name = "GuiFancyButton", parent = "GuiButtonCtrl")]
    struct Fancy {
        #[allow(dead_code)]
        weight: i32,
    }
    impl GuiControlClass for Fancy {
        type State = ();
    }
    #[derive(EngineClass)]
    #[class(name = "GuiEarly", parent = "GuiLate")]
    struct Early {
        #[allow(dead_code)]
        weight: i32,
    }
    impl GuiControlClass for Early {
        type State = ();
    }
    for refused in [0, 1] {
        #[derive(PluginId)]
        #[plugin(id = "test.fancy")]
        struct FancyPlugin(u8);
        impl Plugin for FancyPlugin {
            fn build(&self, engine: &mut EngineBuilder) {
                match self.0 {
                    0 => engine.add_gui_control::<Fancy>(),
                    _ => engine.add_gui_control::<Early>(),
                };
            }
        }
        let error = build_error(|e| {
            e.add_plugins(FancyPlugin(refused));
        });
        assert!(
            matches!(&error, BuildError::Rejected { plugin, .. } if plugin.as_str() == "test.fancy"),
            "{error:?}"
        );
    }
}

#[test]
fn a_click_runs_the_class_and_calls_back_into_script() {
    let mut client = client(
        (320, 200),
        "function GuiTileCtrl::onClick(%this, %n) { $clicked = $clicked @ %this.getName() @ %n @ \";\"; }\
         Content.add(new GuiTileCtrl(A) { position = \"10 10\"; extent = \"50 50\"; });\
         Content.add(new GuiTileCtrl(B) { position = \"100 10\"; extent = \"50 50\"; });",
    );
    click(&mut client, (20, 20));
    click(&mut client, (110, 30));
    click(&mut client, (20, 20));
    click(&mut client, (200, 150)); // on the content: nothing
    assert_eq!(
        client.vm().global("$clicked").unwrap(),
        b"A1;B1;A2;".to_vec()
    );
}

#[test]
fn the_class_is_laid_out_woken_slept_and_given_the_mouse_and_the_keys() {
    let mut client = client(
        (320, 200),
        "new GuiControlProfile(KeyProfile) { canKeyFocus = 1; };         Content.add(new GuiRecorderCtrl(P) { profile = \"KeyProfile\"; capture = 1; position = \"40 40\"; extent = \"100 80\"; });",
    );
    let log_now = log(&client);
    assert!(log_now.contains("wake "), "{log_now}");
    // The canvas laid the content out to its size (640 wide to 320), which told the recorder its parent's size changed.
    assert!(log_now.contains("resize:100x80"), "{log_now}");
    let p = id(&client, "P");
    // A press takes the mouse: the drag is the recorder's though the pointer is outside it, with the pointer from its corner.
    client.vm_mut().set_global("$log", "");
    client.host.mouse_move((50, 50));
    client.host.mouse_button(MouseButton::Left, true, (50, 50));
    client.host.mouse_move((300, 190));
    client
        .host
        .mouse_button(MouseButton::Left, false, (300, 190));
    // The window's move is an X move then a Y move, each a drag.
    assert_eq!(log(&client), "down:10,10x1 drag:260,10 drag:260,150 up ");
    // The key goes to the control with the key focus, which takes it.
    client.vm_mut().set_global("$log", "");
    let gui = client.host.gui.clone();
    torque_gui::input::set_first_responder(client.vm_mut(), &gui, p).unwrap();
    client.host.key(Key::A, true);
    assert!(log(&client).starts_with("key:"), "{}", log(&client));
    // Deleted, it sleeps and its state is dropped.
    client.vm_mut().set_global("$log", "");
    client.vm_mut().eval(b"P.delete();").unwrap();
    assert_eq!(log(&client), "sleep ");
    assert!(!client.host.gui.is_custom(p));
}

fn folder() -> Option<PathBuf> {
    std::env::var_os("TORQUE_GUI_PICTURES").map(PathBuf::from)
}

fn pixel(frame: &torque_render_core::Frame, x: u32, y: u32) -> [u8; 3] {
    let i = 4 * (y * frame.width + x) as usize;
    [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
}

#[test]
fn a_row_of_tiles_is_drawn_with_a_hover_and_a_count_of_clicks() {
    let gpu = match torque_app::window::offscreen_gpu() {
        Ok(gpu) => gpu,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let mut client = client(
        (320, 200),
        "new GuiControlProfile(BoxProfile) { opaque = 1; fillColor = \"84 92 124 255\"; };         Content.add(new GuiTileCtrl(A) { position = \"20 20\"; extent = \"60 60\"; color = \"220 60 60 255\"; });\
         Content.add(new GuiTileCtrl(B) { position = \"100 20\"; extent = \"60 60\"; color = \"60 180 90 255\"; });\
         Content.add(new GuiTileCtrl(C) { position = \"180 20\"; extent = \"60 60\"; color = \"70 110 230 255\"; });\
         Content.add(new GuiControl(Panel) { profile = \"BoxProfile\"; position = \"20 110\"; extent = \"220 60\"; });\
         Panel.add(new GuiTileCtrl(D) { position = \"10 10\"; extent = \"40 40\"; color = \"240 200 60 255\"; });",
    );
    // B is clicked three times; the pointer rests on C.
    for _ in 0..3 {
        click(&mut client, (110, 60));
    }
    client.host.mouse_move((200, 60));
    client.host.run(32, |_| false);
    let mut cache = torque_app::draw_cache();
    let draws = client.host.draw(&mut cache);
    let mut pass = GuiPass::new(&gpu, None);
    let target = gpu.offscreen(320, 200).expect("a target");
    pass.draw(&gpu, &draws, &target.color, Some([0.0, 0.0, 0.0, 1.0]));
    let frame = gpu.read_back(&target).expect("the picture");
    if let Some(dir) = folder() {
        std::fs::create_dir_all(&dir).unwrap();
        torque_app::shots::write_png(&frame, &dir.join("tiles.png")).unwrap();
    }
    // The content's profile fill is the backdrop.
    assert_eq!(pixel(&frame, 5, 190), [40, 44, 60]);
    // Each tile is its color; the one inside the panel too, over the panel's fill.
    assert_eq!(pixel(&frame, 50, 60), [220, 60, 60]);
    assert_eq!(pixel(&frame, 130, 60), [60, 180, 90]);
    assert_eq!(pixel(&frame, 210, 60), [70, 110, 230]);
    assert_eq!(pixel(&frame, 50, 140), [240, 200, 60]);
    assert_eq!(
        pixel(&frame, 200, 160),
        [84, 92, 124],
        "the panel's own fill, a plain GuiControl's"
    );
    // B shows three bars of its clicks along the top and A none; C is outlined by the hover and A, B are not.
    assert_eq!(pixel(&frame, 100 + 3 + 1, 20 + 3 + 1), [0, 0, 0]);
    assert_eq!(pixel(&frame, 100 + 3 + 13, 20 + 3 + 1), [0, 0, 0]);
    assert_eq!(
        pixel(&frame, 100 + 3 + 19, 20 + 3 + 1),
        [60, 180, 90],
        "three bars"
    );
    assert_eq!(
        pixel(&frame, 20 + 3 + 1, 20 + 3 + 1),
        [220, 60, 60],
        "A has none"
    );
    assert_eq!(pixel(&frame, 180, 20), [255, 255, 255]);
    assert_eq!(pixel(&frame, 100, 20), [60, 180, 90]);
}

/// An extension written before control classes: it draws its own class through the deprecated `draw_control`.
struct Legacy;

impl torque_gui::GuiExtension for Legacy {
    fn into_any(self: std::rc::Rc<Self>) -> std::rc::Rc<dyn std::any::Any> {
        self
    }

    #[allow(deprecated)]
    fn draw_control(
        &self,
        d: &mut DrawCtx<'_, '_>,
        class: &str,
        id: torque_ts_vm::sim::ObjId,
        offset: (i32, i32),
        _update: [i32; 4],
        _depth: usize,
    ) -> bool {
        if class != "GuiLegacyCtrl" {
            return false;
        }
        let extent = torque_gui::point(d.vm(), id, b"extent");
        d.fill([offset.0, offset.1, extent.0, extent.1], [12, 34, 56, 255]);
        true
    }
}

#[test]
fn an_extensions_draw_control_still_draws_its_class_beside_the_plugins_controls() {
    let mut builder =
        ClientBuilder::from_engine(&engine(), None, ClientNet::None).expect("the client");
    builder.gui.force_canvas_size(Some((320, 200)));
    builder
        .vm
        .register_class("GuiLegacyCtrl", "GuiControl", &[]);
    builder.gui.set_extension(std::rc::Rc::new(Legacy));
    builder
        .vm
        .eval(
            b"createCanvas(\"test\");\
              new GuiControlProfile(GuiDefaultProfile) { };\
              new GuiControl(Content) { extent = \"640 480\"; };\
              Canvas.setContent(Content);\
              Content.add(new GuiLegacyCtrl(L) { position = \"10 10\"; extent = \"30 20\"; });\
              Content.add(new GuiTileCtrl(T) { position = \"100 10\"; extent = \"30 20\"; color = \"1 2 3 255\"; });",
        )
        .expect("the controls");
    let mut client = builder.build::<GameServer>();
    client.host.run(100, |_| false);
    let draws = client.host.draw_frame();
    let quads = |color: [u8; 4]| {
        draws
            .batches
            .iter()
            .flat_map(|b| &b.quads)
            .filter(|q| q.color == color)
            .map(|q| q.rect)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        quads([12, 34, 56, 255]),
        [[10.0, 10.0, 40.0, 30.0]],
        "the extension's class"
    );
    assert_eq!(
        quads([1, 2, 3, 255]),
        [[100.0, 10.0, 130.0, 30.0]],
        "the plugin's class"
    );
}
