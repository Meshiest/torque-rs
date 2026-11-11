//! GUI control classes written in Rust, added by plugins.
//!
//! A control class is a Rust type that implements [`GuiControlClass`]: its console fields are the type's fields (declared with
//! `#[derive(EngineClass)]`, so a script writes them, `dump()` lists them and they are read back as the struct), and its
//! behavior is the trait's methods: how it draws, how it lays itself out, what it does with the mouse and the keys it is given.
//! What a control keeps between frames that is not a field is its [`State`](GuiControlClass::State).
//!
//! A plugin adds the class while the engine is built ([`GuiBuilderExt::add_gui_control`]); any number of plugins can each add
//! controls. A name two plugins both declare, a name of the engine's own GUI classes and a parent that is not a control class
//! are build errors that name the plugin. The engine's controls (`GuiButtonCtrl` and the rest) are drawn and handled as they
//! always were; a control of a plugin's class is drawn and handled by its class alone, and its children are drawn after it,
//! as a `GuiControl`'s are. An extension's `GuiExtension::draw_control` (deprecated) is still asked for a class no plugin
//! control handles.
//!
//! ```
//! use torque_gui::custom::{GuiBuilderExt, GuiControlClass, Place, PointerEvent, ControlCx};
//! use torque_gui::draw::DrawCtx;
//! use torque_engine::{EngineBuilder, EngineClass, Plugin, PluginId};
//! use torque_ts_vm::vm::VmError;
//!
//! /// A block of one color that tells the script when it is clicked.
//! #[derive(EngineClass)]
//! #[class(name = "GuiTileCtrl", parent = "GuiControl")]
//! struct Tile {
//!     /// The fill.
//!     #[field(default = "255 128 0 255")]
//!     color: [u8; 4],
//! }
//!
//! #[derive(Default)]
//! struct Clicks(u32);
//!
//! impl GuiControlClass for Tile {
//!     type State = Clicks;
//!
//!     fn draw(&self, _clicks: &mut Clicks, d: &mut DrawCtx<'_, '_>, at: &Place) {
//!         d.fill(at.rect(), self.color);
//!     }
//!
//!     fn mouse_down(&self, clicks: &mut Clicks, cx: &mut ControlCx<'_>, _ev: &PointerEvent) -> Result<(), VmError> {
//!         clicks.0 += 1;
//!         cx.call("onClick", &[clicks.0.to_string().into_bytes()])?;
//!         Ok(())
//!     }
//! }
//!
//! #[derive(PluginId)]
//! #[plugin(id = "example.tile")]
//! struct TilePlugin;
//! impl Plugin for TilePlugin {
//!     fn build(&self, engine: &mut EngineBuilder) {
//!         engine.add_gui_control::<Tile>();
//!     }
//! }
//!
//! let engine = EngineBuilder::new().add_plugins(TilePlugin).build()?;
//! assert_eq!(engine.classes().map(|c| c.name).collect::<Vec<_>>(), ["GuiTileCtrl"]);
//! # Ok::<(), torque_engine::BuildError>(())
//! ```
//!
//! The GUI of a client gets the engine's controls from the GUI plugin (`torque.gui`) as it makes the client; a client made
//! without it has none.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use torque_engine::{ClassInfo, EngineBuilder, ScriptClass};
use torque_ts_vm::idhash::IdMap;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use crate::client_script::{RealmLayout, RealmLayoutEntry};
use crate::control::{self, screen_position};
use crate::draw::DrawCtx;
use crate::input::{self, GuiEvent};
use crate::{Gui, specs};

/// A Rust type that is a GUI control class: its fields ([`ScriptClass`], from `#[derive(EngineClass)]`, below
/// `GuiControl` or another plugin's control class) and what its controls do. Every method has `GuiControl`'s behavior as its
/// default.
///
/// The methods get the control's fields as the struct, read when the method is called (a field a script wrote an instant
/// ago is in it), and the control's [`State`](Self::State). A method that can run script returns the script's error, which
/// stops the run as any other would.
pub trait GuiControlClass: ScriptClass {
    /// What a control keeps beside its fields: made with `Default` when the control is made, gone with it.
    type State: Default + 'static;

