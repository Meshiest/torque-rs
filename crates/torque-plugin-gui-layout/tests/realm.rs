//! The layout in a client-script realm: `flex` and `grid` controls made by the realm's applier are laid out by this plugin's
//! classes, as a script's own would be; an engine without the plugin keeps them as plain groups; and a hostile op sequence
//! never breaks the realm's invariants or the layout.

use torque_app::plugins::DefaultPlugins;
use torque_app::{ClientBuilder, ClientNet, GameClient, GameServer};
use torque_client_script_format::ui::{self, Kind, KindDef, Prop, PropType, PropValue, Track};
use torque_client_script_format::{Grants, Tier, Writer, op};
use torque_engine::{Engine, EngineBuilder, Plugin, PluginId};
use torque_gui::client_script::{Realm, RealmView, TierToggle, add_controls};
use torque_plugin_gui_layout::GuiLayoutPlugin;

#[derive(PluginId)]
#[plugin(id = "test.realm-controls")]
struct RealmControls;

impl Plugin for RealmControls {
    fn build(&self, engine: &mut EngineBuilder) {
        add_controls(engine);
    }
}

fn engine(layout: bool) -> Engine {
    let mut builder = EngineBuilder::new();
    builder.add_plugins((DefaultPlugins, RealmControls));
    if layout {
        builder.add_plugins(GuiLayoutPlugin);
    }
    builder.build().expect("the engine builds")
}

/// A client with a 640 x 480 canvas and a realm with the `ui` tier.
struct Rig {
    client: GameClient<GameServer>,
    realm: Realm,
    clock: u64,
}

impl Rig {
    fn new(layout: bool) -> Rig {
        let mut builder =
            ClientBuilder::from_engine(&engine(layout), None, ClientNet::None).expect("the client");
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
        let mut client = builder.build::<GameServer>();
        client.host.run(32, |_| false);
        let gui = client.host.gui.clone();
        gui.set_client_script_screens(&["Content"]);
        let view = RealmView {
            address: "10.0.0.1:28000".into(),
            name: "A Server".into(),
            tiers: vec![TierToggle {
                tier: Tier::Ui,
                on: true,
            }],
        };
        let realm = Realm::new(client.vm_mut(), &gui, Grants::of(&[Tier::Ui]), view)
            .expect("the realm starts");
        Rig {
            client,
            realm,
            clock: 0,
        }
    }

    fn apply(&mut self, ops: &Ops) -> Result<(), torque_gui::client_script::Violation> {
        self.clock += 1000;
        let gui = self.client.host.gui.clone();
        let result = self
            .realm
            .apply_flush(self.client.vm_mut(), &gui, ops.0.bytes(), self.clock);
        self.frame();
        result.map(|_| ())
    }

    fn frame(&mut self) {
        let gui = self.client.host.gui.clone();
        gui.frame(self.client.vm_mut());
    }

    /// `[x, y, width, height]` of the control `handle`, its position from its parent.
    fn rect(&self, handle: u32) -> [i32; 4] {
        let obj = self.realm.object(handle).expect("a live handle");
        let ((x, y), (w, h)) = torque_gui::bounds(self.client.vm(), obj);
        [x, y, w, h]
    }
}

/// Builds the ops of a flush.
#[derive(Default)]
struct Ops(Writer);

impl Ops {
    fn record(&mut self, kind: u8, body: impl FnOnce(&mut Writer)) -> &mut Ops {
        let mark = self.0.begin(Tier::Ui.id(), kind);
        body(&mut self.0);
        self.0.end(mark).expect("the record fits");
        self
    }

    fn root(&mut self, handle: u32) -> &mut Ops {
        self.record(op::ui::ROOT, |w| {
            w.u32(handle);
            w.text("mod");
        })
    }

    fn create(
        &mut self,
        handle: u32,
        parent: u32,
        kind: Kind,
        props: &[(Prop, PropValue)],
    ) -> &mut Ops {
        self.record(op::ui::CREATE, |w| {
            w.u32(handle);
            w.u32(parent);
            w.u8(kind as u8);
            write_sorted(w, props);
        })
    }

