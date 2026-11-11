//! More than one [`GuiExtension`] on a GUI: the primary one ([`Gui::set_extension`]) and a list added after it
//! ([`Gui::add_extension`]), behind one object ([`Gui::ext`]) that answers each method by a rule.
//!
//! The members are asked primary first, then the added ones in the order they were added:
//!
//! | Rule | Methods |
//! |------|---------|
//! | primary only (game-wide) | `fonts`, `begin_draw`, `default_canvas_size`, `key_tables`, `rule`, `missing_bitmap`, `message`, `classic_boxes` |
//! | all, in order (stop at the first `Err`) | `removed`, `before_control`, `scrolled`, `thumb_dragged`, `mouse_pressed`, `focused`, `before_canvas` |
//! | first answer (`Some` or `true`) | `draw_control`, `key_press`, `list_color`, `list_pad`, `text_outline`, `bitmap_fit`, `ml_max_bitmap_height`, `scroll_line_sizes`, `scroll_rects`, `scroll_thumbs` |
//! | any true | `starts_active`, `replaces_fill`, `ml_auto_resize` |
//! | all true | `clip_to_parent` (its answer is true unless a member says no) |
//! | combined | `opacity` (the product of the answers), `shift` (the sum), `layer_stamp` (a hash of the answers, none when any member cannot say) |
//!
//! With nothing added [`Gui::ext`] is the primary itself, so a GUI with one extension pays nothing for the others.
//!
//! A plugin adds extensions while the engine is built ([`GuiExtensions`]): one primary (an exclusive slot, a second one fails
//! the build naming both plugins) and any number of added ones. Each is a factory the GUI plugin calls for every client's GUI.

use std::any::Any;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use torque_engine::{Engine, EngineBuilder};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Vm, VmError};

use crate::draw::DrawCtx;
use crate::ext::{FontSource, GuiExtension, GuiMessage, GuiRule, ListColor, NoExtension};
use crate::scroll::ScrollState;
use crate::{Gui, SharedFonts};

/// The GUI's extensions: the primary one, those added after it, and the one object the GUI asks.
#[derive(Clone)]
pub struct Extension {
    primary: Rc<dyn GuiExtension>,
    added: Vec<Rc<dyn GuiExtension>>,
    /// The primary while nothing is added, else a [`FanOut`] over every member.
    effective: Rc<dyn GuiExtension>,
}

impl Extension {
    fn new(primary: Rc<dyn GuiExtension>) -> Extension {
        Extension {
            effective: Rc::clone(&primary),
            primary,
            added: Vec::new(),
        }
    }

    pub(crate) fn effective(&self) -> &Rc<dyn GuiExtension> {
        &self.effective
    }

    pub(crate) fn set_primary(&mut self, primary: Rc<dyn GuiExtension>) {
        self.primary = primary;
        self.rebuild();
    }

    pub(crate) fn add(&mut self, ext: Rc<dyn GuiExtension>) {
        self.added.push(ext);
        self.rebuild();
    }

    /// The primary, then the added ones in order.
    pub(crate) fn members(&self) -> impl Iterator<Item = &Rc<dyn GuiExtension>> {
        std::iter::once(&self.primary).chain(&self.added)
    }

    fn rebuild(&mut self) {
        self.effective = if self.added.is_empty() {
            Rc::clone(&self.primary)
        } else {
            Rc::new(FanOut {
                members: self.members().cloned().collect(),
            })
        };
    }
}

impl Default for Extension {
    fn default() -> Extension {
        Extension::new(Rc::new(NoExtension))
    }
}

impl std::fmt::Debug for Extension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extension")
            .field("added", &self.added.len())
            .finish_non_exhaustive()
    }
}

/// Every member of an [`Extension`] behind one [`GuiExtension`]; the primary is the first member.
struct FanOut {
    members: Vec<Rc<dyn GuiExtension>>,
}

impl FanOut {
    fn primary(&self) -> &dyn GuiExtension {
        &*self.members[0]
    }

    fn first<T>(&self, mut ask: impl FnMut(&dyn GuiExtension) -> Option<T>) -> Option<T> {
        self.members.iter().find_map(|m| ask(&**m))
    }

    fn any(&self, mut ask: impl FnMut(&dyn GuiExtension) -> bool) -> bool {
        self.members.iter().any(|m| ask(&**m))
    }

    fn each(&self, mut run: impl FnMut(&dyn GuiExtension)) {
        self.members.iter().for_each(|m| run(&**m));
    }

    fn each_try(
        &self,
        mut run: impl FnMut(&dyn GuiExtension) -> Result<(), VmError>,
    ) -> Result<(), VmError> {
        self.members.iter().try_for_each(|m| run(&**m))
    }
}