    /// Whether a new control is active, so it takes the mouse (`GuiControl`'s own is not; a button's is).
    const ACTIVE: bool = true;

    /// Draws the control at `at`, in the walk's clip. The default is `GuiControl`'s: the profile's fill and border. The
    /// control's script `onRender` runs once, before this, and its children are drawn after it.
    fn draw(&self, state: &mut Self::State, d: &mut DrawCtx<'_, '_>, at: &Place) {
        let _ = state;
        d.control_box(at.id, at.offset);
    }

    /// Sets the control's position and extent (the layout of the children included): the default is
    /// [`ControlCx::base_resize`], `GuiControl::resize`. A control that lays its children out itself calls it first.
    fn resize(
        &self,
        state: &mut Self::State,
        cx: &mut ControlCx<'_>,
        position: (i32, i32),
        extent: (i32, i32),
    ) {
        let _ = state;
        cx.base_resize(position, extent);
    }

    /// How far the control's children are drawn from where their positions put them (a scrolling area's offset). The
    /// default is none.
    fn child_shift(state: &Self::State) -> (i32, i32) {
        let _ = state;
        (0, 0)
    }

    /// Once a frame while the control is awake and shown, after the canvas laid its children out and before the frame is
    /// drawn, parents before their children: a control that keeps something in step with its children or fields checks it
    /// here. A control in one of the host's overlays ([`Gui::set_overlay`]) is not updated. An error is printed and the
    /// other controls still update. The default does nothing. What it changes through the context ([`ControlCx`]: bounds set,
    /// children resized) is known to the GUI; a change made through [`ControlCx::vm`] is told with [`ControlCx::changed`].
    fn update(&self, state: &mut Self::State, cx: &mut ControlCx<'_>) -> Result<(), VmError> {
        let _ = (state, cx);
        Ok(())
    }

    /// The control woke (its parents are awake): after `GuiControl::onWake` and the script's `onWake`.
    fn wake(&self, state: &mut Self::State, cx: &mut ControlCx<'_>) -> Result<(), VmError> {
        let _ = (state, cx);
        Ok(())
    }

    /// The control goes to sleep: before the script's `onSleep`.
    fn sleep(&self, state: &mut Self::State, cx: &mut ControlCx<'_>) -> Result<(), VmError> {
        let _ = (state, cx);
        Ok(())
    }

    /// The left button went down on the control (or on it as the control that holds the mouse).
    fn mouse_down(
        &self,
        state: &mut Self::State,
        cx: &mut ControlCx<'_>,
        ev: &PointerEvent,
    ) -> Result<(), VmError> {
        let _ = (state, cx, ev);
        Ok(())
    }

    /// The left button went up, on the control or after [`ControlCx::lock_mouse`].
    fn mouse_up(
        &self,
        state: &mut Self::State,
        cx: &mut ControlCx<'_>,
        ev: &PointerEvent,
    ) -> Result<(), VmError> {
        let _ = (state, cx, ev);
        Ok(())
    }

    /// The mouse moved over the control. After it the control's parent gets the move, as a `GuiControl`'s does.
    fn mouse_move(
        &self,
        state: &mut Self::State,
        cx: &mut ControlCx<'_>,
        ev: &PointerEvent,
    ) -> Result<(), VmError> {
        let _ = (state, cx, ev);
        Ok(())
    }

    /// The mouse moved with the left button down, over the control or after [`ControlCx::lock_mouse`].
    fn mouse_dragged(
        &self,
        state: &mut Self::State,
        cx: &mut ControlCx<'_>,
        ev: &PointerEvent,
    ) -> Result<(), VmError> {
        let _ = (state, cx, ev);
        Ok(())
    }

    /// The mouse came onto the control.
    fn mouse_enter(
        &self,
        state: &mut Self::State,
        cx: &mut ControlCx<'_>,
        ev: &PointerEvent,
    ) -> Result<(), VmError> {
        let _ = (state, cx, ev);
        Ok(())
    }

