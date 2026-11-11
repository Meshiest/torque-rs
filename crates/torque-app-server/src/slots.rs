//! The main loop's seams on the engine builder: the hooks every server's main loop runs and how its missions load.
//!
//! | Seam | Setter | Shape |
//! |------|--------|-------|
//! | hooks ([`ServerHooks`]) | [`ServerSlots::server_hooks`] | a list: every plugin may add, in add order |
//! | mission loader ([`MissionLoader`]) | [`ServerSlots::mission_loader`] | exclusive: a second setter fails the build, naming both plugins |
//!
//! A plugin gives a factory, not an instance: [`GameServer::start_engine`](crate::server::GameServer::start_engine) calls each
//! one once per server, after the `Net` stage and before the entry script, so what it makes can read what the server's recipes
//! left on the script engine (its extensions, its state). An error from a factory stops that server's start with the error's
//! message. A server whose engine has no entry keeps the plain engine's hooks and loader.
//!
//! The hooks of one server are run together: `before_events` and `pass_sliced` in add order, `pass_slice` answered by the first
//! hook that gives a slice ([`HookList`]). The `ServerTick` list is separate and unchanged (work per 32 ms tick); hooks are
//! for the once-a-pass work and the pass's slice of the wall, which a tick cannot express.

use std::rc::Rc;

use torque_engine::{Engine, EngineBuilder, PluginId};
use torque_server::server::{HookList, MakeHooks, MakeMissionLoader, MissionLoader, ServerHooks};
use torque_ts_vm::vm::Vm;

/// A plugin's hooks factory, as the engine's list holds it, with the plugin that registered it.
struct HooksEntry {
    plugin: PluginId,
    make: Rc<MakeHooks>,
}

/// The engine's mission loader factory (the exclusive slot's value).
struct LoaderSlot(Rc<MakeMissionLoader>);

/// The engine builder's calls that set a server's main-loop seams.
pub trait ServerSlots {
    /// Adds the hooks `make` makes to every server's main loop. A list: each plugin's hooks run in add order.
    fn server_hooks(
        &mut self,
        make: impl Fn(&mut Vm) -> Result<Rc<dyn ServerHooks>, String> + 'static,
    ) -> &mut EngineBuilder;

    /// Sets how every server loads a mission: the loader `make` makes replaces TGE's (which `exec`s the `.mis`). Exclusive: a
    /// second setter fails the build, naming both plugins.
    fn mission_loader(
        &mut self,
        make: impl Fn(&mut Vm) -> Result<Rc<dyn MissionLoader>, String> + 'static,
    ) -> &mut EngineBuilder;
}

impl ServerSlots for EngineBuilder {
    fn server_hooks(
        &mut self,
        make: impl Fn(&mut Vm) -> Result<Rc<dyn ServerHooks>, String> + 'static,
    ) -> &mut EngineBuilder {
        let plugin = self.current_plugin();
        self.push(HooksEntry {
            plugin,
            make: Rc::new(make),
        })
    }

    fn mission_loader(
        &mut self,
        make: impl Fn(&mut Vm) -> Result<Rc<dyn MissionLoader>, String> + 'static,
    ) -> &mut EngineBuilder {
        self.set_slot(Rc::new(LoaderSlot(Rc::new(make))))
    }
}

/// Makes the server hooks `engine`'s plugins registered ([`ServerSlots::server_hooks`]) over `vm`, each factory once, in add
/// order. None when no plugin registered any (the server keeps its plain hooks), the hooks themselves when there is one, a
/// [`HookList`] of them when there are several. A factory's error is returned with its message.
///
/// For a host that does not go through `GameServer::start*` (those already apply the slots, after the `Net` stage and before the
/// entry script): call this at the same point, then `Server::set_hooks`. Calling it for a server `GameServer` made runs the
/// factories a second time.
pub fn make_server_hooks(
    engine: &Engine,
    vm: &mut Vm,
) -> Result<Option<Rc<dyn ServerHooks>>, String> {
    let mut made = Vec::new();
    for entry in engine.list::<HooksEntry>() {
        let hooks = (entry.make)(vm)
            .map_err(|e| format!("the server hooks of plugin `{}`: {e}", entry.plugin))?;
        made.push(hooks);
    }
    Ok(match made.len() {
        0 => None,
        1 => made.pop(),
        _ => Some(Rc::new(HookList::new(made))),
    })
}

/// Makes the mission loader `engine`'s plugins set ([`ServerSlots::mission_loader`]) over `vm`. None when no plugin set one (the
/// server keeps TGE's, which `exec`s the `.mis`); a factory's error is returned with its message. Only for a host that does not
/// go through `GameServer::start*` (which already applies it): call it at the same point as
/// [`make_server_hooks`], then `Server::set_mission_loader`.
pub fn make_mission_loader(
    engine: &Engine,
    vm: &mut Vm,
) -> Result<Option<Rc<dyn MissionLoader>>, String> {
    let Some(slot) = engine.slot::<LoaderSlot>() else {
        return Ok(None);
    };
    (slot.0)(vm)
        .map(Some)
        .map_err(|e| format!("the mission loader: {e}"))
}
