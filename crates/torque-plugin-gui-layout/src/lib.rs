//! Flex and grid layout for torque-rs GUIs: two container controls that lay their children out by CSS's flexbox and grid
//! rules (computed by [taffy](https://crates.io/crates/taffy)), inside ordinary GUIs that keep Torque's absolute positions.
//!
//! [`GuiLayoutPlugin`] adds the control classes
//!
//! - `GuiFlexCtrl` ([`FlexCtrl`]): its children in a row or a column, wrapped onto more lines or not, grown, shrunk and
//!   aligned;
//! - `GuiGridCtrl` ([`GridCtrl`]): its children in the cells of a grid of fixed, `fr` and `auto` tracks, placed by row and
//!   column or by the auto-flow, spanning tracks.
//!
//! It is not in `DefaultPlugins`: a game adds it.
//!
//! ```
//! use torque_engine::EngineBuilder;
//! use torque_plugin_gui_layout::GuiLayoutPlugin;
//!
//! let engine = EngineBuilder::new().add_plugins(GuiLayoutPlugin).build()?;
//! let classes: Vec<_> = engine.classes().map(|c| c.name).collect();
//! assert_eq!(classes, ["GuiFlexCtrl", "GuiGridCtrl"]);
//! # Ok::<(), torque_engine::BuildError>(())
//! ```
//!
//! A GUI file then uses them as any other control, with the engine's controls inside:
//!
//! ```text
//! new GuiFlexCtrl(MainMenu) {
//!    position = "40 40";
//!    extent = "240 400";
//!    direction = "column";
//!    gap = 8;
//!    padding = 16;
//!    new GuiButtonCtrl() { text = "Play"; extent = "100 30"; };
//!    new GuiButtonCtrl() { text = "Options"; extent = "100 30"; };
//!    new GuiControl() { flexGrow = 1; };                // the room left over
//!    new GuiButtonCtrl() { text = "Quit"; extent = "100 30"; };
//! };
//! ```
//!
//! # How it behaves
//!
//! - **The children's fields.** What the layout reads of a child is a dynamic field on the child ([`CHILD_FIELDS`]:
//!   `flexGrow`, `flexShrink`, `flexBasis`, `alignSelf`, `justifySelf`, `margin`, `gridRow`, `gridColumn`, `gridRowSpan`,
//!   `gridColumnSpan`, `layoutWidth`, `layoutHeight`, `layoutMinExtent`, `layoutMaxExtent`, `layoutIgnore`), so the engine's
//!   controls and any plugin's go inside unchanged. A child's size is its own `extent` unless `layoutWidth`/`layoutHeight`
//!   say otherwise or it is stretched; its least size is its `minExtent`. A hidden child (`visible = 0`) takes no room, and
//!   one with `layoutIgnore = 1` keeps its own position and its sizing flags, as in a `GuiControl`.
//! - **Nesting.** A container inside a container is one layout with it (its size follows its content unless its own
//!   `layoutWidth`/`layoutHeight` set it); a container inside an absolute GUI is laid out to its own extent, and the
//!   absolute controls inside a container's children are placed by their sizing flags, as always. Nesting is followed 16
//!   levels ([`MAX_NESTING`]): the container that deep is a plain child of the layout above (its own `extent`) and
//!   the outermost container of a layout of its own.
//! - **Minimum size.** The outermost container of a layout never takes an extent below what its content needs (nor below
//!   its own `minExtent`), the way `minExtent` clamps any control; the content's least size is in its `contentMinExtent`
//!   field after each layout.
//! - **When it lays out.** When it is resized (its parent's sizing flags, the canvas, a script's `resize`), when it wakes,
//!   and in the frame after a child is added or removed, shown or hidden, resized, or has a layout field changed: each
//!   frame the outermost container compares a fingerprint of those inputs with the last layout's and lays out only on a
//!   difference. A container in one of the host's overlays has no frame check: it lays out on resize and wake only.
//!
//! The rules are CSS's: a field's words are the CSS property's values (`space-between`, `1fr`, `minmax(80, 1fr)`,
//! `repeat(3, 1fr)`, ...), lengths in pixels without a unit.

use torque_engine::{EngineBuilder, EngineClass, Plugin, PluginId, ScriptClass};
use torque_gui::custom::{ControlCx, GuiBuilderExt, GuiControlClass};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