    /// The mouse left the control.
    fn mouse_leave(
        &self,
        state: &mut Self::State,
        cx: &mut ControlCx<'_>,
        ev: &PointerEvent,
    ) -> Result<(), VmError> {
        let _ = (state, cx, ev);
        Ok(())
    }

    /// A key went down while the control has the key focus. True: the control took it; false: it goes to the parent, as a
    /// `GuiControl`'s does.
    fn key_down(
        &self,
        state: &mut Self::State,
        cx: &mut ControlCx<'_>,
        ev: &KeyEvent,
    ) -> Result<bool, VmError> {
        let _ = (state, cx, ev);
        Ok(false)
    }
}

/// Where a control is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    /// The control.
    pub id: ObjId,
    /// Its top left corner on the canvas.
    pub offset: (i32, i32),
    /// Its width and height.
    pub extent: (i32, i32),
    /// The clip its parent gave it (x, y, width, height).
    pub update: [i32; 4],
    /// How deep in the tree it is.
    pub depth: usize,
}

impl Place {
    /// The control's rectangle on the canvas: x, y, width, height.
    pub fn rect(&self) -> [i32; 4] {
        [self.offset.0, self.offset.1, self.extent.0, self.extent.1]
    }
}

/// A mouse event a control is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerEvent {
    /// The pointer on the canvas.
    pub point: (i32, i32),
    /// The pointer from the control's top left corner.
    pub local: (i32, i32),
    /// The modifier keys held (`SI_SHIFT`, `SI_CTRL`, `SI_ALT` of [`keys`](crate::keys)).
    pub modifier: u8,
    /// How many clicks in a row this press is (1 for a single click).
    pub clicks: u8,
}

/// A key event a control is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// The key's code (a `KEY_*` of [`keys`](crate::keys)).
    pub key_code: u8,
    /// The character the key made, when it made one (0: none).
    pub ascii: u16,
    /// The modifier keys held.
    pub modifier: u8,
}

/// What a control's method reaches: the script engine, the GUI and the control itself.
pub struct ControlCx<'a> {
    /// The script engine.
    pub vm: &'a mut Vm,
    /// The GUI.
    pub gui: &'a Gui,
    /// The control.
    pub id: ObjId,
    depth: usize,
}

impl ControlCx<'_> {
    /// The control's position and extent.
    pub fn bounds(&self) -> ((i32, i32), (i32, i32)) {
        control::bounds(self.vm, self.id)
    }

    /// The control's top left corner on the canvas.
    pub fn screen_position(&self) -> (i32, i32) {
        screen_position(self.vm, self.id)
    }

    /// Says that this control, or one it places, now draws differently from what a draw already made of it saw. Everything the
    /// methods below change is said already; a control that changes something else through [`vm`](Self::vm) (a field a draw
    /// reads) says so here. A client-script layer is drawn again, not kept, after it (`Gui::custom_stamp`).
    pub fn changed(&self) {
        self.gui.touch_custom();
    }

    /// `GuiControl::resize`: the extent clamped to `minExtent`, the children told, then the bounds set.
    pub fn base_resize(&mut self, position: (i32, i32), extent: (i32, i32)) {
        control::base_resize(self.vm, self.gui, self.id, position, extent, self.depth);
        self.changed();
    }

    /// Sets the position and extent of the control `id` (this control or another) and nothing else: its children are not
    /// told and no `minExtent` applies. A control that lays its children out itself uses it for itself and for the
    /// controls it places directly.
    pub fn set_bounds_of(&mut self, id: ObjId, position: (i32, i32), extent: (i32, i32)) {
        // Only a bounds that differ change a draw: a layout that places what is already placed leaves kept draws as they are.
        let same = control::bounds(self.vm, id) == (position, extent);
        control::set_bounds(self.vm, id, position, extent);
        if !same {
            self.changed();
        }
    }

    /// Runs the `resize` of the control `id` (its class's own: a `GuiControl`'s clamps to `minExtent` and tells its
    /// children by their sizing flags), as a control `below` levels under this one (the walk's depth cap counts it).
    pub fn resize_control(
        &mut self,
        id: ObjId,
        position: (i32, i32),
        extent: (i32, i32),
        below: usize,
    ) {
        control::resize_at(self.vm, self.gui, id, position, extent, self.depth + below);
        self.changed();
    }

    /// Runs the `parentResized` of the control `id` (by its `horizSizing` and `vertSizing`) for a parent whose extent went
    /// from `old` to `new`, as a control `below` levels under this one.
    pub fn parent_resized_control(
        &mut self,
        id: ObjId,
        old: (i32, i32),
        new: (i32, i32),
        below: usize,
    ) {
        control::parent_resized_at(self.vm, self.gui, id, old, new, self.depth + below);
        self.changed();
    }

    /// Takes the mouse: every move and release goes to the control until [`unlock_mouse`](Self::unlock_mouse).
    pub fn lock_mouse(&mut self) -> Result<(), VmError> {
        input::mouse_lock(self.vm, self.gui, self.id)
    }

    /// Gives the mouse back.
    pub fn unlock_mouse(&mut self) -> Result<(), VmError> {
        input::mouse_unlock(self.vm, self.gui, self.id)
    }

    /// Calls the script method `name` of the control with `args` (nothing happens when it has none), and returns what it
    /// returned.
    pub fn call(&mut self, name: &str, args: &[Vec<u8>]) -> Result<Vec<u8>, VmError> {
        // A script method may change any field of any control.
        self.changed();
        self.vm.call_method(self.id, name.as_bytes(), args)
    }
}

