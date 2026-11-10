//! [`Plugin`], [`PluginType`] (a plugin type's stable [`PluginId`](struct@PluginId)), [`PluginSet`] and [`Requires`].

use std::fmt;

use crate::EngineBuilder;

/// A plugin's stable name, such as `my_game::HudPlugin` or `torque.gamepad`.
///
/// The id is a plugin's identity: duplicate checks, ordering, config files (which name plugins to turn on or off) and
/// peers (which compare the plugins that have a [`net_version`](Plugin::net_version)) all go by it. The typed calls
/// (`disable::<P>()`, ...) look up `P`'s id ([`PluginType::ID`]). It never comes from `std::any::type_name`, which can
/// change between compiler versions. It is ASCII letters, digits, `_`, `-`, `.` and
/// `:`, never empty; [`PluginId::new`] checks that, so a bad id in a `const` fails to compile.
///
/// ```
/// use torque_engine::PluginId;
///
/// const GAMEPAD: PluginId = PluginId::new("torque.gamepad");
/// assert_eq!(GAMEPAD.as_str(), "torque.gamepad");
/// assert!(PluginId::is_valid("my_game::HudPlugin"));
/// assert!(!PluginId::is_valid("torque gamepad"));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PluginId(&'static str);

impl PluginId {
    /// The id `id`.
    ///
    /// # Panics
    ///
    /// When `id` is not a valid id ([`PluginId::is_valid`]); in a `const`, that is a compile error.
    pub const fn new(id: &'static str) -> PluginId {
        assert!(
            PluginId::is_valid(id),
            "a plugin id is ASCII letters, digits, '_', '-', '.' and ':', and not empty"
        );
        PluginId(id)
    }

    /// The id `id`, or `None` when it is not a valid id. For text that is not `'static`, check it with
    /// [`is_valid`](Self::is_valid) first, or use [`leak`](Self::leak), which checks before it copies.
    pub const fn try_new(id: &'static str) -> Option<PluginId> {
        if PluginId::is_valid(id) {
            Some(PluginId(id))
        } else {
            None
        }
    }

    /// The id `id`, for text known only at run time (the id a loaded extension reports): checked first, and only a
    /// valid id is copied and leaked to make it `'static`, so call it once per id (once per loaded extension).
    pub fn leak(id: &str) -> Option<PluginId> {
        if !PluginId::is_valid(id) {
            return None;
        }
        Some(PluginId(Box::leak(id.into())))
    }

    /// The id as text.
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// Whether `id` is a valid id: not empty, and only ASCII letters, digits, `_`, `-`, `.` and `:`.
    pub const fn is_valid(id: &str) -> bool {
        let bytes = id.as_bytes();
        if bytes.is_empty() {
            return false;
        }
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if !(b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.' || b == b':') {
                return false;
            }
            i += 1;
        }
        true
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl fmt::Debug for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PluginId({:?})", self.0)
    }
}