    fn set(&mut self, handle: u32, props: &[(Prop, PropValue)]) -> &mut Ops {
        self.record(op::ui::SET, |w| {
            w.u32(handle);
            write_sorted(w, props);
        })
    }

    fn parent(&mut self, handle: u32, parent: u32) -> &mut Ops {
        self.record(op::ui::PARENT, |w| {
            w.u32(handle);
            w.u32(parent);
        })
    }

    fn destroy(&mut self, handle: u32) -> &mut Ops {
        self.record(op::ui::DESTROY, |w| w.u32(handle))
    }
}

fn write_sorted(w: &mut Writer, props: &[(Prop, PropValue)]) {
    let mut sorted: Vec<&(Prop, PropValue)> = props.iter().collect();
    sorted.sort_by_key(|(p, _)| *p as u8);
    ui::write_props(w, sorted.into_iter().map(|(p, v)| (*p, v)));
}

fn size(w: f64, h: f64) -> (Prop, PropValue) {
    (Prop::Size, PropValue::Vec2(w, h))
}

fn grow(share: f64) -> (Prop, PropValue) {
    (Prop::Grow, PropValue::Number(share))
}

/// The index of `word` among the names of the enum prop `prop`.
fn word(prop: Prop, word: &str) -> PropValue {
    let PropType::Enum(names) = ui::prop_def(prop).ty else {
        panic!("{prop:?} is not an enum");
    };
    let at = names.iter().position(|n| *n == word).expect("a name");
    PropValue::Enum(at as u8)
}

/// A row of 410: three children of 50 and two gaps of 10 leave 240, which B (grow 1) and C (grow 2) share as 80 and 160.
#[test]
fn a_realms_flex_container_lays_its_children_out() {
    let mut rig = Rig::new(true);
    let mut ops = Ops::default();
    ops.root(1)
        .create(
            2,
            1,
            Kind::Flex,
            &[
                size(410.0, 100.0),
                (Prop::Gap, PropValue::Vec2(0.0, 10.0)),
                (Prop::AlignItems, word(Prop::AlignItems, "start")),
            ],
        )
        .create(3, 2, Kind::Panel, &[size(50.0, 20.0)])
        .create(4, 2, Kind::Panel, &[size(50.0, 20.0), grow(1.0)])
        .create(5, 2, Kind::Button, &[size(50.0, 30.0), grow(2.0)]);
    rig.apply(&ops).expect("the flush applies");
    assert_eq!(rig.rect(3), [0, 0, 50, 20]);
    assert_eq!(rig.rect(4), [60, 0, 130, 20]);
    assert_eq!(rig.rect(5), [200, 0, 210, 30]);

    // A set that turns the row into a column lays it out again in the next frame. The column is 100 high, the children take 70
    // of it (the gap between rows is 0), and the 30 left go to B and C in the shares 1 and 2.
    let mut ops = Ops::default();
    ops.set(2, &[(Prop::Direction, word(Prop::Direction, "column"))]);
    rig.apply(&ops).expect("the flush applies");
    assert_eq!(rig.rect(3), [0, 0, 50, 20]);
    assert_eq!(rig.rect(4), [0, 20, 50, 30]);
    assert_eq!(rig.rect(5), [0, 50, 50, 50]);
    rig.realm
        .check(rig.client.vm(), &rig.client.host.gui)
        .expect("the invariants hold");
}

/// A grid of 300 x 100 with a 100 column and a 1fr column, and a 40 row: A and B stretch to their cells, C is placed on the
/// second row and spans both columns, which is 60 high (the rest of the 100) once its row is `1fr`.
#[test]
fn a_realms_grid_container_places_children_in_cells() {
    let mut rig = Rig::new(true);
    let mut ops = Ops::default();
    ops.root(1)
        .create(
            2,
            1,
            Kind::Grid,
            &[
                size(300.0, 100.0),
                (
                    Prop::Columns,
                    PropValue::Tracks(vec![Track::Px(100.0), Track::Fr(1.0)]),
                ),
                (
                    Prop::TrackRows,
                    PropValue::Tracks(vec![Track::Px(40.0), Track::Fr(1.0)]),
                ),
            ],
        )
        .create(3, 2, Kind::Panel, &[])
        .create(4, 2, Kind::Panel, &[])
        .create(
            5,
            2,
            Kind::Panel,
            &[
                (Prop::Row, PropValue::Int(2)),
                (Prop::Column, PropValue::Int(1)),
                (Prop::ColumnSpan, PropValue::Int(2)),
            ],
        );
    rig.apply(&ops).expect("the flush applies");
    assert_eq!(rig.rect(3), [0, 0, 100, 40]);
    assert_eq!(rig.rect(4), [100, 0, 200, 40]);
    assert_eq!(rig.rect(5), [0, 40, 300, 60]);
}

