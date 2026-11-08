//! The layout containers in a client: flex rows and columns that grow, shrink, wrap and align; grids of fixed, `fr` and `auto`
//! tracks with spans and placements; nesting with absolute GUIs both ways; the least size; and laying out again on a resize, a
//! child added or removed and a field changed, but not otherwise. The expected bounds are worked out by hand from the CSS rules
//! (each test says how).

use torque_app::plugins::DefaultPlugins;
use torque_app::{ClientBuilder, ClientNet, GameClient, GameServer};
use torque_engine::{Engine, EngineBuilder, EngineClass, Plugin, PluginId};
use torque_gui::custom::{ControlCx, GuiBuilderExt, GuiControlClass};
use torque_plugin_gui_layout::GuiLayoutPlugin;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::VmError;

/// A plain control that counts how often it is resized, in `$resizes`.
#[derive(EngineClass)]
#[class(name = "GuiProbeCtrl", parent = "GuiControl")]
struct Probe {}

impl GuiControlClass for Probe {
    type State = ();
    const ACTIVE: bool = false;

    fn resize(&self, _: &mut (), cx: &mut ControlCx<'_>, position: (i32, i32), extent: (i32, i32)) {
        let n: i32 = String::from_utf8_lossy(&cx.vm.global("$resizes").unwrap_or_default())
            .parse()
            .unwrap_or(0);
        cx.vm.set_global("$resizes", &(n + 1).to_string());
        cx.base_resize(position, extent);
    }
}

/// A control whose frame update fails when `fail` is set, and otherwise counts its updates in `$updates`.
#[derive(EngineClass)]
#[class(name = "GuiTickCtrl", parent = "GuiControl")]
struct Tick {
    fail: bool,
}

impl GuiControlClass for Tick {
    type State = ();
    const ACTIVE: bool = false;

    fn update(&self, _: &mut (), cx: &mut ControlCx<'_>) -> Result<(), VmError> {
        if self.fail {
            return Err(VmError::StepLimit);
        }
        let n: i32 = String::from_utf8_lossy(&cx.vm.global("$updates").unwrap_or_default())
            .parse()
            .unwrap_or(0);
        cx.vm.set_global("$updates", &(n + 1).to_string());
        Ok(())
    }
}

#[derive(PluginId)]
#[plugin(id = "test.probe")]
struct ProbePlugin;

impl Plugin for ProbePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_gui_control::<Probe>().add_gui_control::<Tick>();
    }
}

fn engine() -> Engine {
    EngineBuilder::new()
        .add_plugins((DefaultPlugins, GuiLayoutPlugin, ProbePlugin))
        .build()
        .expect("the engine builds")
}

/// A client with a 640 x 480 canvas whose content `Content` (640 x 480, absolute) holds `script`'s controls.
fn client(script: &str) -> GameClient<GameServer> {
    let mut builder =
        ClientBuilder::from_engine(&engine(), None, ClientNet::None).expect("the client");
    builder.gui.force_canvas_size(Some((640, 480)));
    builder
        .vm
        .eval(
            b"createCanvas(\"test\");\
              new GuiControlProfile(GuiDefaultProfile) { };\
              new GuiControl(Content) { extent = \"640 480\"; };\
              Canvas.setContent(Content);",
        )
        .expect("the canvas");
    builder.vm.eval(script.as_bytes()).expect("the controls");
    let mut client = builder.build::<GameServer>();
    client.host.run(32, |_| false);
    client
}

fn id(client: &GameClient<GameServer>, name: &str) -> ObjId {
    client
        .vm()
        .objects()
        .find(name.as_bytes())
        .unwrap_or_else(|| panic!("no object {name}"))
}

/// `[x, y, width, height]` of the control `name`, its position from its parent.
fn rect(client: &GameClient<GameServer>, name: &str) -> [i32; 4] {
    let ((x, y), (w, h)) = torque_gui::bounds(client.vm(), id(client, name));
    [x, y, w, h]
}