/// The behavior of one control class, with its state type erased: what the GUI calls. [`Adapter`] makes it of a
/// [`GuiControlClass`].
pub(crate) trait ControlBehavior {
    fn active(&self) -> bool;
    fn new_state(&self) -> Box<dyn Any>;
    fn draw(&self, state: &mut dyn Any, d: &mut DrawCtx<'_, '_>, at: &Place);
    fn resize(
        &self,
        state: &mut dyn Any,
        cx: &mut ControlCx<'_>,
        position: (i32, i32),
        extent: (i32, i32),
    );
    fn child_shift(&self, state: &dyn Any) -> (i32, i32);
    fn update(&self, state: &mut dyn Any, cx: &mut ControlCx<'_>) -> Result<(), VmError>;
    fn wake(&self, state: &mut dyn Any, cx: &mut ControlCx<'_>) -> Result<(), VmError>;
    fn sleep(&self, state: &mut dyn Any, cx: &mut ControlCx<'_>) -> Result<(), VmError>;
    fn pointer(
        &self,
        which: Pointer,
        state: &mut dyn Any,
        cx: &mut ControlCx<'_>,
        ev: &PointerEvent,
    ) -> Result<(), VmError>;
    fn key_down(
        &self,
        state: &mut dyn Any,
        cx: &mut ControlCx<'_>,
        ev: &KeyEvent,
    ) -> Result<bool, VmError>;
}

/// The mouse events a control is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pointer {
    Down,
    Up,
    Move,
    Dragged,
    Enter,
    Leave,
}

struct Adapter<C>(PhantomData<fn() -> C>);

impl<C: GuiControlClass> Adapter<C> {
    /// The control's fields, or the failure printed to the console.
    fn read(vm: &mut Vm, id: ObjId) -> Option<C> {
        match C::read(vm, id) {
            Ok(this) => Some(this),
            Err(error) => {
                vm.error(format!("{}: {error}", C::NAME));
                None
            }
        }
    }
}

impl<C: GuiControlClass> ControlBehavior for Adapter<C> {
    fn active(&self) -> bool {
        C::ACTIVE
    }

    fn new_state(&self) -> Box<dyn Any> {
        Box::<C::State>::default()
    }

    fn draw(&self, state: &mut dyn Any, d: &mut DrawCtx<'_, '_>, at: &Place) {
        // A draw runs no script and cannot print: a control whose fields cannot be read is not drawn.
        if let Some(state) = state.downcast_mut::<C::State>()
            && let Ok(this) = C::read(d.vm(), at.id)
        {
            this.draw(state, d, at);
        }
    }

    fn resize(
        &self,
        state: &mut dyn Any,
        cx: &mut ControlCx<'_>,
        position: (i32, i32),
        extent: (i32, i32),
    ) {
        match (
            state.downcast_mut::<C::State>(),
            Self::read(&mut *cx.vm, cx.id),
        ) {
            (Some(state), Some(this)) => this.resize(state, cx, position, extent),
            _ => cx.base_resize(position, extent),
        }
    }

    fn child_shift(&self, state: &dyn Any) -> (i32, i32) {
        state
            .downcast_ref::<C::State>()
            .map_or((0, 0), |state| C::child_shift(state))
    }

    fn update(&self, state: &mut dyn Any, cx: &mut ControlCx<'_>) -> Result<(), VmError> {
        match (
            state.downcast_mut::<C::State>(),
            Self::read(&mut *cx.vm, cx.id),
        ) {
            (Some(state), Some(this)) => this.update(state, cx),
            _ => Ok(()),
        }
    }

    fn wake(&self, state: &mut dyn Any, cx: &mut ControlCx<'_>) -> Result<(), VmError> {
        match (
            state.downcast_mut::<C::State>(),
            Self::read(&mut *cx.vm, cx.id),
        ) {
            (Some(state), Some(this)) => this.wake(state, cx),
            _ => Ok(()),
        }
    }

    fn sleep(&self, state: &mut dyn Any, cx: &mut ControlCx<'_>) -> Result<(), VmError> {
        match (
            state.downcast_mut::<C::State>(),
            Self::read(&mut *cx.vm, cx.id),
        ) {
            (Some(state), Some(this)) => this.sleep(state, cx),
            _ => Ok(()),
        }
    }

    fn pointer(
        &self,
        which: Pointer,
        state: &mut dyn Any,
        cx: &mut ControlCx<'_>,
        ev: &PointerEvent,
    ) -> Result<(), VmError> {
        let (Some(state), Some(this)) = (
            state.downcast_mut::<C::State>(),
            Self::read(&mut *cx.vm, cx.id),
        ) else {
            return Ok(());
        };
        match which {
            Pointer::Down => this.mouse_down(state, cx, ev),
            Pointer::Up => this.mouse_up(state, cx, ev),
            Pointer::Move => this.mouse_move(state, cx, ev),
            Pointer::Dragged => this.mouse_dragged(state, cx, ev),
            Pointer::Enter => this.mouse_enter(state, cx, ev),
            Pointer::Leave => this.mouse_leave(state, cx, ev),
        }
    }

    fn key_down(
        &self,
        state: &mut dyn Any,
        cx: &mut ControlCx<'_>,
        ev: &KeyEvent,
    ) -> Result<bool, VmError> {
        match (
            state.downcast_mut::<C::State>(),
            Self::read(&mut *cx.vm, cx.id),
        ) {
            (Some(state), Some(this)) => this.key_down(state, cx, ev),
            _ => Ok(false),
        }
    }
}

/// A control class a plugin added: its declaration and its behavior. The list of them the engine holds is what the GUI plugin
/// gives each client's GUI ([`Gui::add_controls`]).
#[derive(Clone)]
pub struct ControlEntry {
    /// The class: its name, parent and fields.
    pub info: ClassInfo,
    behavior: Rc<dyn ControlBehavior>,
}

impl ControlEntry {
    /// The entry of the control class `C`.
    pub fn of<C: GuiControlClass>() -> ControlEntry {
        ControlEntry {
            info: ClassInfo::of::<C>(),
            behavior: Rc::new(Adapter::<C>(PhantomData)),
        }
    }
}

impl fmt::Debug for ControlEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ControlEntry")
            .field("class", &self.info.name)
            .finish_non_exhaustive()
    }
}