/// A container inside a container is laid out at the size it was given, not at its content's.
#[test]
fn a_container_inside_a_container_keeps_the_size_it_was_given() {
    let mut rig = Rig::new(true);
    let mut ops = Ops::default();
    ops.root(1)
        .create(
            2,
            1,
            Kind::Flex,
            &[
                size(200.0, 200.0),
                (Prop::Direction, word(Prop::Direction, "column")),
                (Prop::AlignItems, word(Prop::AlignItems, "start")),
                (Prop::Padding, PropValue::Sides([10.0; 4])),
            ],
        )
        .create(3, 2, Kind::Flex, &[size(120.0, 50.0)])
        .create(4, 3, Kind::Panel, &[size(20.0, 20.0)]);
    rig.apply(&ops).expect("the flush applies");
    assert_eq!(rig.rect(3), [10, 10, 120, 50]);
}

#[test]
fn without_the_plugin_a_container_places_nothing() {
    let mut rig = Rig::new(false);
    let mut ops = Ops::default();
    ops.root(1)
        .create(2, 1, Kind::Flex, &[size(410.0, 100.0)])
        .create(
            3,
            2,
            Kind::Panel,
            &[
                (Prop::Pos, PropValue::Vec2(7.0, 9.0)),
                size(50.0, 20.0),
                grow(1.0),
            ],
        );
    rig.apply(&ops).expect("the flush applies");
    assert_eq!(rig.rect(3), [7, 9, 50, 20]);
    rig.realm
        .check(rig.client.vm(), &rig.client.host.gui)
        .expect("the invariants hold");
}

/// A small xorshift: the same ops every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A value of `prop`'s type, within its range.
fn random_value(rng: &mut Rng, prop: Prop) -> Option<PropValue> {
    let def = ui::prop_def(prop);
    Some(match def.ty {
        PropType::Number { min, max } => PropValue::Number(min + rng.unit() * (max - min)),
        PropType::Int { min, max } => {
            PropValue::Int(min + rng.below(u64::from(max - min) + 1) as u32)
        }
        PropType::Vec2 { min, max } => PropValue::Vec2(
            min + rng.unit() * (max - min),
            min + rng.unit() * (max - min),
        ),
        PropType::Sides { min, max } => {
            PropValue::Sides([0; 4].map(|_| min + rng.unit() * (max - min)))
        }
        PropType::Tracks { max } => PropValue::Tracks(
            (0..rng.below(max as u64 + 1))
                .map(|_| match rng.below(3) {
                    0 => Track::Auto,
                    1 => Track::Px(rng.unit() * 400.0),
                    _ => Track::Fr(rng.unit() * 4.0),
                })
                .collect(),
        ),
        PropType::Enum(names) => PropValue::Enum(rng.below(names.len() as u64) as u8),
        PropType::Bool => PropValue::Bool(rng.below(2) == 0),
        // Text, colors and images are not what the layout reads.
        _ => return None,
    })
}

/// The layout-relevant props of `kind` with random values, in range.
fn random_props(rng: &mut Rng, kind: &KindDef) -> Vec<(Prop, PropValue)> {
    let props: Vec<Prop> = kind.props().collect();
    let mut out: Vec<(Prop, PropValue)> = Vec::new();
    for _ in 0..rng.below(7) {
        let prop = props[rng.below(props.len() as u64) as usize];
        // Keep the kind's text small and its size sane.
        if out.iter().any(|(p, _)| *p == prop) || matches!(prop, Prop::Pos) {
            continue;
        }
        if let Some(value) = random_value(rng, prop) {
            out.push((prop, value));
        }
    }
    out
}