mod layout;
pub mod parse;

pub use layout::{CHILD_FIELDS, MAX_FIELD_BYTES, MAX_NESTING, MAX_NODES};

/// Adds `GuiFlexCtrl` and `GuiGridCtrl` to every client's GUI.
#[derive(Debug, Default, Clone, Copy, PluginId)]
#[plugin(id = "torque.gui-layout")]
pub struct GuiLayoutPlugin;

impl Plugin for GuiLayoutPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .add_gui_control::<FlexCtrl>()
            .add_gui_control::<GridCtrl>();
    }
}

/// `GuiFlexCtrl`: lays its children out along one axis by the flexbox rules.
#[derive(EngineClass, Debug, Clone, PartialEq)]
#[class(name = "GuiFlexCtrl", parent = "GuiControl")]
pub struct FlexCtrl {
    /// The main axis: `row` (left to right), `column` (top to bottom), `row-reverse` or `column-reverse`.
    #[field(default = "row")]
    pub direction: String,
    /// `nowrap` (one line), `wrap` (as many lines as the children need) or `wrap-reverse`.
    #[field(default = "nowrap")]
    pub wrap: String,
    /// Where the children sit along the main axis when they leave room: `start`, `end`, `center`, `space-between`,
    /// `space-around` or `space-evenly`.
    #[field(default = "start")]
    pub justify_content: String,
    /// Where the children sit across their line: `stretch` (they fill it), `start`, `end`, `center` or `baseline`.
    #[field(default = "stretch")]
    pub align_items: String,
    /// How the lines of a wrapped container share its cross axis: the words of `justifyContent`, or `stretch`.
    #[field(default = "start")]
    pub align_content: String,
    /// The room between the children: one length, or the room between rows then between columns (`"8 16"`).
    #[field(default = "0")]
    pub gap: String,
    /// The room inside the edges: one to four lengths, CSS's order (top, right, bottom, left).
    #[field(default = "0")]
    pub padding: String,
}

/// `GuiGridCtrl`: lays its children out in the cells of a grid by the CSS grid rules.
#[derive(EngineClass, Debug, Clone, PartialEq)]
#[class(name = "GuiGridCtrl", parent = "GuiControl")]
pub struct GridCtrl {
    /// The column tracks, left to right: lengths (`120`), percentages (`25%`), shares of the room left (`1fr`), `auto`,
    /// `min-content`, `max-content`, `minmax(80, 1fr)`, `fit-content(200)` and `repeat(3, 1fr)` (or `repeat(auto-fill, 96)`).
    #[field(default = "")]
    pub columns: String,
    /// The row tracks, top to bottom, in the words of `columns`.
    #[field(default = "")]
    pub rows: String,
    /// The size of a column the template does not name (one the auto-flow or a child's `gridColumn` makes).
    #[field(default = "auto")]
    pub auto_columns: String,
    /// The size of a row the template does not name.
    #[field(default = "auto")]
    pub auto_rows: String,
    /// How children without a place fill the grid: `row` (along the rows), `column`, `row dense` or `column dense`.
    #[field(default = "row")]
    pub auto_flow: String,
    /// Where the children sit across their cell's width: `stretch` (they fill it), `start`, `end` or `center`.
    #[field(default = "stretch")]
    pub justify_items: String,
    /// Where the children sit across their cell's height: `stretch`, `start`, `end`, `center` or `baseline`.
    #[field(default = "stretch")]
    pub align_items: String,
    /// Where the columns sit when they leave room: `start`, `end`, `center`, `stretch`, `space-between`, `space-around` or
    /// `space-evenly`.
    #[field(default = "start")]
    pub justify_content: String,
    /// Where the rows sit when they leave room, in the words of `justifyContent`.
    #[field(default = "start")]
    pub align_content: String,
    /// The room between the cells: one length, or the room between rows then between columns (`"8 16"`).
    #[field(default = "0")]
    pub gap: String,
    /// The room inside the edges: one to four lengths, CSS's order (top, right, bottom, left).
    #[field(default = "0")]
    pub padding: String,
}