/// Adds control classes to an engine being built.
pub trait GuiBuilderExt {
    /// Adds the control class `C`: registered in every client's script engine below its parent, with its fields, and handled
    /// by its [`GuiControlClass`] methods. Build errors ([`BuildError::Rejected`](torque_engine::BuildError::Rejected), or
    /// [`DuplicateScriptName`](torque_engine::BuildError::DuplicateScriptName) when another plugin added a class of the
    /// name, both naming the plugins):
    ///
    /// - the name is one of the engine's own GUI classes (a plugin cannot take over `GuiButtonCtrl`);
    /// - the parent is neither `GuiControl` nor a control class added before (the engine's other GUI classes draw and
    ///   handle input by their own names, so a class below one would lose that).
    fn add_gui_control<C: GuiControlClass>(&mut self) -> &mut Self;

    /// Gives client-script realms their layout containers: the classes (added with [`add_gui_control`](Self::add_gui_control))
    /// that lay a realm's `flex` and `grid` controls out, and where the layout props go. Without one a container is a plain
    /// group. A second one is a build error naming the plugin.
    fn add_realm_layout(&mut self, layout: &'static RealmLayout) -> &mut Self;
}

impl GuiBuilderExt for EngineBuilder {
    fn add_realm_layout(&mut self, layout: &'static RealmLayout) -> &mut Self {
        if !self.list::<RealmLayoutEntry>().is_empty() {
            self.reject("a client-script realm has one layout, and another plugin added it first");
            return self;
        }
        self.push(RealmLayoutEntry::of(layout))
    }