#[test]
fn random_container_ops_keep_the_realm_and_its_layout_sound() {
    let mut rig = Rig::new(true);
    let mut start = Ops::default();
    start.root(1);
    rig.apply(&start).expect("the root");
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let kinds = [
        Kind::Flex,
        Kind::Grid,
        Kind::Panel,
        Kind::Button,
        Kind::Text,
    ];
    let mut live: Vec<u32> = vec![1];
    let mut next = 2;
    let steps = if std::env::var_os("TORQUE_CLIENT_SCRIPT_FUZZ").is_some() {
        60_000
    } else {
        4000
    };
    for step in 0..steps {
        let mut ops = Ops::default();
        match rng.below(10) {
            0..=4 => {
                let kind = kinds[rng.below(kinds.len() as u64) as usize];
                let parent = live[rng.below(live.len() as u64) as usize];
                let props = random_props(&mut rng, ui::kind_def(kind));
                ops.create(next, parent, kind, &props);
                if rig.apply(&ops).is_ok() {
                    live.push(next);
                    next += 1;
                }
            }
            5..=6 => {
                let target = live[rng.below(live.len() as u64) as usize];
                let kind = rig_kind(&rig, target);
                let props = random_props(&mut rng, ui::kind_def(kind));
                ops.set(target, &props);
                let _ = rig.apply(&ops);
            }
            7 => {
                let target = live[rng.below(live.len() as u64) as usize];
                let to = live[rng.below(live.len() as u64) as usize];
                ops.parent(target, to);
                let _ = rig.apply(&ops);
            }
            _ => {
                let target = live[rng.below(live.len() as u64) as usize];
                ops.destroy(target);
                if rig.apply(&ops).is_ok() {
                    live = rig.realm.handles();
                }
            }
        }
        if step % 100 == 0 {
            rig.frame();
            rig.realm
                .check(rig.client.vm(), &rig.client.host.gui)
                .unwrap_or_else(|e| panic!("step {step}: {e}"));
            for &handle in &live {
                let [x, y, w, h] = rig.rect(handle);
                let sane = |v: i32| (-2_000_000..=2_000_000).contains(&v);
                assert!(
                    [x, y, w, h].into_iter().all(sane) && w >= 0 && h >= 0,
                    "step {step}: control {handle} is at {x},{y} and {w} x {h}"
                );
            }
        }
    }
    assert!(rig.realm.control_count() > 0);
}

/// The kind of the live control `handle`, from its class.
fn rig_kind(rig: &Rig, handle: u32) -> Kind {
    let objects = rig.client.vm().objects();
    let obj = rig.realm.object(handle).expect("live");
    let class = objects.get(obj).expect("an object").class();
    match objects.classes.name(class) {
        b"GuiFlexCtrl" => Kind::Flex,
        b"GuiGridCtrl" => Kind::Grid,
        b"ClientScriptButton" => Kind::Button,
        b"ClientScriptText" => Kind::Text,
        b"ClientScriptPanel" => Kind::Panel,
        // A mod root is never set.
        _ => Kind::Panel,
    }
}

/// A grid of 32 x 32 fractions in 600 x 600 holding `kids` panels of `span` x `span` cells, flowing `flow` (`"row"` or
/// `"row dense"`): what a hostile server would build to make the layout work hard.
fn crowded_grid(span: u32, kids: u32, flow: &str) -> Rig {
    let mut rig = Rig::new(true);
    let mut ops = Ops::default();
    ops.root(1).create(
        2,
        1,
        Kind::Grid,
        &[
            size(600.0, 600.0),
            (Prop::Columns, PropValue::Tracks(vec![Track::Fr(1.0); 32])),
            (Prop::TrackRows, PropValue::Tracks(vec![Track::Fr(1.0); 32])),
            (Prop::AutoFlow, word(Prop::AutoFlow, flow)),
        ],
    );
    for k in 0..kids {
        ops.create(
            3 + k,
            2,
            Kind::Panel,
            &[
                (Prop::RowSpan, PropValue::Int(span)),
                (Prop::ColumnSpan, PropValue::Int(span)),
            ],
        );
    }
    rig.apply(&ops).expect("the flush applies");
    rig
}