/// What a container keeps between layouts.
#[derive(Debug, Default)]
pub struct LayoutState {
    /// The fingerprint of what the last layout read ([`layout::fingerprint`]); none before the first.
    fingerprint: Option<u64>,
    /// The extent the last layout was asked for, and the bounds it gave the container (the asked extent may have been raised
    /// to the content's least size).
    asked: (i32, i32),
    placed: ((i32, i32), (i32, i32)),
}

/// A container of either class, read from its fields.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Container {
    Flex(FlexCtrl),
    Grid(GridCtrl),
}

impl Container {
    /// The container `id` is, when it is one.
    pub(crate) fn read(vm: &Vm, id: ObjId) -> Option<Container> {
        if torque_gui::is_a(vm, id, FlexCtrl::NAME) {
            FlexCtrl::read(vm, id).ok().map(Container::Flex)
        } else if torque_gui::is_a(vm, id, GridCtrl::NAME) {
            GridCtrl::read(vm, id).ok().map(Container::Grid)
        } else {
            None
        }
    }
}

/// Whether `id` is a container control (either class).
pub(crate) fn is_container(vm: &Vm, id: ObjId) -> bool {
    torque_gui::is_a(vm, id, FlexCtrl::NAME) || torque_gui::is_a(vm, id, GridCtrl::NAME)
}

fn resize(
    this: Container,
    state: &mut LayoutState,
    cx: &mut ControlCx<'_>,
    position: (i32, i32),
    extent: (i32, i32),
) {
    match layout::root_of(cx.vm, cx.id) {
        None => layout::lay_out(this, state, cx, position, extent),
        // Inside another container's layout: that layout places it, so a resize asks it to lay out again.
        Some(root) => {
            cx.set_bounds_of(cx.id, position, extent);
            let (at, size) = torque_gui::bounds(cx.vm, root);
            // Counted a level deeper, so the walk's depth cap bounds any chain of these.
            cx.resize_control(root, at, size, 1);
        }
    }
}

fn update(this: Container, state: &mut LayoutState, cx: &mut ControlCx<'_>) {
    if layout::root_of(cx.vm, cx.id).is_some() {
        return;
    }
    let print = layout::fingerprint(cx.vm, cx.id, &this);
    if state.fingerprint == Some(print) {
        return;
    }
    let (position, extent) = cx.bounds();
    // An extent a script set since is what is asked now; one the layout gave is asked as before.
    let asked = if (position, extent) == state.placed {
        state.asked
    } else {
        extent
    };
    layout::lay_out(this, state, cx, position, asked);
}

fn wake(this: Container, state: &mut LayoutState, cx: &mut ControlCx<'_>) {
    if layout::root_of(cx.vm, cx.id).is_none() {
        let (position, extent) = cx.bounds();
        layout::lay_out(this, state, cx, position, extent);
    }
}

impl GuiControlClass for FlexCtrl {
    type State = LayoutState;
    const ACTIVE: bool = false;

    fn resize(
        &self,
        state: &mut LayoutState,
        cx: &mut ControlCx<'_>,
        position: (i32, i32),
        extent: (i32, i32),
    ) {
        resize(Container::Flex(self.clone()), state, cx, position, extent);
    }

    fn update(&self, state: &mut LayoutState, cx: &mut ControlCx<'_>) -> Result<(), VmError> {
        update(Container::Flex(self.clone()), state, cx);
        Ok(())
    }

    fn wake(&self, state: &mut LayoutState, cx: &mut ControlCx<'_>) -> Result<(), VmError> {
        wake(Container::Flex(self.clone()), state, cx);
        Ok(())
    }
}

impl GuiControlClass for GridCtrl {
    type State = LayoutState;
    const ACTIVE: bool = false;

    fn resize(
        &self,
        state: &mut LayoutState,
        cx: &mut ControlCx<'_>,
        position: (i32, i32),
        extent: (i32, i32),
    ) {
        resize(Container::Grid(self.clone()), state, cx, position, extent);
    }

    fn update(&self, state: &mut LayoutState, cx: &mut ControlCx<'_>) -> Result<(), VmError> {
        update(Container::Grid(self.clone()), state, cx);
        Ok(())
    }

    fn wake(&self, state: &mut LayoutState, cx: &mut ControlCx<'_>) -> Result<(), VmError> {
        wake(Container::Grid(self.clone()), state, cx);
        Ok(())
    }
}

#[cfg(test)]
mod parse_tests;
