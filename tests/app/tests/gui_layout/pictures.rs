//! A sample menu laid out by the containers, drawn offscreen at 800 x 600 and again after the canvas grew to 1280 x 720: a
//! column of buttons with a spacer that keeps Quit at the bottom, beside a panel whose grid of items fills as many columns
//! as fit. Skipped with a note when the machine has no GPU; `TORQUE_GUI_PICTURES=<folder>` also writes both pictures there
//! as PNGs.

use std::path::PathBuf;

use torque_app::plugins::DefaultPlugins;
use torque_app::{ClientBuilder, ClientNet, GameClient, GameServer};
use torque_engine::EngineBuilder;
use torque_plugin_gui_layout::GuiLayoutPlugin;
use torque_render_core::GuiPass;

const MENU: &str = r#"
new GuiControlProfile(GuiDefaultProfile) { };
new GuiControlProfile(BackProfile) { opaque = true; fillColor = "18 22 30 255"; };
new GuiControlProfile(PanelProfile) { opaque = true; border = true; fillColor = "30 36 48 255"; borderColor = "70 82 104 255"; };
new GuiControlProfile(HeadProfile) { fontType = "Arial"; fontSize = 26; fontColors[0] = "246 240 228 255"; justify = "center"; };
new GuiControlProfile(LabelProfile) { fontType = "Arial"; fontSize = 15; fontColors[0] = "226 230 238 255"; justify = "center"; };
new GuiControlProfile(ButtonProfile) {
   opaque = true; border = true; fillColor = "48 58 78 255"; fillColorHL = "255 176 72 255"; borderColor = "100 118 150 255";
   fontType = "Arial"; fontSize = 20; fontColors[0] = "238 242 248 255"; justify = "center";
};
new GuiControlProfile(QuitProfile : ButtonProfile) { fillColor = "120 48 48 255"; borderColor = "170 90 90 255"; };
new GuiControlProfile(TileProfile0) { opaque = true; border = true; fillColor = "60 110 170 255"; borderColor = "110 150 210 255"; };
new GuiControlProfile(TileProfile1) { opaque = true; border = true; fillColor = "70 140 90 255"; borderColor = "120 190 140 255"; };
new GuiControlProfile(TileProfile2) { opaque = true; border = true; fillColor = "160 110 50 255"; borderColor = "210 160 100 255"; };
new GuiControlProfile(TileProfile3) { opaque = true; border = true; fillColor = "120 70 150 255"; borderColor = "170 120 200 255"; };

new GuiControl(Back) {
   profile = "BackProfile"; extent = "800 600"; horizSizing = "width"; vertSizing = "height";

   new GuiFlexCtrl(Menu) {
      profile = "PanelProfile"; position = "24 24"; extent = "220 552"; vertSizing = "height";
      direction = "column"; padding = 16; gap = 10;
      new GuiTextCtrl(Title) { profile = "HeadProfile"; enabled = 1; extent = "188 40"; text = "Main Menu"; };
      new GuiButtonCtrl(Play) { profile = "ButtonProfile"; enabled = 1; extent = "188 44"; text = "Play"; };
      new GuiButtonCtrl(Join) { profile = "ButtonProfile"; enabled = 1; extent = "188 44"; text = "Join a game"; };
      new GuiButtonCtrl(Options) { profile = "ButtonProfile"; enabled = 1; extent = "188 44"; text = "Options"; };
      new GuiControl(Spacer) { extent = "10 10"; flexGrow = 1; };
      new GuiButtonCtrl(Quit) { profile = "QuitProfile"; enabled = 1; extent = "188 44"; text = "Quit"; };
   };

   new GuiFlexCtrl(Side) {
      profile = "PanelProfile"; position = "268 24"; extent = "508 552"; horizSizing = "width"; vertSizing = "height";
      direction = "column"; padding = 16; gap = 12;
      new GuiTextCtrl(ItemsTitle) { profile = "HeadProfile"; enabled = 1; extent = "200 36"; text = "Inventory"; };
      new GuiGridCtrl(Items) {
         flexGrow = 1; columns = "repeat(auto-fill, minmax(104, 1fr))"; autoRows = 80; gap = 12; alignContent = "start";
      };
   };
};
"#;

/// The items: a featured one two columns wide and two rows tall, then thirteen, each a tile with its name along the bottom.
const ITEMS: &str = r#"
%names = "Sword Shield Bow Arrows Potion Map Torch Rope Lantern Key Compass Helmet Boots";
Items.add(new GuiControl(Featured) { profile = "TileProfile2"; extent = "104 80"; gridColumnSpan = 2; gridRowSpan = 2;
   new GuiTextCtrl() { profile = "HeadProfile"; enabled = 1; position = "0 22"; extent = "104 36"; horizSizing = "width"; vertSizing = "center"; text = "Featured"; };
});
for (%i = 0; %i < getWordCount(%names); %i++) {
   %tile = new GuiControl("Item" @ %i) { profile = "TileProfile" @ (%i % 4); extent = "104 80"; };
   %tile.add(new GuiTextCtrl() { profile = "LabelProfile"; enabled = 1; position = "0 56"; extent = "104 20"; horizSizing = "width"; vertSizing = "top"; text = getWord(%names, %i); });
   Items.add(%tile);
}
"#;

