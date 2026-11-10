//! [`PluginGroup`], [`PluginGroupBuilder`] and [`Plugins`], what [`EngineBuilder::add_plugins`] takes.
//!
//! [`EngineBuilder::add_plugins`]: crate::EngineBuilder::add_plugins

use std::fmt;

use crate::plugin::ids_of;
use crate::{Plugin, PluginId, PluginSet, PluginType, Warning};

/// One plugin in a list, with whether it is on.
pub(crate) struct Entry {
    pub(crate) id: PluginId,
    /// `None` only while its own `build` or `finish` runs.
    pub(crate) plugin: Option<Box<dyn Plugin>>,
    pub(crate) enabled: bool,
}

impl Entry {
    pub(crate) fn new(id: PluginId, plugin: Box<dyn Plugin>) -> Entry {
        Entry {
            id,
            plugin: Some(plugin),
            enabled: true,
        }
    }
}

/// A named set of plugins added together, such as the engine's defaults.
///
/// A group lists its plugins in order; [`set`](PluginGroup::set) swaps one for a differently configured one of the same
/// type, [`disable`](PluginGroup::disable) and [`enable`](PluginGroup::enable) turn one off or on by its type. A
/// disabled plugin stays in the list, so a later [`EngineBuilder::enable`](crate::EngineBuilder::enable) or a config
/// file ([`EngineBuilder::enable_id`](crate::EngineBuilder::enable_id)) can turn it back on.
///
/// ```
/// use torque_engine::{EngineBuilder, Plugin, PluginGroup, PluginGroupBuilder, PluginId, PluginType};
///
/// #[derive(PluginId)]
/// struct Sky;
/// impl Plugin for Sky {
///     fn build(&self, _engine: &mut EngineBuilder) {}
/// }
/// #[derive(PluginId)]
/// struct Rain;
/// impl Plugin for Rain {
///     fn build(&self, _engine: &mut EngineBuilder) {}
/// }
///
/// /// The weather: a sky, and rain that a game turns on when it wants it.
/// struct WeatherPlugins;
/// impl PluginGroup for WeatherPlugins {
///     fn build(self) -> PluginGroupBuilder {
///         PluginGroupBuilder::new().add(Sky).add(Rain).disable::<Rain>()
///     }
/// }
///
/// let engine = EngineBuilder::new().add_plugins(WeatherPlugins).build()?;
/// assert_eq!(engine.plugins(), [Sky::ID]);
///
/// let engine = EngineBuilder::new().add_plugins(WeatherPlugins).enable::<Rain>().build()?;
/// assert_eq!(engine.plugins(), [Sky::ID, Rain::ID]);
/// # Ok::<(), torque_engine::BuildError>(())
/// ```
pub trait PluginGroup: Sized + 'static {
    /// The group's plugins, in order.
    fn build(self) -> PluginGroupBuilder;

    /// The group with its plugin of type `P` replaced by `plugin`: see [`PluginGroupBuilder::set`].
    fn set<P: PluginType>(self, plugin: P) -> PluginGroupBuilder {
        self.build().set(plugin)
    }

    /// The group with its plugins of the types `S` turned off: see [`PluginGroupBuilder::disable`].
    fn disable<S: PluginSet>(self) -> PluginGroupBuilder {
        self.build().disable::<S>()
    }

    /// The group with its plugins of the types `S` turned on: see [`PluginGroupBuilder::enable`].
    fn enable<S: PluginSet>(self) -> PluginGroupBuilder {
        self.build().enable::<S>()
    }
}

/// The plugins of a [`PluginGroup`], in order, with their on or off state and ordering constraints.
///
/// The order is the add order, except where a constraint moves a plugin: [`add_before`](Self::add_before) and
/// [`add_after`](Self::add_after) here, and [`Plugin::requires`] (a required plugin comes first). When the engine is
/// built, it walks the list in add order and, before placing a plugin, places every plugin that must come before it.
/// A constraint naming a plugin that is absent or disabled is ignored.
///
/// The changes that name a plugin by type ([`set`](Self::set), [`disable`](Self::disable), [`enable`](Self::enable))
/// apply to the plugins already in this group; a type that is not there is skipped with a
/// [`Warning::UnknownPlugin`].
#[derive(Default)]
pub struct PluginGroupBuilder {
    pub(crate) entries: Vec<Entry>,
    /// `(first, then)`: `first` builds before `then`.
    pub(crate) order: Vec<(String, String)>,
    pub(crate) warnings: Vec<Warning>,
}

