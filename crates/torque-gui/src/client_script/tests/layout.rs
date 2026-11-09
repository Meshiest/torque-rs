//! Layout containers in the realm: the layout plugin's classes made with the checked props as CSS fields, the plain group an
//! engine without a layout makes, and the caps on nesting and children.

use torque_client_script_format::ui::{
    MAX_CONTAINER_CHILDREN, MAX_CONTAINER_DEPTH, MAX_TRACKS, Track,
};
use torque_engine::EngineClass;

use super::super::RealmLayout;
use super::*;
use crate::custom::{GuiBuilderExt, GuiControlClass};

/// A container class of a test layout: it lays nothing out, so a test sees what the applier gave it.
#[derive(EngineClass)]
#[class(name = "TestFlexCtrl", parent = "GuiControl")]
struct TestFlex {}

impl GuiControlClass for TestFlex {
    type State = ();
}

/// The grid of the test layout.
#[derive(EngineClass)]
#[class(name = "TestGridCtrl", parent = "GuiControl")]
struct TestGrid {}

impl GuiControlClass for TestGrid {
    type State = ();
}

static TEST_LAYOUT: RealmLayout = RealmLayout::new(
    "TestFlexCtrl",
    "TestGridCtrl",
    &[
        (Prop::Grow, "flexGrow"),
        (Prop::Basis, "flexBasis"),
        (Prop::Margin, "margin"),
        (Prop::Row, "gridRow"),
        (Prop::Direction, "direction"),
        (Prop::Gap, "gap"),
        (Prop::Padding, "padding"),
        (Prop::Columns, "columns"),
    ],
    ("layoutWidth", "layoutHeight"),
);

#[derive(PluginId)]
#[plugin(id = "test.client-script-layout")]
struct LayoutPlugin;

impl Plugin for LayoutPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        add_controls(engine);
        engine
            .add_gui_control::<TestFlex>()
            .add_gui_control::<TestGrid>()
            .add_realm_layout(&TEST_LAYOUT);
    }
}

/// A rig whose engine has the test layout.
pub(super) fn layout_rig(tiers: &[Tier]) -> Rig {
    let engine = EngineBuilder::new()
        .add_plugins(LayoutPlugin)
        .build()
        .expect("the engine builds");
    Rig::of(&engine, tiers)
}

fn class_of(rig: &Rig, handle: u32) -> String {
    let objects = rig.vm.objects();
    let class = objects.get(rig.object(handle)).expect("the object").class();
    String::from_utf8_lossy(objects.classes.name(class)).into_owned()
}

fn field(rig: &Rig, handle: u32, name: &str) -> String {
    let bytes = rig
        .vm
        .objects()
        .field(rig.object(handle), name.as_bytes(), b"")
        .unwrap_or_default();
    String::from_utf8(bytes).expect("text")
}

fn num(n: f64) -> PropValue {
    PropValue::Number(n)
}