impl PartialEq<str> for PluginId {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for PluginId {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

/// A part of the engine or of a game: it records what it adds (script recipes for the servers and clients, resources,
/// slots, list entries, other plugins) into the [`EngineBuilder`] when the engine is built.
///
/// [`build`](Plugin::build) runs once per engine, in the engine's plugin order; then [`finish`](Plugin::finish) runs
/// for every plugin in the same order, after every `build`, so a plugin can read what the others registered. The
/// recipes recorded there run every time a server or client script engine is made, so one engine can start a server
/// many times.
///
/// Every plugin type also has a stable id ([`PluginType`]), from `#[derive(PluginId)]` or written by hand. The engine
/// keeps plugins as `dyn Plugin` and reads the id when one is added: [`id`](Plugin::id), or else the type's.
///
/// Plugins carry their options as fields; the run-time inputs of one start (files, sockets, globals, scripts) are not
/// plugin state.
///
/// ```
/// use torque_engine::{ClientStage, EngineBuilder, Plugin, PluginId, PluginType};
///
/// /// Shows the player's name on a client.
/// #[derive(PluginId)]
/// #[plugin(id = "example.name")]
/// struct NamePlugin {
///     name: String,
/// }
///
/// impl Plugin for NamePlugin {
///     fn build(&self, engine: &mut EngineBuilder) {
///         let name = self.name.clone();
///         engine.client(ClientStage::Natives, move |vm, _engine| {
///             vm.set_global_text(b"$Pref::Player::Name", name.as_bytes());
///             Ok(())
///         });
///     }
/// }
///
/// let engine = EngineBuilder::new().add_plugins(NamePlugin { name: "Ada".into() }).build()?;
/// assert_eq!(engine.plugins(), [NamePlugin::ID]);
/// # Ok::<(), torque_engine::BuildError>(())
/// ```
pub trait Plugin: 'static {
    /// This plugin's own id, when it is not its type's ([`PluginType::ID`], the default `None`): an adapter type that
    /// stands for many loaded extensions returns the id each one reports. It is read once, when the plugin is added,
    /// and checked: an invalid id is a [`Warning::InvalidId`](crate::Warning::InvalidId) and the plugin is not added; a
    /// valid one is kept with [`PluginId::leak`].
    fn id(&self) -> Option<&str> {
        None
    }

    /// Names the plugins this one needs ([`Requires::plugin`], [`Requires::id`]). Each builds before this one. A type
    /// not in the engine yet is added as its `Default`. When a required plugin is disabled, this one is turned off with
    /// a [`Warning::RequiresDisabled`](crate::Warning::RequiresDisabled); when it is named by an id no plugin has, with
    /// a [`Warning::RequiresMissing`](crate::Warning::RequiresMissing).
    fn requires(&self, _requires: &mut Requires) {}

    /// `Some(version)` when the plugin changes what goes over the network: a server and a client must then both have
    /// it, at the same version. `None` (the default) for a plugin that only one side needs.
    fn net_version(&self) -> Option<u32> {
        None
    }

    /// Records what the plugin adds. It may add other plugins ([`EngineBuilder::add_plugins`]): they build at once,
    /// before the rest of this `build`.
    fn build(&self, engine: &mut EngineBuilder);

