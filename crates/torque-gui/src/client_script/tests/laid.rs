//! Containers that lay their children out, as a layout plugin's do: placed when they wake or are resized, and again in a frame
//! that follows a change to what they read (their children's count, extents and visibility). The layout is small (a column, and a
//! grid of two columns) and is the contract of a plugin's: it moves its children only through the control's context.

use torque_engine::EngineClass;
use torque_ts_vm::vm::VmError;

use super::super::RealmLayout;
use super::*;
use crate::control::{children, flag};
use crate::custom::{ControlCx, GuiBuilderExt, GuiControlClass};

/// What a container's layout read when it last laid out.
#[derive(Default, PartialEq)]
pub(super) struct Print(Vec<(ObjId, (i32, i32), bool)>);

fn print_of(cx: &ControlCx<'_>) -> Print {
    Print(
        children(cx.vm, cx.id)
            .into_iter()
            .map(|c| (c, bounds(cx.vm, c).1, flag(cx.vm, c, b"visible")))
            .collect(),
    )
}

/// Puts the shown children in a column, or in two columns.
fn lay_out(state: &mut Print, cx: &mut ControlCx<'_>, columns: usize) {
    let mut y = [0; 2];
    let shown: Vec<ObjId> = children(cx.vm, cx.id)
        .into_iter()
        .filter(|c| flag(cx.vm, *c, b"visible"))
        .collect();
    for (i, child) in shown.into_iter().enumerate() {
        let column = i % columns;
        let extent = bounds(cx.vm, child).1;
        cx.set_bounds_of(child, (column as i32 * 70, y[column]), extent);
        y[column] += extent.1 + 2;
    }
    *state = print_of(cx);
}

macro_rules! laid_class {
    ($ty:ident, $name:literal, $columns:literal) => {
        #[derive(EngineClass)]
        #[class(name = $name, parent = "GuiControl")]
        struct $ty {}

        impl GuiControlClass for $ty {
            type State = Print;
            const ACTIVE: bool = false;

            fn resize(
                &self,
                state: &mut Print,
                cx: &mut ControlCx<'_>,
                position: (i32, i32),
                extent: (i32, i32),
            ) {
                cx.base_resize(position, extent);
                lay_out(state, cx, $columns);
            }

            fn update(&self, state: &mut Print, cx: &mut ControlCx<'_>) -> Result<(), VmError> {
                if *state != print_of(cx) {
                    lay_out(state, cx, $columns);
                }
                Ok(())
            }

            fn wake(&self, state: &mut Print, cx: &mut ControlCx<'_>) -> Result<(), VmError> {
                lay_out(state, cx, $columns);
                Ok(())
            }
        }
    };
}

laid_class!(LaidFlex, "LaidFlexCtrl", 1);
laid_class!(LaidGrid, "LaidGridCtrl", 2);

static LAID_LAYOUT: RealmLayout = RealmLayout::new(
    "LaidFlexCtrl",
    "LaidGridCtrl",
    &[],
    ("layoutWidth", "layoutHeight"),
);

#[derive(PluginId)]
#[plugin(id = "test.client-script-laid")]
struct LaidPlugin;

impl Plugin for LaidPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        add_controls(engine);
        engine
            .add_gui_control::<LaidFlex>()
            .add_gui_control::<LaidGrid>()
            .add_realm_layout(&LAID_LAYOUT);
    }
}

/// A rig whose engine has containers that lay their children out.
pub(super) fn laid_rig(tiers: &[Tier]) -> Rig {
    let engine = EngineBuilder::new()
        .add_plugins(LaidPlugin)
        .build()
        .expect("the engine builds");
    Rig::of(&engine, tiers)
}

#[test]
fn the_test_layout_places_children_when_they_change() {
    let mut rig = laid_rig(&[Tier::Ui]);
    let mut o = Ops::default();
    o.root(1, "mod")
        .create(
            2,
            1,
            Kind::Flex,
            &[
                (Prop::Pos, vec2(10.0, 10.0)),
                (Prop::Size, vec2(100.0, 100.0)),
            ],
        )
        .create(3, 2, Kind::Panel, &[(Prop::Size, vec2(20.0, 10.0))])
        .create(4, 2, Kind::Panel, &[(Prop::Size, vec2(20.0, 30.0))]);
    rig.apply_ok(&o);
    assert_eq!(bounds(&rig.vm, rig.object(4)).0, (0, 12));
    let mut o = Ops::default();
    o.set(3, &[(Prop::Size, vec2(20.0, 50.0))]);
    rig.apply_ok(&o);
    assert_eq!(bounds(&rig.vm, rig.object(4)).0, (0, 52), "laid out again");
}

/// A frame drawn after a layout moved the children is not the one kept from before it, though nothing but the layout changed.
#[test]
fn a_layout_that_moves_children_leaves_no_stale_kept_draws() {
    let mut rig = laid_rig(&[Tier::Ui]);
    let panel = |color: [u8; 4], h: f64| {
        [
            (Prop::Size, vec2(20.0, h)),
            (Prop::Fill, PropValue::Color(color)),
        ]
    };
    let mut o = Ops::default();
    o.root(1, "mod")
        .create(
            2,
            1,
            Kind::Flex,
            &[
                (Prop::Pos, vec2(10.0, 110.0)),
                (Prop::Size, vec2(100.0, 100.0)),
            ],
        )
        .create(3, 2, Kind::Panel, &panel([200, 0, 0, 255], 10.0))
        .create(4, 2, Kind::Panel, &panel([0, 200, 0, 255], 30.0))
        .create(5, 2, Kind::Panel, &panel([0, 0, 200, 255], 10.0));
    rig.apply_ok(&o);
    let draw = |rig: &mut Rig| draw_canvas(&rig.vm, &rig.gui, &mut rig.cache).draws;
    draw(&mut rig);
    // The flush, a draw, then the frame that lays out: the draw recorded what the layout is about to change.
    let mut o = Ops::default();
    o.set(3, &[(Prop::Size, vec2(20.0, 50.0))]);
    rig.clock += 1000;
    rig.realm
        .apply_flush(&mut rig.vm, &rig.gui, o.bytes(), rig.clock)
        .expect("the flush applies");
    draw(&mut rig);
    rig.frame();
    let replays = rig.cache.layer_replays;
    let after = draw(&mut rig);
    assert_eq!(
        rig.cache.layer_replays, replays,
        "drawn again after the layout"
    );
    let mut cold = DrawCache::for_gui(&rig.gui);
    let fresh = draw_canvas(&rig.vm, &rig.gui, &mut cold).draws;
    assert_eq!(super::picture::drawn(&after), super::picture::drawn(&fresh));
    draw(&mut rig);
    assert_eq!(
        rig.cache.layer_replays,
        replays + 1,
        "kept again once it is the same"
    );
}