/// Changes a prop of the grid every frame for `frames` frames: the time of a frame and the work its layout was charged.
fn churn(rig: &mut Rig, frames: u32) -> (std::time::Duration, u64) {
    let before = rig.client.host.gui.client_script_stats();
    let start = std::time::Instant::now();
    for i in 0..frames {
        let mut ops = Ops::default();
        ops.set(
            2,
            &[(
                Prop::Gap,
                PropValue::Vec2(f64::from(i % 7), f64::from(i % 5)),
            )],
        );
        rig.apply(&ops).expect("the flush applies");
    }
    let after = rig.client.host.gui.client_script_stats();
    (
        start.elapsed() / frames,
        (after.layout_work - before.layout_work) / u64::from(frames),
    )
}

/// 256 children of 32 x 32 cells flowing `row dense` make taffy rescan about 270,000 cells for each child, far past what a
/// frame may spend: the layout is charged for it, refused, and the children keep their bounds, every frame.
#[test]
fn a_dense_grid_of_wide_children_is_refused_by_its_charge() {
    let mut rig = crowded_grid(32, 256, "row dense");
    let (frame, work) = churn(&mut rig, 30);
    assert!(
        work > torque_gui::MAX_FRAME_WORK,
        "a frame's layout was charged {work}"
    );
    assert!(
        frame < std::time::Duration::from_millis(250),
        "{frame:?} a frame"
    );
    for handle in [3, 100, 258] {
        assert_eq!(
            rig.rect(handle),
            [0, 0, 128, 32],
            "child {handle} was not placed"
        );
    }
    rig.realm
        .check(rig.client.vm(), &rig.client.host.gui)
        .expect("the invariants hold");
}

/// The same children flowing `row` fit the budget, are placed, and are charged for their cells: far more than the same
/// number of one-cell children.
#[test]
fn the_charge_of_a_grid_follows_its_spans() {
    let mut wide = crowded_grid(32, 256, "row");
    let (frame, work) = churn(&mut wide, 30);
    assert!(work <= torque_gui::MAX_FRAME_WORK, "{work}");
    assert!(
        frame < std::time::Duration::from_millis(250),
        "{frame:?} a frame"
    );
    assert!(wide.rect(3)[2] > 128, "the children were laid out");
    let mut narrow = crowded_grid(1, 256, "row");
    let (_, narrow_work) = churn(&mut narrow, 30);
    assert!(work > narrow_work * 5, "{work} against {narrow_work}");
}

/// Nothing changes, nothing is checked: a realm of containers does no layout work in a frame until a change (or a resize)
/// makes the layouts due, and then only for two frames.
#[test]
fn an_idle_realm_with_containers_does_no_layout_work() {
    let mut rig = crowded_grid(1, 200, "row");
    for _ in 0..4 {
        rig.frame();
    }
    let idle = rig.client.host.gui.client_script_stats();
    assert!(idle.layout_updates > 0, "the layouts were checked once");
    for _ in 0..200 {
        rig.frame();
    }
    let still = rig.client.host.gui.client_script_stats();
    assert_eq!(still.layout_updates, idle.layout_updates);
    assert_eq!(still.layout_work, idle.layout_work);

    let mut ops = Ops::default();
    ops.set(2, &[(Prop::Gap, PropValue::Vec2(3.0, 3.0))]);
    rig.apply(&ops).expect("the flush applies");
    let changed = rig.client.host.gui.client_script_stats();
    assert!(changed.layout_work > still.layout_work);
    for _ in 0..50 {
        rig.frame();
    }
    let settled = rig.client.host.gui.client_script_stats();
    assert!(settled.layout_updates <= changed.layout_updates + 1);

    // A resize of the screen makes them due again.
    rig.client.host.gui.set_canvas_size((800, 600));
    rig.frame();
    assert!(rig.client.host.gui.client_script_stats().layout_updates > settled.layout_updates);
}