impl GuiExtension for FanOut {
    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }

    fn fonts(&self) -> Option<Box<dyn FontSource>> {
        self.primary().fonts()
    }

    fn begin_draw(&self, gui: &Gui, fonts: &mut dyn FontSource) {
        self.primary().begin_draw(gui, fonts);
    }

    fn default_canvas_size(&self) -> (i32, i32) {
        self.primary().default_canvas_size()
    }

    fn key_tables(&self) -> crate::keys::KeyTables {
        self.primary().key_tables()
    }

    fn rule(&self, rule: GuiRule) -> bool {
        self.primary().rule(rule)
    }

    fn missing_bitmap(&self) -> Option<&'static str> {
        self.primary().missing_bitmap()
    }

    fn message(&self, message: &GuiMessage<'_>) -> Option<String> {
        self.primary().message(message)
    }

    fn classic_boxes(&self, vm: &Vm, profile: ObjId) -> bool {
        self.primary().classic_boxes(vm, profile)
    }

    fn removed(&self, id: ObjId) {
        self.each(|m| m.removed(id));
    }

    fn before_control(
        &self,
        d: &mut DrawCtx<'_, '_>,
        id: ObjId,
        offset: (i32, i32),
        update: [i32; 4],
    ) {
        self.each(|m| m.before_control(d, id, offset, update));
    }

    fn scrolled(&self, vm: &Vm, gui: &Gui, id: ObjId, moved: (i32, i32)) {
        self.each(|m| m.scrolled(vm, gui, id, moved));
    }

    fn thumb_dragged(&self, gui: &Gui, id: ObjId) {
        self.each(|m| m.thumb_dragged(gui, id));
    }

    fn mouse_pressed(&self, gui: &Gui) {
        self.each(|m| m.mouse_pressed(gui));
    }

    fn focused(&self, vm: &mut Vm, gui: &Gui, id: ObjId) -> Result<(), VmError> {
        self.each_try(|m| m.focused(vm, gui, id))
    }

    fn before_canvas(&self, vm: &mut Vm) -> Result<(), VmError> {
        self.each_try(|m| m.before_canvas(vm))
    }

    #[expect(deprecated, reason = "the fan-out forwards the deprecated seam")]
    fn draw_control(
        &self,
        d: &mut DrawCtx<'_, '_>,
        class: &str,
        id: ObjId,
        offset: (i32, i32),
        update: [i32; 4],
        depth: usize,
    ) -> bool {
        self.any(|m| m.draw_control(d, class, id, offset, update, depth))
    }

    fn key_press(&self, vm: &Vm, id: ObjId) -> bool {
        self.any(|m| m.key_press(vm, id))
    }

    fn list_color(&self, vm: &Vm, profile: ObjId, which: ListColor) -> Option<[u8; 4]> {
        self.first(|m| m.list_color(vm, profile, which))
    }

    fn list_pad(&self, vm: &Vm, profile: &[u8]) -> Option<(i32, i32)> {
        self.first(|m| m.list_pad(vm, profile))
    }

    fn text_outline(&self, vm: &Vm, profile: ObjId) -> Option<[u8; 4]> {
        self.first(|m| m.text_outline(vm, profile))
    }

    fn bitmap_fit(&self, vm: &Vm, id: ObjId) -> Option<(bool, bool, bool)> {
        self.first(|m| m.bitmap_fit(vm, id))
    }

    fn ml_max_bitmap_height(&self, vm: &Vm, id: ObjId) -> Option<i32> {
        self.first(|m| m.ml_max_bitmap_height(vm, id))
    }

    fn scroll_line_sizes(&self, vm: &Vm, id: ObjId) -> Option<(u32, u32)> {
        self.first(|m| m.scroll_line_sizes(vm, id))
    }

    fn scroll_rects(&self, s: &mut ScrollState, ext: (i32, i32), border: i32) -> bool {
        self.any(|m| m.scroll_rects(s, ext, border))
    }

    fn scroll_thumbs(&self, s: &mut ScrollState, constant: bool) -> bool {
        self.any(|m| m.scroll_thumbs(s, constant))
    }

    fn starts_active(&self, vm: &Vm, id: ObjId) -> bool {
        self.any(|m| m.starts_active(vm, id))
    }

    fn replaces_fill(&self, vm: &Vm, profile: ObjId) -> bool {
        self.any(|m| m.replaces_fill(vm, profile))
    }

    fn ml_auto_resize(&self, vm: &Vm, id: ObjId) -> bool {
        self.any(|m| m.ml_auto_resize(vm, id))
    }

    fn clip_to_parent(&self, vm: &Vm, id: ObjId) -> bool {
        self.members.iter().all(|m| m.clip_to_parent(vm, id))
    }

    fn opacity(&self, vm: &Vm, id: ObjId) -> Option<f32> {
        self.members
            .iter()
            .filter_map(|m| m.opacity(vm, id))
            .reduce(|product, answer| product * answer)
    }

    fn shift(&self, gui: &Gui, id: ObjId, now: u32) -> (i32, i32) {
        self.members
            .iter()
            .map(|m| m.shift(gui, id, now))
            .fold((0, 0), |sum, d| (sum.0 + d.0, sum.1 + d.1))
    }

    fn layer_stamp(&self, gui: &Gui, now: u32) -> Option<u64> {
        let mut hasher = DefaultHasher::new();
        for member in &self.members {
            member.layer_stamp(gui, now)?.hash(&mut hasher);
        }
        Some(hasher.finish())
    }
}

