//! Debug gizmos: lines, boxes, spheres, arrows, crosses and text labels drawn in world space, for one frame or a number of seconds,
//! in a color, depth tested or not. [`GizmosPlugin`] is not in [`DefaultPlugins`](crate::plugins::DefaultPlugins); a game adds it
//! while it is debugging and builds without the `gizmos` Cargo feature when it ships.
//!
//! | Plugin | Id | Cargo feature | What it adds |
//! |--------|----|---------------|--------------|
//! | [`GizmosPlugin`] | `torque.gizmos` | `gizmos` | each client's [`Gizmos`] and the console functions below |
//!
//! **From Rust**, a client's [`Gizmos`] (`Gizmos::of(game.vm())`, or [`GameClient::gizmos`](crate::client::GameClient::gizmos)) is
//! a set you call each time a shape should be drawn: `gizmos.sphere(center, radius, Style::new(color).seconds(2.0))`. See
//! [`Gizmos`] and [`Style`].
//!
//! **From scripts**, with `color` as `"r g b"` or `"r g b a"` (floats, 0 to 1), `seconds` as a float (0 or left out: one frame)
//! and `depth` as a number (0: drawn over everything; left out: hidden behind the scene):
//!
//! | Function | Draws |
//! |----------|-------|
//! | `drawDebugLine(start, end, color [, seconds [, depth]])` | a line |
//! | `drawDebugBox(min, max, color [, seconds [, depth]])` | the edges of an axis-aligned box |
//! | `drawDebugOrientedBox(center, halfExtents, rotation, color [, seconds [, depth]])` | the edges of a box turned by `"x y z angle"` (radians) |
//! | `drawDebugSphere(center, radius, color [, seconds [, depth]])` | a sphere's outline |
//! | `drawDebugArrow(start, end, color [, seconds [, depth]])` | an arrow, its head at `end` |
//! | `drawDebugCross(center, size, color [, seconds [, depth]])` | three lines through a point |
//! | `drawDebugText(position, text, color [, seconds [, depth]])` | a label above a point (the depth switch is ignored: labels are never hidden) |
//! | `clearDebug()` | everything drawn so far |
//!
//! **Drawing.** The view that draws the client ([`WorldView`](crate::view::WorldView), attached to the client as the window and
//! the offscreen runners do) draws the lines in one batched draw over the finished world, then the labels as glyph quads of the
//! GUI's fonts (the system's faces, `Arial` 14 unless [`GizmoSettings`] says otherwise), then the GUI. A depth-tested line is
//! hidden where the engine's renderer drew something nearer; with another renderer on the slot every line is drawn over the
//! picture. Each drawn frame ages the shapes by its time; a view drawing at a fixed step ([`shots`](crate::shots)) ages them by
//! that step. A set with nothing in it costs nothing on the GPU, and a client made without the plugin has none.
//!
//! **Caps.** At most [`GizmoSettings::max_lines`] segments (a box is twelve, a sphere ninety-six) and
//! [`GizmoSettings::max_labels`] labels are held at once; a shape that does not fit is dropped whole, and the console says so
//! once for each run of frames that dropped any.
//!
//! **Shipped builds.** Without the `gizmos` feature of torque-app the types, the pass and the drawing are not compiled;
//! [`GizmosPlugin`] stays, registering the console functions as ones that do nothing, so a game's scripts need no change.

#[cfg(feature = "gizmos")]
mod natives;
#[cfg(feature = "gizmos")]
mod paint;
#[cfg(feature = "gizmos")]
mod shapes;
#[cfg(feature = "gizmos")]
mod state;

use torque_engine::{ClientStage, EngineBuilder, Plugin};

#[cfg(feature = "gizmos")]
pub use paint::{GizmoPainter, project};
#[cfg(feature = "gizmos")]
pub use state::{Gizmos, Label, MAX_LABEL_BYTES, MAX_SECONDS, Style};

/// What the console function `name` takes: its argument counts (the name counts) and usage line.
pub(crate) struct NativeSpec {
    pub name: &'static str,
    pub min: i32,
    pub max: i32,
    pub usage: &'static str,
}

/// The console functions, in the order they are registered.
pub(crate) const NATIVE_SPECS: [NativeSpec; 8] = [
    NativeSpec {
        name: "drawDebugLine",
        min: 4,
        max: 6,
        usage: "drawDebugLine(start, end, color [, seconds [, depth]]);",
    },
    NativeSpec {
        name: "drawDebugBox",
        min: 4,
        max: 6,
        usage: "drawDebugBox(min, max, color [, seconds [, depth]]);",
    },
    NativeSpec {
        name: "drawDebugOrientedBox",
        min: 5,
        max: 7,
        usage: "drawDebugOrientedBox(center, halfExtents, rotation, color [, seconds [, depth]]);",
    },
    NativeSpec {
        name: "drawDebugSphere",
        min: 4,
        max: 6,
        usage: "drawDebugSphere(center, radius, color [, seconds [, depth]]);",
    },
    NativeSpec {
        name: "drawDebugArrow",
        min: 4,
        max: 6,
        usage: "drawDebugArrow(start, end, color [, seconds [, depth]]);",
    },
    NativeSpec {
        name: "drawDebugCross",
        min: 4,
        max: 6,
        usage: "drawDebugCross(center, size, color [, seconds [, depth]]);",
    },
    NativeSpec {
        name: "drawDebugText",
        min: 4,
        max: 6,
        usage: "drawDebugText(position, text, color [, seconds [, depth]]);",
    },
    NativeSpec {
        name: "clearDebug",
        min: 1,
        max: 1,
        usage: "clearDebug();",
    },
];

