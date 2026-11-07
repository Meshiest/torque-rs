//! [`AudioPlugin`]: the `alx` natives, the audio datablock classes and the mixer, as a plugin of the engine builder.
//!
//! Each client the engine makes gets its own [`AudioHost`] (the mixer, the buffers, the profiles' preloaded sounds), kept as
//! a script-engine extension ([`AudioPlugin::host`]); its voices advance once per client tick ([`ClientTick`]). The host
//! plays through the null sink, so nothing is ever played on the machine and no sound card is needed; a game that wants
//! sound opens the card itself with [`AudioHost::open_device`] (feature `device`). Without the plugin a client has no `alx`
//! natives and no audio datablock classes.

use std::cell::RefCell;
use std::rc::Rc;

use torque_engine::{ClientStage, ClientTick, EngineBuilder, Plugin, PluginId};
use ts_vm::vm::Vm;

use crate::natives::{self, AlxProfile, AudioHost};

/// The `alx` console functions, `AudioDescription`, `AudioProfile` and `AudioSampleEnvironment` on every client, over a
/// mixer of the client's own that the client's tick moves.
#[derive(Clone, Copy, Debug, Default, PluginId)]
pub struct AudioPlugin;

impl AudioPlugin {
    /// The audio state of the client made over `vm` (None: the client was made without the plugin).
    pub fn host(vm: &Vm) -> Option<Rc<RefCell<AudioHost>>> {
        vm.extension::<RefCell<AudioHost>>()
    }
}

/// The host as the natives reach it: the state is the host itself.
fn own(host: &mut AudioHost) -> &mut AudioHost {
    host
}

impl Plugin for AudioPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client(ClientStage::Natives, |vm, _| {
            natives::install_classes(vm);
            let host = Rc::new(RefCell::new(AudioHost::default()));
            natives::register(vm, &host, own, AlxProfile::tge());
            vm.set_extension(host);
            Ok(())
        });
        engine.push(ClientTick::new(|vm| {
            if let Some(host) = AudioPlugin::host(vm) {
                natives::tick(vm, &host, own);
            }
        }));
    }
}

#[cfg(test)]
#[path = "plugin_tests.rs"]
mod tests;