/// How an extension is made for a client's GUI: by a plugin's factory, once for each client.
pub type GuiExtensionFactory = dyn Fn(&Gui, &mut Vm) -> Rc<dyn GuiExtension>;

/// The primary extension's factory, the engine's exclusive slot of it.
#[derive(Clone)]
pub struct GuiPrimary(Rc<GuiExtensionFactory>);

/// One added extension's factory, in the engine's list of them.
#[derive(Clone)]
pub struct GuiExtensionEntry(Rc<GuiExtensionFactory>);

/// Adds extensions to the GUI of an engine being built.
pub trait GuiExtensions {
    /// Sets the primary extension: the one that owns the game-wide answers (fonts, canvas size, key tables, rules, messages)
    /// and is asked first for the rest. `make` runs for every client's GUI. A second primary is a build error
    /// ([`BuildError::DuplicateSlot`](torque_engine::BuildError::DuplicateSlot)) naming both plugins.
    fn gui_primary(
        &mut self,
        make: impl Fn(&Gui, &mut Vm) -> Rc<dyn GuiExtension> + 'static,
    ) -> &mut Self;

    /// Adds an extension after the primary and the ones added before it ([`Gui::add_extension`]). Any number of plugins can.
    fn gui_extension(
        &mut self,
        make: impl Fn(&Gui, &mut Vm) -> Rc<dyn GuiExtension> + 'static,
    ) -> &mut Self;
}

impl GuiExtensions for EngineBuilder {
    fn gui_primary(
        &mut self,
        make: impl Fn(&Gui, &mut Vm) -> Rc<dyn GuiExtension> + 'static,
    ) -> &mut Self {
        self.set_slot(Rc::new(GuiPrimary(Rc::new(make))))
    }

    fn gui_extension(
        &mut self,
        make: impl Fn(&Gui, &mut Vm) -> Rc<dyn GuiExtension> + 'static,
    ) -> &mut Self {
        self.push(GuiExtensionEntry(Rc::new(make)))
    }
}

impl Gui {
    /// The extension the GUI asks: the primary itself while nothing was added, else every member behind the rules of
    /// [`crate::fanout`] (held outside the state's borrow while it runs).
    pub fn ext(&self) -> Rc<dyn GuiExtension> {
        Rc::clone(self.0.borrow().ext.effective())
    }

    /// The primary extension or the first added one that is a `T`.
    pub fn extension<T: GuiExtension>(&self) -> Option<Rc<T>> {
        self.0
            .borrow()
            .ext
            .members()
            .find_map(|m| Rc::clone(m).into_any().downcast::<T>().ok())
    }

    /// Sets the primary extension and the layout's font source it gives (if any); the ones added stay.
    pub fn set_extension(&self, ext: Rc<dyn GuiExtension>) {
        let fonts = ext.fonts();
        let mut s = self.0.borrow_mut();
        if s.canvas.is_none() {
            s.canvas_size = ext.default_canvas_size();
        }
        if let Some(f) = fonts {
            s.fonts = SharedFonts::new(f);
        }
        s.ext.set_primary(ext);
    }

    /// Adds an extension after the primary and the ones added before it. Its game-wide answers (fonts, canvas size, key
    /// tables, rules, messages) are not asked: they are the primary's.
    pub fn add_extension(&self, ext: Rc<dyn GuiExtension>) {
        self.0.borrow_mut().ext.add(ext);
    }

    /// Applies the extensions the plugins of `engine` added ([`GuiExtensions`]) to this GUI: the primary's factory first (it
    /// becomes the primary, [`set_extension`](Self::set_extension)), then each added one's in the order the plugins added them
    /// ([`add_extension`](Self::add_extension)). The GUI plugin calls it for every client's GUI; a game with a host of its own
    /// calls it on the GUI it makes, after [`register`](crate::register), to get the same extensions.
    pub fn install_extensions(&self, vm: &mut Vm, engine: &Engine) {
        if let Some(primary) = engine.slot::<GuiPrimary>() {
            let ext = (primary.0)(self, vm);
            self.set_extension(ext);
        }
        for GuiExtensionEntry(make) in engine.list::<GuiExtensionEntry>() {
            let ext = make(self, vm);
            self.add_extension(ext);
        }
    }
}

#[cfg(test)]
#[path = "fanout_tests.rs"]
mod tests;