    /// Runs after every plugin's [`build`](Plugin::build): reads, and may add to, what the others registered.
    fn finish(&self, _engine: &mut EngineBuilder) {}
}

/// A plugin type with its stable id: what the engine's typed calls (`add_plugins`, `disable::<P>()`,
/// `add_before::<P>`, [`Requires::plugin`], ...) take the id from.
///
/// `#[derive(PluginId)]` implements it. The id is the package's name (with `-` as `_`) and the type's, as a path:
/// `my_game::HudPlugin` for `HudPlugin` in the package `my-game`; `#[plugin(id = "torque.audio")]` gives
/// another. The derive finds this crate in the deriving crate's manifest (`torque-engine`, or
/// `torque` as `torque::engine`); `#[plugin(crate = path)]` names it otherwise. Generic types are refused (each instance would share one id). By hand:
///
/// ```
/// use torque_engine::{EngineBuilder, Plugin, PluginId, PluginType};
///
/// struct AudioPlugin;
///
/// impl PluginType for AudioPlugin {
///     const ID: PluginId = PluginId::new("torque.audio");
/// }
///
/// impl Plugin for AudioPlugin {
///     fn build(&self, _engine: &mut EngineBuilder) {}
/// }
/// # assert_eq!(AudioPlugin::ID.as_str(), "torque.audio");
/// ```
pub trait PluginType: Plugin {
    /// The id: stable across builds, named by config files and seen by peers. Two plugin types in one engine never
    /// share one.
    const ID: PluginId;
}

/// A required plugin: its id, and how to make its default when the engine has none (`None` when it is named only by id).
pub(crate) struct Required {
    pub(crate) id: String,
    pub(crate) make: Option<MakeDefault>,
}

/// Makes a required plugin's default, with its id.
pub(crate) type MakeDefault = fn() -> (Result<PluginId, String>, Box<dyn Plugin>);

/// One plugin type or a tuple of them (nested too, up to 12 each): what the typed `disable` and `enable` take.
///
/// `disable::<AudioPlugin>()` and `disable::<(AudioPlugin, (GizmoPlugin, ZoomPlugin))>()` both work.
pub trait PluginSet {
    /// Appends the ids of these plugin types, in order.
    #[doc(hidden)]
    fn ids(out: &mut Vec<PluginId>);
}

impl<P: PluginType> PluginSet for P {
    fn ids(out: &mut Vec<PluginId>) {
        out.push(P::ID);
    }
}

macro_rules! tuple_sets {
    ($($s:ident),*) => {
        impl<$($s: PluginSet),*> PluginSet for ($($s,)*) {
            fn ids(out: &mut Vec<PluginId>) {
                $($s::ids(out);)*
            }
        }
    };
}

tuple_sets!(S1);
tuple_sets!(S1, S2);
tuple_sets!(S1, S2, S3);
tuple_sets!(S1, S2, S3, S4);
tuple_sets!(S1, S2, S3, S4, S5);
tuple_sets!(S1, S2, S3, S4, S5, S6);
tuple_sets!(S1, S2, S3, S4, S5, S6, S7);
tuple_sets!(S1, S2, S3, S4, S5, S6, S7, S8);
tuple_sets!(S1, S2, S3, S4, S5, S6, S7, S8, S9);
tuple_sets!(S1, S2, S3, S4, S5, S6, S7, S8, S9, S10);
tuple_sets!(S1, S2, S3, S4, S5, S6, S7, S8, S9, S10, S11);
tuple_sets!(S1, S2, S3, S4, S5, S6, S7, S8, S9, S10, S11, S12);

/// The ids of the plugin types `S`.
pub(crate) fn ids_of<S: PluginSet>() -> Vec<PluginId> {
    let mut ids = Vec::new();
    S::ids(&mut ids);
    ids
}

/// The plugins a plugin needs: what [`Plugin::requires`] fills in.
///
/// ```
/// use torque_engine::{EngineBuilder, Plugin, PluginId, PluginType, Requires};
///
/// #[derive(Default, PluginId)]
/// struct Gui;
/// impl Plugin for Gui {
///     fn build(&self, _engine: &mut EngineBuilder) {}
/// }
///
/// #[derive(PluginId)]
/// struct Menu;
/// impl Plugin for Menu {
///     fn requires(&self, requires: &mut Requires) {
///         requires.plugin::<Gui>();
///     }
///     fn build(&self, _engine: &mut EngineBuilder) {}
/// }
///
/// // `Gui` was not added: it is added as its default, and builds first.
/// let engine = EngineBuilder::new().add_plugins(Menu).build()?;
/// assert_eq!(engine.plugins(), [Gui::ID, Menu::ID]);
/// # Ok::<(), torque_engine::BuildError>(())
/// ```
#[derive(Default)]
pub struct Requires {
    pub(crate) list: Vec<Required>,
}

impl Requires {
    /// Requires the plugin `P`: added as `P::default()` when the engine has no `P`, and built first either way.
    pub fn plugin<P: PluginType + Default>(&mut self) -> &mut Requires {
        self.list.push(Required {
            id: P::ID.as_str().to_owned(),
            make: Some(|| {
                let plugin = P::default();
                (crate::group::id_of(&plugin), Box::new(plugin))
            }),
        });
        self
    }

    /// Requires the plugin with the id `id`, for a plugin that knows the others only by id (a loaded extension). It
    /// builds first; when no plugin has the id, the requiring plugin is turned off with a
    /// [`Warning::RequiresMissing`](crate::Warning::RequiresMissing).
    pub fn id(&mut self, id: &str) -> &mut Requires {
        self.list.push(Required {
            id: id.to_owned(),
            make: None,
        });
        self
    }
}

impl fmt::Debug for Requires {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.list.iter().map(|r| &r.id))
            .finish()
    }
}