    fn add_gui_control<C: GuiControlClass>(&mut self) -> &mut Self {
        if is_engine_class(C::NAME) {
            self.reject(format!(
                "`{}` is one of the engine's own GUI classes: a control class needs a name of its own",
                C::NAME
            ));
            return self;
        }
        let parent_is_control = C::PARENT.eq_ignore_ascii_case("GuiControl")
            || self
                .list::<ControlEntry>()
                .iter()
                .any(|entry| entry.info.name.eq_ignore_ascii_case(C::PARENT));
        if !parent_is_control {
            self.reject(format!(
                "`{}` goes below `{}`, which is neither GuiControl nor a control class added before it",
                C::NAME,
                C::PARENT
            ));
            return self;
        }
        self.client_script().class::<C>();
        self.push(ControlEntry::of::<C>());
        self
    }
}

/// Whether `name` is a class this crate declares ([`specs::CLASSES`]), without case.
fn is_engine_class(name: &str) -> bool {
    specs::CLASSES
        .iter()
        .any(|class| class.name.eq_ignore_ascii_case(name))
}

/// The control classes of a GUI, and the controls of them that exist.
pub(crate) struct Custom {
    /// By lower case class name.
    classes: HashMap<String, Rc<dyn ControlBehavior>>,
    objects: IdMap<ObjId, Slot>,
    /// What plugins keep on any control ([`Gui::control_data`]), by control and type.
    data: IdMap<ObjId, HashMap<TypeId, Box<dyn Any>>>,
    /// The layout a client-script realm's containers use ([`Gui::add_realm_layouts`]).
    realm_layout: Option<&'static RealmLayout>,
    /// Names the state of the controls as it was last changed: no two values are ever the same, in one GUI or across them.
    stamp: u64,
}

impl Default for Custom {
    fn default() -> Custom {
        Custom {
            classes: HashMap::new(),
            objects: IdMap::default(),
            data: IdMap::default(),
            realm_layout: None,
            stamp: fresh_stamp(),
        }
    }
}

/// A value no earlier call returned.
fn fresh_stamp() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A control of a plugin's class. Its state is out of the slot (`None`) while one of the control's methods runs, so that a
/// call that reaches the same control again finds it busy.
struct Slot {
    behavior: Rc<dyn ControlBehavior>,
    state: Option<Box<dyn Any>>,
}

impl fmt::Debug for Custom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Custom")
            .field("classes", &self.classes.len())
            .field("objects", &self.objects.len())
            .field("data", &self.data.len())
            .finish()
    }
}

impl Gui {
    /// Gives the GUI the control classes `entries` (a class the GUI already has is kept as it was). Their classes are
    /// registered by the script registry, which the engine's client recipes run.
    pub fn add_controls(&self, entries: &[ControlEntry]) {
        let mut state = self.0.borrow_mut();
        for entry in entries {
            state
                .custom
                .classes
                .entry(entry.info.name.to_ascii_lowercase())
                .or_insert_with(|| Rc::clone(&entry.behavior));
        }
    }