impl PluginGroupBuilder {
    /// An empty group.
    pub fn new() -> PluginGroupBuilder {
        PluginGroupBuilder::default()
    }

    /// Adds `plugin` at the end, enabled.
    #[allow(clippy::should_implement_trait, reason = "a builder's add")]
    pub fn add<P: PluginType>(mut self, plugin: P) -> PluginGroupBuilder {
        self.push(plugin);
        self
    }

    /// Adds `plugin` at the end, to build before the plugin of type `Target` (wherever that is in the engine's list):
    /// `add_before::<GuiPlugin>(MenuPlugin)`.
    pub fn add_before<Target: PluginType>(self, plugin: impl PluginType) -> PluginGroupBuilder {
        self.add_before_id(Target::ID.as_str(), plugin)
    }

    /// Adds `plugin` at the end, to build after the plugin of type `Target` (wherever that is in the engine's list).
    pub fn add_after<Target: PluginType>(self, plugin: impl PluginType) -> PluginGroupBuilder {
        self.add_after_id(Target::ID.as_str(), plugin)
    }

    /// [`add_before`](Self::add_before) naming the target by id, for a plugin that knows it only so.
    pub fn add_before_id(mut self, target: &str, plugin: impl PluginType) -> PluginGroupBuilder {
        if let Some(id) = self.push(plugin) {
            self.order.push((id.as_str().to_owned(), target.to_owned()));
        }
        self
    }

    /// [`add_after`](Self::add_after) naming the target by id, for a plugin that knows it only so.
    pub fn add_after_id(mut self, target: &str, plugin: impl PluginType) -> PluginGroupBuilder {
        if let Some(id) = self.push(plugin) {
            self.order.push((target.to_owned(), id.as_str().to_owned()));
        }
        self
    }

    /// Adds every plugin of `group` at the end, in its order, with its states and constraints.
    pub fn add_group(mut self, group: impl PluginGroup) -> PluginGroupBuilder {
        self.append(group.build());
        self
    }

    /// Replaces the plugin with `plugin`'s id with `plugin`, in the same place and state.
    pub fn set<P: PluginType>(mut self, plugin: P) -> PluginGroupBuilder {
        let id = match id_of(&plugin) {
            Ok(id) => id,
            Err(id) => {
                self.warnings.push(Warning::InvalidId { id });
                return self;
            }
        };
        match self.entries.iter_mut().find(|entry| entry.id == id) {
            Some(entry) => entry.plugin = Some(Box::new(plugin)),
            None => self.warnings.push(Warning::UnknownPlugin {
                id: id.as_str().to_owned(),
                action: "set",
            }),
        }
        self
    }

    /// Turns the plugins of the types `S` (one type or a tuple) off: they do not build, and their recipes never run.
    pub fn disable<S: PluginSet>(mut self) -> PluginGroupBuilder {
        for id in ids_of::<S>() {
            self.set_enabled(id, false, "disable");
        }
        self
    }

    /// Turns the plugins of the types `S` (one type or a tuple) back on.
    pub fn enable<S: PluginSet>(mut self) -> PluginGroupBuilder {
        for id in ids_of::<S>() {
            self.set_enabled(id, true, "enable");
        }
        self
    }

    /// The ids of the plugins in the group, in add order, disabled ones too.
    pub fn ids(&self) -> impl ExactSizeIterator<Item = PluginId> + '_ {
        self.entries.iter().map(|entry| entry.id)
    }

    /// Adds `plugin`, unless the id it reports is invalid (a warning).
    pub(crate) fn push<P: PluginType>(&mut self, plugin: P) -> Option<PluginId> {
        match id_of(&plugin) {
            Ok(id) => {
                self.entries.push(Entry::new(id, Box::new(plugin)));
                Some(id)
            }
            Err(id) => {
                self.warnings.push(Warning::InvalidId { id });
                None
            }
        }
    }

    pub(crate) fn append(&mut self, other: PluginGroupBuilder) {
        self.entries.extend(other.entries);
        self.order.extend(other.order);
        self.warnings.extend(other.warnings);
    }

    fn set_enabled(&mut self, id: PluginId, enabled: bool, action: &'static str) {
        match self.entries.iter_mut().find(|entry| entry.id == id) {
            Some(entry) => entry.enabled = enabled,
            None => self.warnings.push(Warning::UnknownPlugin {
                id: id.as_str().to_owned(),
                action,
            }),
        }
    }
}