/// The limits and the look of a client's gizmos.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GizmoSettings {
    /// The most line segments held at once.
    pub max_lines: usize,
    /// The most labels held at once.
    pub max_labels: usize,
    /// The lines' width in pixels.
    pub line_width: f32,
    /// The labels' font face and size (the system's, as the GUI's fonts are found).
    pub font_face: &'static str,
    pub font_size: i32,
}

impl Default for GizmoSettings {
    fn default() -> GizmoSettings {
        GizmoSettings {
            max_lines: 65_536,
            max_labels: 256,
            line_width: 2.0,
            font_face: "Arial",
            font_size: 14,
        }
    }
}

/// Debug gizmos: each client gets a [`Gizmos`] (kept as an extension of its script engine) and the console functions
/// `drawDebugLine`, `drawDebugBox`, `drawDebugOrientedBox`, `drawDebugSphere`, `drawDebugArrow`, `drawDebugCross`, `drawDebugText`
/// and `clearDebug`; a view attached to the client draws them. Not in [`DefaultPlugins`](crate::plugins::DefaultPlugins). See the
/// [module](self) for the arguments and the caps.
///
/// ```
/// use torque_app::gizmos::GizmosPlugin;
/// use torque_app::plugins::DefaultPlugins;
/// use torque_engine::EngineBuilder;
///
/// let engine = EngineBuilder::new()
///     .add_plugins(DefaultPlugins)
///     .add_plugins(GizmosPlugin::default())
///     .build()?;
/// assert!(engine.is_plugin_added::<GizmosPlugin>());
/// # Ok::<(), torque_engine::BuildError>(())
/// ```
#[derive(Clone, Copy, Debug, Default, torque_engine::PluginId)]
#[plugin(id = "torque.gizmos")]
pub struct GizmosPlugin {
    pub settings: GizmoSettings,
}

impl GizmosPlugin {
    /// The plugin with `settings`.
    pub fn new(settings: GizmoSettings) -> GizmosPlugin {
        GizmosPlugin { settings }
    }
}

impl Plugin for GizmosPlugin {
    #[cfg(feature = "gizmos")]
    fn build(&self, engine: &mut EngineBuilder) {
        let settings = self.settings;
        engine.client(ClientStage::Natives, move |vm, _| {
            let gizmos = std::rc::Rc::new(Gizmos::new(settings));
            natives::register(vm, &gizmos);
            vm.set_extension(gizmos);
            Ok(())
        });
        // What Rust callers overflowed is told to the console with the next tick.
        engine.push(torque_engine::ClientTick::new(|vm| {
            if let Some(gizmos) = Gizmos::of(vm) {
                natives::flush(vm, &gizmos);
            }
        }));
    }

    /// Without the `gizmos` feature: the console functions do nothing.
    #[cfg(not(feature = "gizmos"))]
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client(ClientStage::Natives, |vm, _| {
            for spec in NATIVE_SPECS {
                vm.register(spec.name, spec.min, spec.max, spec.usage, |_, _| {
                    torque_ts_vm::vm::Native::Void
                });
            }
            Ok(())
        });
    }
}

#[cfg(all(test, feature = "gizmos"))]
#[path = "gizmos_tests.rs"]
mod tests;

/// Without the `gizmos` feature the console functions exist and do nothing.
#[cfg(all(test, not(feature = "gizmos")))]
mod off_tests {
    use torque_engine::{ClientStage, EngineBuilder};
    use torque_ts_vm::vm::Vm;

    use super::*;

    #[test]
    fn the_console_functions_do_nothing_without_the_feature() {
        let engine = EngineBuilder::new()
            .add_plugins(GizmosPlugin::default())
            .build()
            .expect("the engine builds");
        let mut vm = Vm::new();
        for &stage in ClientStage::ALL {
            engine.replay(stage, &mut vm).expect("the recipes run");
        }
        let listing = vm.native_listing();
        for spec in &NATIVE_SPECS {
            assert!(listing.contains(spec.name), "{}", spec.name);
        }
        vm.eval(br#"drawDebugLine("0 0 0", "1 1 1", "1 0 0", 2); clearDebug();"#)
            .expect("the calls run");
        assert!(vm.drain_log().is_empty());
    }
}