    /// Gives the GUI the layout a plugin added for client-script realms (`entries` are the engine's list of them; the first is
    /// kept, as the builder allows one).
    pub fn add_realm_layouts(&self, entries: &[RealmLayoutEntry]) {
        if let Some(entry) = entries.first() {
            self.0.borrow_mut().custom.realm_layout = Some(entry.0);
        }
    }

    /// The layout of client-script realm containers, when a plugin gave one.
    pub(crate) fn realm_layout(&self) -> Option<&'static RealmLayout> {
        self.0.borrow().custom.realm_layout
    }

    /// `f` of the state of type `T` the control `id` keeps (a control of a plugin's class whose
    /// [`State`](GuiControlClass::State) is `T`, not busy).
    pub(crate) fn with_custom_state<T: 'static, R>(
        &self,
        id: ObjId,
        f: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        let mut state = self.0.borrow_mut();
        let slot = state.custom.objects.get_mut(&id)?;
        slot.state.as_mut()?.downcast_mut::<T>().map(f)
    }

    /// `f` of the value of type `T` a plugin keeps on the control `id` (any control, of any class), made with `Default` the
    /// first time and dropped with the control. None when `id` is not a control. `f` runs under a borrow of the GUI: it must
    /// not reach the GUI or run script.
    pub fn control_data<T: Default + 'static, R>(
        &self,
        id: ObjId,
        f: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        let mut state = self.0.borrow_mut();
        if !state.controls.contains_key(&id) {
            return None;
        }
        // `f` may change what a control is drawn from: a client-script layer is drawn again, not kept ([`Gui::custom_stamp`]).
        state.custom.stamp = fresh_stamp();
        let slot = state
            .custom
            .data
            .entry(id)
            .or_default()
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::<T>::default());
        slot.downcast_mut::<T>().map(f)
    }

    /// `f` of the value of type `T` kept on the control `id`, when there is one ([`control_data`](Self::control_data)).
    pub fn peek_control_data<T: 'static, R>(
        &self,
        id: ObjId,
        f: impl FnOnce(&T) -> R,
    ) -> Option<R> {
        let state = self.0.borrow();
        state
            .custom
            .data
            .get(&id)
            .and_then(|kept| kept.get(&TypeId::of::<T>()))
            .and_then(|slot| slot.downcast_ref::<T>())
            .map(f)
    }

    /// The stamp of the plugin controls' state: it differs after every [`touch_custom`](Self::touch_custom), and after any
    /// change a plugin control's own methods make through input or a resize, a new control or a removed one. A draw made at one
    /// stamp is the same draw at that stamp again (given the same canvas, clock and extension).
    pub(crate) fn custom_stamp(&self) -> u64 {
        self.0.borrow().custom.stamp
    }

    /// Something a plugin control draws from changed without one of its methods running (a host that sets its state).
    pub(crate) fn touch_custom(&self) {
        self.0.borrow_mut().custom.stamp = fresh_stamp();
    }

    /// Whether `id` is a control of a class a plugin added.
    pub fn is_custom(&self, id: ObjId) -> bool {
        self.0.borrow().custom.objects.contains_key(&id)
    }

    /// The behavior and state of the control `id` if it is a plugin's, the state taken out until
    /// [`put_custom`](Self::put_custom) (None while a method of the control is running).
    pub(crate) fn take_custom(&self, id: ObjId) -> Option<(Rc<dyn ControlBehavior>, Box<dyn Any>)> {
        let mut state = self.0.borrow_mut();
        let slot = state.custom.objects.get_mut(&id)?;
        let taken = slot.state.take()?;
        Some((Rc::clone(&slot.behavior), taken))
    }

    /// Gives the control its state back (the control may be gone by now).
    pub(crate) fn put_custom(&self, id: ObjId, state: Box<dyn Any>) {
        if let Some(slot) = self.0.borrow_mut().custom.objects.get_mut(&id) {
            slot.state = Some(state);
        }
    }
}