/// `plugin`'s id: its own ([`Plugin::id`]) when valid (else the text, as the error), or its type's.
pub(crate) fn id_of<P: PluginType>(plugin: &P) -> Result<PluginId, String> {
    match plugin.id() {
        None => Ok(P::ID),
        Some(id) if P::ID == id => Ok(P::ID),
        Some(id) => PluginId::leak(id).ok_or_else(|| id.to_owned()),
    }
}

impl PluginGroup for PluginGroupBuilder {
    fn build(self) -> PluginGroupBuilder {
        self
    }
}

impl fmt::Debug for PluginGroupBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginGroupBuilder")
            .field(
                "plugins",
                &self
                    .entries
                    .iter()
                    .map(|entry| (entry.id, entry.enabled))
                    .collect::<Vec<_>>(),
            )
            .field("order", &self.order)
            .finish_non_exhaustive()
    }
}

/// What [`EngineBuilder::add_plugins`](crate::EngineBuilder::add_plugins) takes: a plugin (a [`PluginType`]), a [`PluginGroup`], or a
/// tuple of up to 12 of those (nested tuples too). The `Marker` parameter only tells the three apart; callers never
/// name it.
pub trait Plugins<Marker> {
    /// Appends these plugins to `list`, in order.
    #[doc(hidden)]
    fn add_to(self, list: &mut PluginGroupBuilder);
}

mod sealed {
    /// [`Plugins`](super::Plugins)' marker for one plugin.
    pub struct PluginMarker;
    /// [`Plugins`](super::Plugins)' marker for a group.
    pub struct GroupMarker;
    /// [`Plugins`](super::Plugins)' marker for a tuple.
    pub struct TupleMarker;
}

impl<P: PluginType> Plugins<sealed::PluginMarker> for P {
    fn add_to(self, list: &mut PluginGroupBuilder) {
        list.push(self);
    }
}

impl<G: PluginGroup> Plugins<sealed::GroupMarker> for G {
    fn add_to(self, list: &mut PluginGroupBuilder) {
        list.append(self.build());
    }
}

macro_rules! tuple_plugins {
    ($(($p:ident, $m:ident)),*) => {
        impl<$($p: Plugins<$m>, $m),*> Plugins<(sealed::TupleMarker, $($m,)*)> for ($($p,)*) {
            #[allow(non_snake_case)]
            fn add_to(self, list: &mut PluginGroupBuilder) {
                let ($($p,)*) = self;
                $($p.add_to(list);)*
            }
        }
    };
}

tuple_plugins!((P1, M1));
tuple_plugins!((P1, M1), (P2, M2));
tuple_plugins!((P1, M1), (P2, M2), (P3, M3));
tuple_plugins!((P1, M1), (P2, M2), (P3, M3), (P4, M4));
tuple_plugins!((P1, M1), (P2, M2), (P3, M3), (P4, M4), (P5, M5));
tuple_plugins!((P1, M1), (P2, M2), (P3, M3), (P4, M4), (P5, M5), (P6, M6));
tuple_plugins!(
    (P1, M1),
    (P2, M2),
    (P3, M3),
    (P4, M4),
    (P5, M5),
    (P6, M6),
    (P7, M7)
);
tuple_plugins!(
    (P1, M1),
    (P2, M2),
    (P3, M3),
    (P4, M4),
    (P5, M5),
    (P6, M6),
    (P7, M7),
    (P8, M8)
);
tuple_plugins!(
    (P1, M1),
    (P2, M2),
    (P3, M3),
    (P4, M4),
    (P5, M5),
    (P6, M6),
    (P7, M7),
    (P8, M8),
    (P9, M9)
);
tuple_plugins!(
    (P1, M1),
    (P2, M2),
    (P3, M3),
    (P4, M4),
    (P5, M5),
    (P6, M6),
    (P7, M7),
    (P8, M8),
    (P9, M9),
    (P10, M10)
);
tuple_plugins!(
    (P1, M1),
    (P2, M2),
    (P3, M3),
    (P4, M4),
    (P5, M5),
    (P6, M6),
    (P7, M7),
    (P8, M8),
    (P9, M9),
    (P10, M10),
    (P11, M11)
);
tuple_plugins!(
    (P1, M1),
    (P2, M2),
    (P3, M3),
    (P4, M4),
    (P5, M5),
    (P6, M6),
    (P7, M7),
    (P8, M8),
    (P9, M9),
    (P10, M10),
    (P11, M11),
    (P12, M12)
);