fn rects(client: &GameClient<GameServer>, names: &[&str]) -> Vec<[i32; 4]> {
    names.iter().map(|n| rect(client, n)).collect()
}

/// One frame of the GUI (what each drawn frame starts with).
fn frame(client: &mut GameClient<GameServer>) {
    let gui = client.host.gui.clone();
    gui.frame(client.vm_mut());
}

fn eval(client: &mut GameClient<GameServer>, code: &str) -> String {
    let out = client
        .vm_mut()
        .eval(code.as_bytes())
        .expect("the script runs");
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn the_plugin_adds_both_classes_and_their_fields() {
    let engine = engine();
    let classes: Vec<_> = engine.classes().map(|c| c.name).collect();
    assert_eq!(
        classes,
        ["GuiFlexCtrl", "GuiGridCtrl", "GuiProbeCtrl", "GuiTickCtrl"]
    );
    let mut client = client("new GuiFlexCtrl(F); new GuiGridCtrl(G);");
    assert_eq!(
        eval(
            &mut client,
            "return F.direction SPC F.alignItems SPC F.gap;"
        ),
        "row stretch 0"
    );
    assert_eq!(
        eval(&mut client, "return G.autoRows SPC G.justifyItems;"),
        "auto stretch"
    );
    assert!(client.host.gui.is_custom(id(&client, "F")));
}

/// A row of 410: three children of 50 and two gaps of 10 leave 240, which B (grow 1) and C (grow 2) share as 80 and 160.
#[test]
fn a_row_grows_its_children_by_their_share_of_the_room() {
    let client = client(
        "Content.add(new GuiFlexCtrl(Row) {\
            extent = \"410 100\"; gap = 10; alignItems = \"start\";\
            new GuiControl(A) { extent = \"50 20\"; };\
            new GuiControl(B) { extent = \"50 20\"; flexGrow = 1; };\
            new GuiControl(C) { extent = \"50 30\"; flexGrow = 2; };\
         });",
    );
    assert_eq!(
        rects(&client, &["A", "B", "C"]),
        [[0, 0, 50, 20], [60, 0, 130, 20], [200, 0, 210, 30]]
    );
}

/// 150 + 50 in a row of 100: the 100 too many is taken in proportion to shrink times size (75 and 25). With B's least size
/// 40, B stops there and A takes the rest (60); a child with shrink 0 keeps its size.
#[test]
fn a_row_shrinks_its_children_down_to_their_least_size() {
    let client = client(
        "Content.add(new GuiFlexCtrl(One) { extent = \"100 40\"; alignItems = \"start\";\
            new GuiControl(A1) { extent = \"150 20\"; };\
            new GuiControl(B1) { extent = \"50 20\"; };\
         });\
         Content.add(new GuiFlexCtrl(Two) { position = \"0 50\"; extent = \"100 40\"; alignItems = \"start\";\
            new GuiControl(A2) { extent = \"150 20\"; };\
            new GuiControl(B2) { extent = \"50 20\"; minExtent = \"40 2\"; };\
         });\
         Content.add(new GuiFlexCtrl(Three) { position = \"0 100\"; extent = \"100 40\"; alignItems = \"start\";\
            new GuiControl(A3) { extent = \"150 20\"; };\
            new GuiControl(B3) { extent = \"50 20\"; flexShrink = 0; };\
         });",
    );
    assert_eq!(
        rects(&client, &["A1", "B1"]),
        [[0, 0, 75, 20], [75, 0, 25, 20]]
    );
    assert_eq!(
        rects(&client, &["A2", "B2"]),
        [[0, 0, 60, 20], [60, 0, 40, 20]]
    );
    assert_eq!(
        rects(&client, &["A3", "B3"]),
        [[0, 0, 50, 20], [50, 0, 50, 20]]
    );
}

/// Six children of 60 x 30 in a wrapping row of 200 with gaps of 10: three fit a line (60 + 10 + 60 + 10 + 60), the second
/// line 30 + 10 below the first.
#[test]
fn a_wrapping_row_starts_new_lines() {
    let client = client(
        "Content.add(new GuiFlexCtrl(Wrap) { extent = \"200 200\"; wrap = \"wrap\"; gap = 10; alignItems = \"start\"; });\
         for (%i = 0; %i < 6; %i++) Wrap.add(new GuiControl(\"W\" @ %i) { extent = \"60 30\"; });",
    );
    assert_eq!(
        rects(&client, &["W0", "W1", "W2", "W3", "W4", "W5"]),
        [
            [0, 0, 60, 30],
            [70, 0, 60, 30],
            [140, 0, 60, 30],
            [0, 40, 60, 30],
            [70, 40, 60, 30],
            [140, 40, 60, 30],
        ]
    );
}

/// A column of 200 x 300 holding 100 x 50 and 60 x 50 with a gap of 20: centered, the 120 of content starts at 90 and each
/// child is centered across (50 and 70); `space-between` puts them at 0 and 250; `stretch` makes them as wide as the room
/// inside the padding, but `alignSelf` keeps one at the end.
#[test]
fn a_column_justifies_and_aligns() {
    let client = client(
        "Content.add(new GuiFlexCtrl(Mid) { extent = \"200 300\"; direction = \"column\"; gap = 20;\
            justifyContent = \"center\"; alignItems = \"center\";\
            new GuiControl(M1) { extent = \"100 50\"; };\
            new GuiControl(M2) { extent = \"60 50\"; };\
         });\
         Content.add(new GuiFlexCtrl(Ends) { position = \"200 0\"; extent = \"200 300\"; direction = \"column\";\
            justifyContent = \"space-between\"; alignItems = \"start\";\
            new GuiControl(E1) { extent = \"100 50\"; };\
            new GuiControl(E2) { extent = \"60 50\"; };\
         });\
         Content.add(new GuiFlexCtrl(Wide) { position = \"400 0\"; extent = \"200 300\"; direction = \"column\"; padding = 10;\
            new GuiControl(S1) { extent = \"100 50\"; };\
            new GuiControl(S2) { extent = \"60 50\"; alignSelf = \"end\"; };\
         });",
    );
    assert_eq!(
        rects(&client, &["M1", "M2"]),
        [[50, 90, 100, 50], [70, 160, 60, 50]]
    );
    assert_eq!(
        rects(&client, &["E1", "E2"]),
        [[0, 0, 100, 50], [0, 250, 60, 50]]
    );
    assert_eq!(
        rects(&client, &["S1", "S2"]),
        [[10, 10, 180, 50], [130, 60, 60, 50]]
    );
}

/// Columns `100 1fr 2fr` in 420 with column gaps of 10: 300 is left for 3fr, so 100 and 200, at 0, 110 and 220. The auto row is
/// as tall as its tallest child (45), the fixed one 50; D spans two columns (0 to 210) and E auto-flows to the third. A child
/// placed by `gridRow`/`gridColumn` goes there; with `justifyItems = start` the children keep their own widths.
#[test]
fn a_grid_sizes_fixed_fr_and_auto_tracks_and_spans() {
    let client = client(
        "Content.add(new GuiGridCtrl(Grid) { extent = \"420 200\"; columns = \"100 1fr 2fr\"; rows = \"auto 50\"; gap = \"0 10\";\
            new GuiControl(GA) { extent = \"40 30\"; };\
            new GuiControl(GB) { extent = \"60 45\"; };\
            new GuiControl(GC) { extent = \"70 20\"; };\
            new GuiControl(GD) { extent = \"10 10\"; gridColumnSpan = 2; };\
            new GuiControl(GE) { extent = \"10 10\"; };\
         });\
         Content.add(new GuiGridCtrl(Placed) { position = \"0 220\"; extent = \"300 200\"; columns = \"repeat(3, 100)\";\
            rows = \"repeat(2, 60)\"; justifyItems = \"start\"; alignItems = \"start\";\
            new GuiControl(P1) { extent = \"40 30\"; gridRow = 2; gridColumn = 3; };\
            new GuiControl(P2) { extent = \"50 20\"; };\
            new GuiControl(P3) { extent = \"30 20\"; gridColumn = -2; };\
         });",
    );
    assert_eq!(
        rects(&client, &["GA", "GB", "GC", "GD", "GE"]),
        [
            [0, 0, 100, 45],
            [110, 0, 100, 45],
            [220, 0, 200, 45],
            [0, 45, 210, 50],
            [220, 45, 200, 50],
        ]
    );
    // P1 sits at row 2, column 3; P2 auto-flows into the first free cell; P3 starts at the line before the last (column 3).
    assert_eq!(
        rects(&client, &["P1", "P2", "P3"]),
        [[200, 60, 40, 30], [0, 0, 50, 20], [200, 0, 30, 20]]
    );
}

/// A row inside an absolute GUI, its child an absolute panel whose own child has `horizSizing = width`: the panel grows by the
/// row's rule and the inner child follows by Torque's. A column nested in the row is one layout with it, as wide as its widest
/// child; a child with `layoutIgnore` keeps its own place.
#[test]
fn layouts_nest_with_absolute_guis_both_ways() {
    let client = client(
        "Content.add(new GuiControl(Panel) { position = \"20 30\"; extent = \"500 300\";\
            new GuiFlexCtrl(Bar) { position = \"10 10\"; extent = \"400 100\"; alignItems = \"start\"; gap = 10;\
               new GuiControl(Box) { extent = \"100 50\"; flexGrow = 1;\
                  new GuiControl(Inner) { position = \"10 10\"; extent = \"80 20\"; horizSizing = \"width\"; };\
               };\
               new GuiFlexCtrl(Stack) { direction = \"column\"; gap = 5; alignItems = \"start\";\
                  new GuiControl(T1) { extent = \"70 20\"; };\
                  new GuiControl(T2) { extent = \"90 20\"; };\
               };\
               new GuiControl(Loose) { position = \"300 60\"; extent = \"30 30\"; layoutIgnore = 1; };\
            };\
         });",
    );
    // The column is 90 wide (its widest child) and 45 tall; Box takes the rest: 400 - 10 - 90 = 300.
    assert_eq!(rect(&client, "Stack"), [310, 0, 90, 45]);
    assert_eq!(rect(&client, "Box"), [0, 0, 300, 50]);
    assert_eq!(
        rect(&client, "Inner"),
        [10, 10, 280, 20],
        "80 + the 200 the box grew"
    );
    assert_eq!(
        rects(&client, &["T1", "T2"]),
        [[0, 0, 70, 20], [0, 25, 90, 20]]
    );
    assert_eq!(rect(&client, "Loose"), [300, 60, 30, 30]);
    assert_eq!(
        rect(&client, "Bar"),
        [10, 10, 400, 100],
        "the row keeps its absolute place"
    );
}

/// Two children of 80 that do not shrink need 160: a row asked to be 100 wide is 160, and says so in `contentMinExtent`.
#[test]
fn the_outermost_container_is_never_smaller_than_its_content() {
    let mut client = client(
        "Content.add(new GuiFlexCtrl(Tight) { extent = \"100 20\"; alignItems = \"start\";\
            new GuiControl() { extent = \"80 30\"; flexShrink = 0; };\
            new GuiControl() { extent = \"80 30\"; flexShrink = 0; };\
         });\
         Content.add(new GuiFlexCtrl(Squeeze) { position = \"0 100\"; extent = \"50 20\"; alignItems = \"start\";\
            new GuiControl() { extent = \"80 30\"; minExtent = \"30 10\"; };\
            new GuiControl() { extent = \"80 30\"; minExtent = \"30 10\"; };\
         });",
    );
    assert_eq!(rect(&client, "Tight"), [0, 0, 160, 30]);
    assert_eq!(
        eval(&mut client, "return Tight.contentMinExtent;"),
        "160 30"
    );
    // Children that can shrink need only their least widths (60), but all of their height (30).
    assert_eq!(rect(&client, "Squeeze"), [0, 100, 60, 30]);
}

/// The canvas grows from 640 to 800: the content (`width`) and a row with `horizSizing = width` grow by 160, and the growing
/// child takes it.
#[test]
fn a_resize_lays_out_again() {
    let mut client = client(
        "Content.horizSizing = \"width\";\
         Content.add(new GuiFlexCtrl(Row) { extent = \"400 50\"; horizSizing = \"width\"; alignItems = \"start\";\
            new GuiControl(Fixed) { extent = \"100 20\"; };\
            new GuiControl(Grow) { extent = \"100 20\"; flexGrow = 1; };\
         });",
    );
    assert_eq!(rect(&client, "Grow"), [100, 0, 300, 20]);
    client.host.gui.set_canvas_size((800, 480));
    frame(&mut client);
    assert_eq!(rect(&client, "Row"), [0, 0, 560, 50]);
    assert_eq!(rect(&client, "Grow"), [100, 0, 460, 20]);
    client.host.gui.set_canvas_size((640, 480));
    frame(&mut client);
    assert_eq!(rect(&client, "Grow"), [100, 0, 300, 20], "and back");
}

/// A child added, a field changed, a child hidden, one resized by a script, one removed: each is laid out in the next frame.
/// Frames with nothing changed lay nothing out (the probe child is not resized).
#[test]
fn changes_lay_out_in_the_next_frame_and_nothing_else_does() {
    let mut client = client(
        "Content.add(new GuiFlexCtrl(Row) { extent = \"300 50\"; alignItems = \"start\";\
            new GuiProbeCtrl(P) { extent = \"50 20\"; };\
            new GuiControl(Q) { extent = \"50 20\"; };\
         });",
    );
    assert_eq!(rect(&client, "Q"), [50, 0, 50, 20]);
    eval(&mut client, "$resizes = 0;");
    for _ in 0..10 {
        frame(&mut client);
    }
    assert_eq!(
        eval(&mut client, "return $resizes;"),
        "0",
        "no layout while nothing changes"
    );

    eval(
        &mut client,
        "Row.add(new GuiControl(N) { extent = \"40 20\"; });",
    );
    frame(&mut client);
    assert_eq!(rect(&client, "N"), [100, 0, 40, 20], "added");
    assert_eq!(eval(&mut client, "return $resizes;"), "1");

    eval(&mut client, "Q.flexGrow = 1;");
    frame(&mut client);
    assert_eq!(
        rects(&client, &["Q", "N"]),
        [[50, 0, 210, 20], [260, 0, 40, 20]],
        "a field"
    );

    eval(&mut client, "P.setVisible(false);");
    frame(&mut client);
    assert_eq!(rect(&client, "Q"), [0, 0, 260, 20], "hidden");

    eval(&mut client, "P.setVisible(true); N.extent = \"100 20\";");
    frame(&mut client);
    assert_eq!(
        rects(&client, &["Q", "N"]),
        [[50, 0, 150, 20], [200, 0, 100, 20]],
        "a new size"
    );

    eval(&mut client, "N.delete();");
    frame(&mut client);
    assert_eq!(rect(&client, "Q"), [50, 0, 250, 20], "removed");

    eval(&mut client, "Row.direction = \"column\";");
    frame(&mut client);
    assert_eq!(
        rects(&client, &["P", "Q"]),
        [[0, 0, 50, 20], [0, 20, 50, 30]],
        "the container's own field"
    );
    eval(&mut client, "$resizes = 0;");
    for _ in 0..10 {
        frame(&mut client);
    }
    assert_eq!(eval(&mut client, "return $resizes;"), "0");
}

/// A stretched or grown child keeps the size it was given only while the layout gives it: back in a row that does not
/// stretch, it has its own extent again.
#[test]
fn a_childs_own_size_survives_being_stretched() {
    let mut client = client(
        "Content.add(new GuiFlexCtrl(Col) { extent = \"300 200\"; direction = \"column\";\
            new GuiControl(K) { extent = \"60 20\"; };\
         });",
    );
    assert_eq!(rect(&client, "K"), [0, 0, 300, 20], "stretched across");
    eval(&mut client, "Col.alignItems = \"start\";");
    frame(&mut client);
    assert_eq!(rect(&client, "K"), [0, 0, 60, 20], "its own width again");
}

/// The fields `layoutWidth`, `layoutHeight` (lengths, percentages), `margin`, `flexBasis` and `layoutMaxExtent`.
#[test]
fn sizes_margins_and_bounds_from_fields() {
    let client = client(
        "Content.add(new GuiFlexCtrl(Row) { extent = \"400 100\"; alignItems = \"start\";\
            new GuiControl(Half) { extent = \"10 10\"; layoutWidth = \"50%\"; layoutHeight = 40; };\
            new GuiControl(Spaced) { extent = \"50 20\"; margin = \"5 10\"; };\
            new GuiControl(Capped) { extent = \"10 20\"; flexBasis = 0; flexGrow = 1; layoutMaxExtent = \"60 auto\"; };\
         });",
    );
    assert_eq!(rect(&client, "Half"), [0, 0, 200, 40]);
    assert_eq!(
        rect(&client, "Spaced"),
        [210, 5, 50, 20],
        "10 left and right, 5 on top"
    );
    assert_eq!(
        rect(&client, "Capped"),
        [270, 0, 60, 20],
        "grown to its largest size"
    );
}

/// A child added to a nested container changes the outer layout too: the nested column grows wider and the growing
/// sibling beside it gives up the room. A grid nested in a row is laid out in the cells the row gives it.
#[test]
fn a_change_inside_a_nested_container_lays_out_the_whole_layout() {
    let mut client = client(
        "Content.add(new GuiFlexCtrl(Outer) { extent = \"400 100\"; alignItems = \"start\";\
            new GuiControl(Fill) { extent = \"10 20\"; flexGrow = 1; };\
            new GuiFlexCtrl(Inner) { direction = \"column\"; alignItems = \"start\";\
               new GuiControl() { extent = \"50 20\"; };\
            };\
            new GuiGridCtrl(Cells) { layoutWidth = 100; columns = \"1fr 1fr\"; autoRows = 30;\
               new GuiControl(C1) { extent = \"5 5\"; };\
               new GuiControl(C2) { extent = \"5 5\"; };\
               new GuiControl(C3) { extent = \"5 5\"; gridColumnSpan = 2; };\
            };\
         });",
    );
    assert_eq!(rect(&client, "Fill"), [0, 0, 250, 20]);
    assert_eq!(rect(&client, "Inner"), [250, 0, 50, 20]);
    assert_eq!(rect(&client, "Cells"), [300, 0, 100, 60]);
    assert_eq!(
        rects(&client, &["C1", "C2", "C3"]),
        [[0, 0, 50, 30], [50, 0, 50, 30], [0, 30, 100, 30]]
    );
    eval(
        &mut client,
        "Inner.add(new GuiControl() { extent = \"120 20\"; });",
    );
    frame(&mut client);
    assert_eq!(rect(&client, "Inner"), [180, 0, 120, 40]);
    assert_eq!(rect(&client, "Fill"), [0, 0, 180, 20]);
}

/// Fields a hostile or careless script might set: the layout ends, and every control's rectangle stays inside the caps (a
/// length is at most 100,000 pixels and a template 256 tracks, so nothing is near the limits of an `i32`).
#[test]
fn hostile_fields_are_bounded() {
    let mut client = client(
        "Content.add(new GuiGridCtrl(G) { extent = \"300 300\"; columns = \"repeat(auto-fill, 0) repeat(60000, 1fr)\";\
            rows = \"repeat(auto-fit, minmax(0, 1fr))\"; gap = \"-40 1e30\"; padding = \"nan\"; });\
         for (%i = 0; %i < 50; %i++) G.add(new GuiControl(\"H\" @ %i) { extent = \"-5 1e12\";\
            gridRow = 99999; gridColumn = -99999; gridRowSpan = 70000; gridColumnSpan = \"x\"; margin = \"1e9 auto\";\
            flexGrow = -3; layoutMinExtent = \"-1 -1\"; layoutMaxExtent = \"0 nan\"; });\
         Content.add(new GuiFlexCtrl(F) { position = \"0 310\"; extent = \"300 100\"; direction = \"sideways\"; wrap = 7;\
            new GuiFlexCtrl() { new GuiFlexCtrl() { new GuiGridCtrl() { columns = \"repeat(3,\"; new GuiControl(Deep); }; }; };\
         });",
    );
    for _ in 0..3 {
        frame(&mut client);
    }
    for name in ["G", "H0", "H49", "F", "Deep"] {
        let [_, _, w, h] = rect(&client, name);
        assert!(
            (0..=100_000_000).contains(&w) && (0..=100_000_000).contains(&h),
            "{name}: {w} x {h}"
        );
    }
}

/// A chain of 20 nested containers is cut into layouts of 16 levels (the 16th below the top starts its own): a change at the
/// top or at the bottom lays the probe at the bottom out once, not once for each level a resize is passed up.
#[test]
fn a_deep_chain_of_containers_lays_out_once_per_change() {
    let mut client = client(
        "%p = Content;         for (%i = 0; %i < 20; %i++) { %c = new GuiFlexCtrl(\"L\" @ %i) { extent = \"10 10\"; }; %p.add(%c); %p = %c; }         %p.add(new GuiProbeCtrl(Deepest) { extent = \"20 20\"; });",
    );
    assert_eq!(rect(&client, "Deepest"), [0, 0, 20, 20]);
    // Past the cut the outer layout sees L16 as a plain child of its own extent (10 x 10), not its content's size.
    assert_eq!(rect(&client, "L0"), [0, 0, 10, 10]);
    eval(&mut client, "$resizes = 0;");
    frame(&mut client);
    assert_eq!(eval(&mut client, "return $resizes;"), "0");
    for change in ["L0.gap = 2;", "L19.gap = 3;", "L16.alignItems = \"start\";"] {
        eval(&mut client, &format!("$resizes = 0; {change}"));
        frame(&mut client);
        assert_eq!(eval(&mut client, "return $resizes;"), "1", "{change}");
        frame(&mut client);
        assert_eq!(
            eval(&mut client, "return $resizes;"),
            "1",
            "{change}: and not again"
        );
    }
}

/// One plugin control's failing update is printed and the others still update in the same frame.
#[test]
fn a_failing_update_does_not_stop_the_others() {
    let mut client = client(
        "Content.add(new GuiTickCtrl(Bad) { fail = 1; });         Content.add(new GuiTickCtrl(Good));",
    );
    eval(&mut client, "$updates = 0;");
    frame(&mut client);
    frame(&mut client);
    assert_eq!(eval(&mut client, "return $updates;"), "2");
    let log: Vec<String> = client
        .vm_mut()
        .drain_log()
        .into_iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect();
    assert!(
        log.iter().any(|l| l.contains("update of control")),
        "{log:?}"
    );
}

/// A container asked for an enormous extent is laid out at the length cap, and a grid's automatic repeat in it stays
/// within the track cap.
#[test]
fn a_huge_extent_is_clamped() {
    let mut client = client(
        "Content.add(new GuiGridCtrl(Huge) { extent = \"2000000000 2000000000\"; minExtent = \"2000000000 5\";            columns = \"repeat(auto-fill, 1)\"; rows = \"repeat(auto-fill, 1)\";            new GuiControl(Cell) { extent = \"5 5\"; };         });",
    );
    frame(&mut client);
    assert_eq!(rect(&client, "Huge")[2..], [100_000, 100_000]);
    assert_eq!(
        rect(&client, "Cell"),
        [0, 0, 8, 2],
        "the first one-pixel cell, raised to its minExtent"
    );
}