#[test]
fn client_script_a_container_is_the_layouts_class_with_the_props_as_css_fields() {
    let mut rig = layout_rig(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod")
        .create(
            2,
            1,
            Kind::Flex,
            &[
                (Prop::Direction, PropValue::Enum(1)),
                (Prop::Gap, vec2(8.0, 16.0)),
                (Prop::Padding, PropValue::Sides([1.0, 2.0, 3.0, 4.0])),
                (Prop::Size, vec2(200.0, 100.0)),
            ],
        )
        .create(
            3,
            2,
            Kind::Button,
            &[
                (Prop::Grow, num(2.0)),
                (Prop::Basis, num(-1.0)),
                (Prop::Margin, PropValue::Sides([5.0; 4])),
                (Prop::Row, PropValue::Int(2)),
            ],
        )
        .create(
            4,
            1,
            Kind::Grid,
            &[(
                Prop::Columns,
                PropValue::Tracks(vec![Track::Px(120.0), Track::Fr(1.0), Track::Auto]),
            )],
        );
    rig.apply_ok(&ops);
    assert_eq!(class_of(&rig, 2), "TestFlexCtrl");
    assert_eq!(class_of(&rig, 3), "ClientScriptButton");
    assert_eq!(class_of(&rig, 4), "TestGridCtrl");
    assert_eq!(field(&rig, 2, "direction"), "column");
    assert_eq!(field(&rig, 2, "gap"), "8 16");
    assert_eq!(field(&rig, 2, "padding"), "1 2 3 4");
    assert_eq!(field(&rig, 2, "layoutWidth"), "200");
    assert_eq!(field(&rig, 2, "layoutHeight"), "100");
    assert_eq!(field(&rig, 3, "flexGrow"), "2");
    assert_eq!(field(&rig, 3, "flexBasis"), "auto");
    assert_eq!(field(&rig, 3, "margin"), "5 5 5 5");
    assert_eq!(field(&rig, 3, "gridRow"), "2");
    assert_eq!(field(&rig, 4, "columns"), "120 1fr auto");
    // Only what was given is written, and a leaf's size is its own.
    assert_eq!(field(&rig, 3, "flexShrink"), "");
    assert_eq!(field(&rig, 3, "layoutWidth"), "");
    assert_eq!(field(&rig, 4, "direction"), "");
    // The child is in the container's group.
    let group = rig.vm.objects().get(rig.object(3)).and_then(|o| o.group());
    assert_eq!(group, Some(rig.object(2)));

    // A set writes what it gives and keeps the rest.
    let mut ops = Ops::default();
    ops.set(
        2,
        &[(Prop::Gap, vec2(1.0, 2.0)), (Prop::Size, vec2(50.0, 60.0))],
    )
    .set(3, &[(Prop::Grow, num(0.5))])
    .set(4, &[(Prop::Columns, PropValue::Tracks(Vec::new()))]);
    rig.apply_ok(&ops);
    assert_eq!(field(&rig, 2, "gap"), "1 2");
    assert_eq!(field(&rig, 2, "direction"), "column");
    assert_eq!(field(&rig, 2, "layoutWidth"), "50");
    assert_eq!(field(&rig, 3, "flexGrow"), "0.5");
    assert_eq!(field(&rig, 3, "margin"), "5 5 5 5");
    assert_eq!(field(&rig, 4, "columns"), "");
    rig.realm
        .check(&rig.vm, &rig.gui)
        .expect("the invariants hold");
    assert_eq!(rig.realm.control_count(), 3);
}

#[test]
fn client_script_without_a_layout_a_container_is_a_plain_group() {
    let mut rig = Rig::new(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod")
        .create(
            2,
            1,
            Kind::Flex,
            &[
                (Prop::Direction, PropValue::Enum(1)),
                (Prop::Size, vec2(300.0, 300.0)),
            ],
        )
        .create(
            3,
            2,
            Kind::Button,
            &[
                (Prop::Pos, vec2(10.0, 10.0)),
                (Prop::Size, vec2(100.0, 30.0)),
                (Prop::Grow, num(1.0)),
            ],
        );
    rig.apply_ok(&ops);
    assert_eq!(class_of(&rig, 2), "ClientScriptGroup");
    assert_eq!(field(&rig, 2, "direction"), "");
    assert_eq!(field(&rig, 3, "flexGrow"), "");
    // The child stays where its own props put it, and a click on it still arrives.
    assert_eq!(bounds(&rig.vm, rig.object(3)), ((10, 10), (100, 30)));
    assert!(rig.click_at(20, 20));
    assert!(matches!(rig.events().as_slice(), [UiEvent::Press(3)]));
    // The group is not a thing to click: a click on it is no event.
    rig.click_at(250, 250);
    assert!(rig.events().is_empty());
    let mut set = Ops::default();
    set.set(2, &[(Prop::Gap, vec2(4.0, 4.0))]);
    rig.apply_ok(&set);
    rig.realm
        .check(&rig.vm, &rig.gui)
        .expect("the invariants hold");
}

#[test]
fn client_script_a_container_is_transparent_to_input_and_adds_no_look() {
    let mut rig = layout_rig(&[Tier::Ui]);
    // A button under a container that covers it: the click goes through the container to the button.
    let mut ops = Ops::default();
    ops.root(1, "mod")
        .create(
            2,
            1,
            Kind::Button,
            &[
                (Prop::Pos, vec2(10.0, 10.0)),
                (Prop::Size, vec2(100.0, 30.0)),
            ],
        )
        .create(3, 1, Kind::Flex, &[(Prop::Size, vec2(300.0, 300.0))])
        .create(
            4,
            3,
            Kind::Button,
            &[
                (Prop::Pos, vec2(10.0, 100.0)),
                (Prop::Size, vec2(100.0, 30.0)),
            ],
        );
    rig.apply_ok(&ops);
    assert!(rig.click_at(20, 20));
    assert!(matches!(rig.events().as_slice(), [UiEvent::Press(2)]));
    assert!(rig.click_at(20, 110));
    assert!(matches!(rig.events().as_slice(), [UiEvent::Press(4)]));
    rig.click_at(250, 250);
    assert!(rig.events().is_empty());
    assert_eq!(rig.draw().realm_controls, 2, "only the buttons draw");
}

#[test]
fn client_script_a_container_refuses_the_looks_it_lacks_and_hostile_values() {
    let mut rig = layout_rig(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod").create(2, 1, Kind::Grid, &[]);
    rig.apply_ok(&ops);
    let before = rig.realm.control_count();
    for (prop, value) in [
        (Prop::Opacity, num(1.0)),
        (Prop::Tooltip, text("x")),
        (Prop::Enabled, PropValue::Bool(true)),
        (Prop::Direction, PropValue::Enum(0)),
        (Prop::Gap, vec2(f64::NAN, 0.0)),
        (Prop::Gap, vec2(2000.0, 0.0)),
        (Prop::Padding, PropValue::Sides([0.0, 0.0, 0.0, -1.0])),
        (
            Prop::Columns,
            PropValue::Tracks(vec![Track::Auto; MAX_TRACKS + 1]),
        ),
        (
            Prop::Columns,
            PropValue::Tracks(vec![Track::Fr(f64::INFINITY)]),
        ),
        (Prop::Columns, PropValue::Tracks(vec![Track::Px(1e12)])),
    ] {
        let mut set = Ops::default();
        set.set(2, &[(prop, value.clone())]);
        // The writer of these ops does not check, as a hostile guest would not.
        assert!(rig.apply(&set).is_err(), "{prop:?} {value:?}");
        let mut create = Ops::default();
        create.create(rig.realm.next_handle(), 1, Kind::Grid, &[(prop, value)]);
        assert!(rig.apply(&create).is_err());
    }
    assert_eq!(
        rig.realm.control_count(),
        before,
        "a refused op changes nothing"
    );
    rig.realm
        .check(&rig.vm, &rig.gui)
        .expect("the invariants hold");
}

/// A rig with the test layout or without, whose realm has a root `1`.
fn rig_with_root(layout: bool) -> Rig {
    let mut rig = if layout {
        layout_rig(&[Tier::Ui])
    } else {
        Rig::new(&[Tier::Ui])
    };
    let mut ops = Ops::default();
    ops.root(1, "mod");
    rig.apply_ok(&ops);
    rig
}

#[test]
fn client_script_a_container_has_at_most_so_many_children() {
    for layout in [true, false] {
        let mut rig = rig_with_root(layout);
        let mut ops = Ops::default();
        ops.create(2, 1, Kind::Flex, &[]);
        for h in 0..MAX_CONTAINER_CHILDREN as u32 {
            ops.create(3 + h, 2, Kind::Panel, &[]);
        }
        rig.apply_ok(&ops);
        let extra = 3 + MAX_CONTAINER_CHILDREN as u32;
        let mut over = Ops::default();
        over.create(extra, 2, Kind::Panel, &[]);
        assert_eq!(
            rig.apply(&over),
            Err(Violation("a container with more children than the cap"))
        );
        assert_eq!(rig.realm.control_count(), MAX_CONTAINER_CHILDREN + 1);

        // A control from outside cannot be moved into the full container, and one inside can be moved about in it.
        let mut outside = Ops::default();
        outside.create(extra, 1, Kind::Panel, &[]);
        rig.apply_ok(&outside);
        let mut into = Ops::default();
        into.parent(extra, 2);
        assert_eq!(
            rig.apply(&into),
            Err(Violation("a container with more children than the cap"))
        );
        let mut within = Ops::default();
        within.parent(3, 2).order(4, true);
        rig.apply_ok(&within);
        // One that leaves makes room for exactly one.
        let mut swap = Ops::default();
        swap.parent(3, 1).parent(extra, 2);
        rig.apply_ok(&swap);
        let mut back = Ops::default();
        back.parent(3, 2);
        assert!(rig.apply(&back).is_err());
        rig.realm
            .check(&rig.vm, &rig.gui)
            .expect("the invariants hold");
    }
}

#[test]
fn client_script_containers_nest_at_most_so_deep() {
    for layout in [true, false] {
        let mut rig = rig_with_root(layout);
        let mut ops = Ops::default();
        for level in 1..=MAX_CONTAINER_DEPTH as u32 {
            ops.create(level + 1, level, Kind::Grid, &[]);
        }
        rig.apply_ok(&ops);
        let last = MAX_CONTAINER_DEPTH as u32 + 1;
        // A leaf at the bottom is no container; one more container is.
        let mut leaf = Ops::default();
        leaf.create(last + 1, last, Kind::Panel, &[]);
        rig.apply_ok(&leaf);
        let mut past = Ops::default();
        past.create(last + 2, last, Kind::Flex, &[]);
        assert_eq!(
            rig.apply(&past),
            Err(Violation("containers nested past the cap"))
        );
        assert_eq!(rig.realm.next_handle(), last + 2, "nothing was made");

        // Two containers moved under the seventh would be nine deep: refused by the subtree's height, not its size.
        let mut side = Ops::default();
        side.create(last + 2, 1, Kind::Flex, &[])
            .create(last + 3, last + 2, Kind::Flex, &[])
            .create(last + 4, 1, Kind::Panel, &[]);
        rig.apply_ok(&side);
        let mut moves = Ops::default();
        moves.parent(last + 2, 8);
        assert_eq!(
            rig.apply(&moves),
            Err(Violation("containers moved past the nesting cap"))
        );
        // A leaf goes anywhere, and one container fits under the seventh.
        let mut fits = Ops::default();
        fits.parent(last + 4, last).parent(last + 3, 8);
        rig.apply_ok(&fits);
        rig.realm
            .check(&rig.vm, &rig.gui)
            .expect("the invariants hold");
    }
}

#[test]
fn client_script_removing_a_container_removes_its_children() {
    let mut rig = layout_rig(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod")
        .create(2, 1, Kind::Flex, &[])
        .create(3, 2, Kind::Grid, &[])
        .create(4, 3, Kind::Panel, &[(Prop::Grow, num(1.0))]);
    rig.apply_ok(&ops);
    let mut gone = Ops::default();
    gone.destroy(2);
    rig.apply_ok(&gone);
    assert_eq!(rig.realm.control_count(), 0);
    let objects = rig.vm.objects();
    let left = objects
        .iter()
        .filter(|(_, o)| objects.classes.name(o.class()).starts_with(b"Test"))
        .count();
    assert_eq!(left, 0);
    assert_eq!(
        rig.realm_objects(),
        3,
        "the server layer, the root and the chip"
    );
    rig.realm
        .check(&rig.vm, &rig.gui)
        .expect("the invariants hold");
}

#[derive(PluginId)]
#[plugin(id = "test.second-layout")]
struct SecondLayout;

impl Plugin for SecondLayout {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_realm_layout(&TEST_LAYOUT);
    }
}

#[test]
fn client_script_a_second_layout_is_a_build_error() {
    let built = EngineBuilder::new()
        .add_plugins((LayoutPlugin, SecondLayout))
        .build();
    let error = built
        .expect_err("two layouts cannot both be the realm's")
        .to_string();
    assert!(error.contains("test.second-layout"), "{error}");
}

#[test]
fn client_script_layouts_are_checked_only_after_a_change_inside_one() {
    let mut rig = layout_rig(&[Tier::Ui]);
    let mut ops = Ops::default();
    ops.root(1, "mod")
        .create(2, 1, Kind::Flex, &[(Prop::Size, vec2(300.0, 300.0))])
        .create(3, 2, Kind::Panel, &[])
        .create(4, 2, Kind::Panel, &[])
        .create(5, 1, Kind::Panel, &[]);
    rig.apply_ok(&ops);
    let updates = |rig: &Rig| rig.gui.client_script_stats().layout_updates;
    // Settled after two frames, then nothing, however many frames pass.
    rig.frame();
    rig.frame();
    let settled = updates(&rig);
    assert!(settled > 0);
    for _ in 0..100 {
        rig.frame();
    }
    assert_eq!(updates(&rig), settled, "an idle realm checks no layout");

    // Each op that touches a container or what is in one makes the layouts due; one on a control outside does not.
    let mut outside = Ops::default();
    outside.set(5, &[(Prop::Grow, num(1.0))]);
    rig.apply_ok(&outside);
    rig.frame();
    rig.frame();
    rig.frame();
    let after_outside = updates(&rig);
    assert_eq!(
        after_outside, settled,
        "a control outside a container is not a layout input"
    );
    let changes: [fn(&mut Ops); 5] = [
        |o| {
            o.set(2, &[(Prop::Gap, vec2(1.0, 1.0))]);
        },
        |o| {
            o.set(3, &[(Prop::Grow, num(1.0))]);
        },
        |o| {
            o.order(3, true);
        },
        |o| {
            o.parent(4, 1);
        },
        |o| {
            o.destroy(3);
        },
    ];
    let mut seen = after_outside;
    for (i, change) in changes.iter().enumerate() {
        let mut ops = Ops::default();
        change(&mut ops);
        rig.apply_ok(&ops);
        for _ in 0..4 {
            rig.frame();
        }
        assert!(updates(&rig) > seen, "change {i} made no layout check");
        seen = updates(&rig);
    }
    rig.realm
        .check(&rig.vm, &rig.gui)
        .expect("the invariants hold");
}