/// A new control `id`: when its class is a plugin's, its slot is made. Whether it starts active.
pub(crate) fn made(vm: &Vm, gui: &Gui, id: ObjId) -> Option<bool> {
    let objects = vm.objects();
    let class = objects.get(id).map(|o| objects.classes.name(o.class()))?;
    let name = String::from_utf8_lossy(class).to_ascii_lowercase();
    let mut state = gui.0.borrow_mut();
    let behavior = Rc::clone(state.custom.classes.get(&name)?);
    let active = behavior.active();
    let slot = Slot {
        state: Some(behavior.new_state()),
        behavior,
    };
    state.custom.objects.insert(id, slot);
    state.custom.stamp = fresh_stamp();
    Some(active)
}

/// The control `id` is removed.
pub(crate) fn removed(gui: &Gui, id: ObjId) {
    let mut state = gui.0.borrow_mut();
    state.custom.objects.remove(&id);
    state.custom.data.remove(&id);
    state.custom.stamp = fresh_stamp();
}

/// `f` with the behavior and state of the control `id` and the script engine, when the control is a plugin's and not busy;
/// None otherwise.
fn with<R>(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    depth: usize,
    f: impl FnOnce(&dyn ControlBehavior, &mut dyn Any, &mut ControlCx<'_>) -> R,
) -> Option<R> {
    let (behavior, mut state) = gui.take_custom(id)?;
    let result = f(
        &*behavior,
        &mut *state,
        &mut ControlCx { vm, gui, id, depth },
    );
    gui.put_custom(id, state);
    Some(result)
}

/// The control's own `resize`; false when it is not a plugin's.
pub(crate) fn resize(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    position: (i32, i32),
    extent: (i32, i32),
    depth: usize,
) -> bool {
    if crate::client_script::defer_layout(vm, gui, id, position, extent) {
        return true;
    }
    let done = with(vm, gui, id, depth, |b, state, cx| {
        b.resize(state, cx, position, extent);
    })
    .is_some();
    if done {
        gui.touch_custom();
    }
    done
}

/// The control's frame update.
pub(crate) fn update(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    with(vm, gui, id, 0, |b, state, cx| b.update(state, cx)).unwrap_or(Ok(()))
}

/// The control woke.
pub(crate) fn wake(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    with(vm, gui, id, 0, |b, state, cx| b.wake(state, cx)).unwrap_or(Ok(()))
}

/// The control goes to sleep.
pub(crate) fn sleep(vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
    with(vm, gui, id, 0, |b, state, cx| b.sleep(state, cx)).unwrap_or(Ok(()))
}

/// A mouse event for the control; None when it is not a plugin's.
pub(crate) fn pointer(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    which: Pointer,
    ev: GuiEvent,
) -> Option<Result<(), VmError>> {
    let at = screen_position(vm, id);
    pointer_at(vm, gui, id, which, ev, at)
}

/// [`pointer`] for a control whose top left corner is at `at` on the canvas (a host that places the control itself, as a
/// scrolling area's children are).
pub(crate) fn pointer_at(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    which: Pointer,
    ev: GuiEvent,
    at: (i32, i32),
) -> Option<Result<(), VmError>> {
    let ev = PointerEvent {
        point: ev.point,
        local: (ev.point.0.wrapping_sub(at.0), ev.point.1.wrapping_sub(at.1)),
        modifier: ev.modifier,
        clicks: ev.clicks,
    };
    // A move changes nothing by itself: the control that is entered or left, pressed or dragged is what draws differently.
    if which != Pointer::Move {
        gui.touch_custom();
    }
    with(vm, gui, id, 0, |b, state, cx| {
        b.pointer(which, state, cx, &ev)
    })
}

/// A key for the control; None when it is not a plugin's.
pub(crate) fn key_down(
    vm: &mut Vm,
    gui: &Gui,
    id: ObjId,
    ev: GuiEvent,
) -> Option<Result<bool, VmError>> {
    let ev = KeyEvent {
        key_code: ev.key_code,
        ascii: ev.ascii,
        modifier: ev.modifier,
    };
    gui.touch_custom();
    with(vm, gui, id, 0, |b, state, cx| b.key_down(state, cx, &ev))
}
