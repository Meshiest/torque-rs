//! What a layout plugin gives a realm: the classes of its containers and where the layout props are written.
//!
//! The realm's `flex` and `grid` kinds are laid out by control classes that live in a plugin (this crate cannot depend on it).
//! The plugin registers a [`RealmLayout`] while the engine is built ([`GuiBuilderExt::add_realm_layout`]), and the GUI plugin
//! hands it to each client's GUI ([`Gui::add_realm_layouts`]). The applier checks every prop against the format first, so the
//! plugin sees only values inside the format's ranges, as the words of CSS ([`field_text`](torque_client_script_format::ui::field_text)),
//! written to console fields of the plugin's own names. Without one, a container is a plain group that places nothing.

use torque_client_script_format::ui::{Container, Prop};

/// The layout classes a plugin lays a realm's containers out with, and the console field each layout prop goes to.
#[derive(Debug)]
pub struct RealmLayout {
    flex: &'static str,
    grid: &'static str,
    fields: &'static [(Prop, &'static str)],
    size: (&'static str, &'static str),
}

impl RealmLayout {
    /// A layout whose flex containers are objects of class `flex` and grid containers of class `grid` (both control classes
    /// the plugin added). `fields` names the console field of each container prop and item prop it reads (a prop not listed is
    /// not written; the placement props, `pos`, `size`, `hsize`, `vsize` and `visible`, are the GUI's own). `size` names the
    /// two fields a container's `size` is also written to, as its width and its height, so that a container inside another
    /// is laid out at it rather than at its content's size.
    #[must_use]
    pub const fn new(
        flex: &'static str,
        grid: &'static str,
        fields: &'static [(Prop, &'static str)],
        size: (&'static str, &'static str),
    ) -> RealmLayout {
        RealmLayout {
            flex,
            grid,
            fields,
            size,
        }
    }

    /// The class of the containers of `container`.
    pub(crate) fn class(&self, container: Container) -> &'static str {
        match container {
            Container::Flex => self.flex,
            Container::Grid => self.grid,
        }
    }

    /// The console field `prop` is written to.
    pub(crate) fn field(&self, prop: Prop) -> Option<&'static str> {
        self.fields
            .iter()
            .find_map(|&(p, name)| (p == prop).then_some(name))
    }

    /// The fields a container's size is written to: width, then height.
    pub(crate) fn size_fields(&self) -> (&'static str, &'static str) {
        self.size
    }
}

/// A layout plugin's registration, as the engine's list of them holds it (`Gui::add_realm_layouts` takes the list).
#[derive(Clone, Copy, Debug)]
pub struct RealmLayoutEntry(pub(crate) &'static RealmLayout);

impl RealmLayoutEntry {
    /// The entry of `layout`.
    pub fn of(layout: &'static RealmLayout) -> RealmLayoutEntry {
        RealmLayoutEntry(layout)
    }
}