fn client() -> GameClient<GameServer> {
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, GuiLayoutPlugin))
        .build()
        .expect("the engine builds");
    let mut builder =
        ClientBuilder::from_engine(&engine, None, ClientNet::None).expect("the client");
    builder.gui.force_canvas_size(Some((800, 600)));
    builder.gui.0.borrow_mut().fonts =
        torque_gui::SharedFonts::new(Box::new(torque_app::layout_fonts()));
    builder
        .vm
        .eval(b"createCanvas(\"test\");")
        .expect("the canvas");
    builder.vm.eval(MENU.as_bytes()).expect("the menu");
    builder.vm.eval(ITEMS.as_bytes()).expect("the items");
    builder
        .vm
        .eval(b"Canvas.setContent(Back);")
        .expect("the content");
    let mut client = builder.build::<GameServer>();
    client.host.run(32, |_| false);
    client
}

fn rect(client: &GameClient<GameServer>, name: &str) -> [i32; 4] {
    let id = client
        .vm()
        .objects()
        .find(name.as_bytes())
        .unwrap_or_else(|| panic!("no object {name}"));
    let ((x, y), (w, h)) = torque_gui::bounds(client.vm(), id);
    [x, y, w, h]
}

fn folder() -> Option<PathBuf> {
    std::env::var_os("TORQUE_GUI_PICTURES").map(PathBuf::from)
}

fn pixel(frame: &torque_render_core::Frame, x: i32, y: i32) -> [u8; 3] {
    let i = 4 * (y as u32 * frame.width + x as u32) as usize;
    [frame.rgba[i], frame.rgba[i + 1], frame.rgba[i + 2]]
}

/// The layout itself, which needs no GPU.
#[test]
fn the_menu_and_the_grid_follow_the_canvas() {
    let mut client = client();
    // The menu: 552 tall, padding 16, four children of 40 + 44 x 3 and five gaps of 10 leave the spacer 552 - 32 - 40 - 132
    // - 44 - 50 = 254, so Quit is at 16 + 40 + 132 + 30 + 254 + 20 = 492.
    assert_eq!(rect(&client, "Quit"), [16, 492, 188, 44]);
    assert_eq!(rect(&client, "Play"), [16, 66, 188, 44]);
    // The grid: 508 - 32 = 476 wide gives four columns of at least 104 (4 x 104 + 3 x 12 = 452), each (476 - 36) / 4 = 110;
    // its five rows of 80 (448) fit the 472 the panel leaves it.
    assert_eq!(rect(&client, "Items"), [16, 64, 476, 472]);
    assert_eq!(rect(&client, "Featured"), [0, 0, 232, 172]);
    assert_eq!(rect(&client, "Item0"), [244, 0, 110, 80]);
    assert_eq!(rect(&client, "Item2"), [244, 92, 110, 80]);
    assert_eq!(rect(&client, "Item4"), [0, 184, 110, 80]);
    client.host.gui.set_canvas_size((1280, 720));
    client.host.run(32, |_| false);
    // 120 taller: the spacer takes it. 480 wider: 956 holds eight columns of (956 - 84) / 8 = 109.
    assert_eq!(rect(&client, "Quit"), [16, 612, 188, 44]);
    assert_eq!(rect(&client, "Items"), [16, 64, 956, 592]);
    assert_eq!(rect(&client, "Item0"), [242, 0, 109, 80]);
    assert_eq!(rect(&client, "Item5"), [847, 0, 109, 80]);
    assert_eq!(rect(&client, "Item6"), [242, 92, 109, 80]);
}

#[test]
fn the_menu_is_drawn_before_and_after_a_resize() {
    let gpu = match torque_app::window::offscreen_gpu() {
        Ok(gpu) => gpu,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let mut client = client();
    for (size, name) in [
        ((800, 600), "gui-layout-menu-800x600.png"),
        ((1280, 720), "gui-layout-menu-1280x720.png"),
    ] {
        client.host.gui.set_canvas_size(size);
        client.host.run(32, |_| false);
        let mut cache = torque_app::draw_cache();
        let draws = client.host.draw(&mut cache);
        let mut pass = GuiPass::new(&gpu, None);
        let target = gpu
            .offscreen(size.0 as u32, size.1 as u32)
            .expect("a target");
        pass.draw(&gpu, &draws, &target.color, Some([0.0, 0.0, 0.0, 1.0]));
        let frame = gpu.read_back(&target).expect("the picture");
        if let Some(dir) = folder() {
            std::fs::create_dir_all(&dir).unwrap();
            torque_app::shots::write_png(&frame, &dir.join(name)).unwrap();
        }
        // The backdrop, the menu panel's fill, the Quit button at the panel's foot and the featured tile at the grid's corner.
        assert_eq!(pixel(&frame, 4, 4), [18, 22, 30], "{name}");
        assert_eq!(pixel(&frame, 30, 200), [30, 36, 48], "{name}");
        let quit = rect(&client, "Quit");
        assert_eq!(
            pixel(&frame, 24 + quit[0] + 4, 24 + quit[1] + 4),
            [120, 48, 48],
            "{name}"
        );
        assert_eq!(
            pixel(&frame, 268 + 16 + 6, 24 + 64 + 6),
            [160, 110, 50],
            "{name}"
        );
    }
}
