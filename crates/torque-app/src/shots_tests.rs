//! The offscreen seat draws every frame through the game's [`GameApp::draw`]: the default draws exactly what
//! [`WorldView::draw`] drew, and a game's own draw paints the frame instead (skipped with a note when the machine has no GPU).

use torque_engine::EngineBuilder;
use torque_gui::GuiDraws;
use torque_render_core::{Frame, Gpu, GuiPass, wgpu};
use torque_render_scene::source::Eye;

use crate::client::{ClientBuilder, ClientNet, GameClient};
use crate::orbit::Orbit;
use crate::plugins::DefaultPlugins;
use crate::shots::Seat;
use crate::view::WorldView;
use crate::{App, GameServer, Host};

const SIZE: (u32, u32) = (96, 54);
/// The panel's fill, at `PANEL_AT` within the canvas.
const PANEL: [u8; 3] = [200, 40, 40];
const PANEL_AT: (u32, u32) = (16, 12);
/// What the game's own draw clears the frame to, in 8-bit levels (exact in the target's format).
const OWN_CLEAR: [u8; 3] = [51, 153, 102];

fn gpu() -> Option<Gpu> {
    match crate::window::offscreen_gpu() {
        Ok(gpu) => Some(gpu),
        Err(e) => {
            eprintln!("skipped: {e}");
            None
        }
    }
}

/// A client of a plain engine whose canvas holds one opaque panel over nothing.
fn client() -> GameClient<GameServer> {
    let engine = EngineBuilder::new()
        .add_plugins(DefaultPlugins)
        .build()
        .expect("the engine builds");
    let mut builder =
        ClientBuilder::from_engine(&engine, None, ClientNet::None).expect("the client");
    builder
        .gui
        .force_canvas_size(Some((SIZE.0 as i32, SIZE.1 as i32)));
    builder
        .vm
        .eval(
            b"createCanvas(\"test\");\
              new GuiControlProfile(GuiDefaultProfile) { };\
              new GuiControlProfile(PanelProfile) { opaque = 1; fillColor = \"200 40 40 255\"; };\
              new GuiControl(Content) { extent = \"96 54\"; };\
              Content.add(new GuiControl(Panel) { profile = \"PanelProfile\"; position = \"8 8\"; extent = \"32 16\"; });\
              Canvas.setContent(Content);",
        )
        .expect("the canvas");
    let mut client = builder.build::<GameServer>();
    client.host.run(100, |_| false);
    client
}

/// A game that draws through the default [`App::draw`].
struct Panel {
    client: GameClient<GameServer>,
}

impl App for Panel {
    fn host(&mut self) -> &mut Host {
        &mut self.client.host
    }

    fn host_ref(&self) -> &Host {
        &self.client.host
    }

    fn title(&self) -> String {
        "panel".to_string()
    }

    fn log_name(&self) -> &str {
        "panel"
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        WorldView::new(gpu)
    }

    fn advance(&mut self, ms: u32) {
        self.client.step(ms);
    }

    fn frame(&mut self, _: &mut WorldView) -> (GuiDraws, Eye) {
        (
            self.client.host.draw_frame(),
            Orbit::default().eye([0.0; 3]),
        )
    }
}

/// [`Panel`] drawn by a renderer of its own: its GUI pass clears the frame to [`OWN_CLEAR`] and draws the GUI over it, the
/// view left alone. It counts its draws.
struct OwnLook {
    panel: Panel,
    pass: Option<GuiPass>,
    draws: u32,
}

impl App for OwnLook {
    fn host(&mut self) -> &mut Host {
        self.panel.host()
    }

    fn host_ref(&self) -> &Host {
        self.panel.host_ref()
    }

    fn title(&self) -> String {
        self.panel.title()
    }

    fn log_name(&self) -> &str {
        self.panel.log_name()
    }

    fn view(&self, gpu: &Gpu) -> WorldView {
        self.panel.view(gpu)
    }

    fn advance(&mut self, ms: u32) {
        self.panel.advance(ms);
    }

    fn frame(&mut self, view: &mut WorldView) -> (GuiDraws, Eye) {
        self.panel.frame(view)
    }

    fn draw(
        &mut self,
        _view: &mut WorldView,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        gui: &GuiDraws,
        _eye: Eye,
    ) -> Result<bool, String> {
        self.draws += 1;
        let clear = OWN_CLEAR.map(|c| f64::from(c) / 255.0);
        self.pass
            .get_or_insert_with(|| GuiPass::new(gpu, None))
            .draw(gpu, gui, target, Some([clear[0], clear[1], clear[2], 1.0]));
        Ok(false)
    }
}

/// The color at `(x, y)` of `frame`.
fn rgb(frame: &Frame, (x, y): (u32, u32)) -> [u8; 3] {
    let i = 4 * (y * frame.width + x) as usize;
    [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
}

#[test]
fn the_default_draw_is_the_views_draw_pixel_for_pixel() {
    let Some(gpu) = gpu() else {
        return;
    };
    let mut seat = Seat::new(Panel { client: client() }, &gpu, SIZE).expect("the seat");
    seat.frame(&gpu).expect("a frame");
    let hooked = gpu.read_back(&seat.target).expect("the picture");
    // The frame as the seat drew it before games drew their own: the view's draw called directly.
    let mut direct = Seat::new(Panel { client: client() }, &gpu, SIZE).expect("the seat");
    let (gui, eye) = direct.game.frame(&mut direct.view);
    direct
        .view
        .draw(&gpu, &direct.target.color, &gui, eye)
        .expect("a frame");
    let direct = gpu.read_back(&direct.target).expect("the picture");
    assert_eq!(rgb(&hooked, PANEL_AT), PANEL, "the panel is drawn");
    assert_eq!(
        rgb(&hooked, (90, 50)),
        [0, 0, 0],
        "no world: cleared to black"
    );
    assert!(
        hooked.rgba == direct.rgba,
        "the default draw differs from the view's"
    );
}

#[test]
fn a_games_own_draw_paints_the_frame_in_place_of_the_view() {
    let Some(gpu) = gpu() else {
        return;
    };
    let own = OwnLook {
        panel: Panel { client: client() },
        pass: None,
        draws: 0,
    };
    let mut seat = Seat::new(own, &gpu, SIZE).expect("the seat");
    let own = seat.picture(&gpu).expect("the picture");
    assert_eq!(seat.game.draws, crate::shots::SETTLE_FRAMES);
    let mut plain = Seat::new(Panel { client: client() }, &gpu, SIZE).expect("the seat");
    let plain = plain.picture(&gpu).expect("the picture");
    // The game's GUI over the game's own clear; the default's over black.
    assert_eq!(rgb(&own, PANEL_AT), PANEL);
    assert_eq!(rgb(&own, (90, 50)), OWN_CLEAR);
    assert_eq!(rgb(&plain, (90, 50)), [0, 0, 0]);
    let differing = own
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(plain.rgba.as_chunks::<4>().0)
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing,
        (SIZE.0 * SIZE.1 - 32 * 16) as usize,
        "every pixel but the panel's differs"
    );
}
