//! The client's own work timed by part for a frame breakdown: the connection's datagrams decoded into the world, the scripts its
//! events ran, the ghosts' script objects kept in step, the prediction and the move packet, the world's sounds and the audio
//! state. A few clock reads a pass, always on; the window takes the sums. A console function a game names sets when the window's
//! breakdown prints ([`Mode`], [`register`]).

use std::time::Duration;

use ts_vm::vm::{Native, Vm};

/// The client's work summed by part since the window last took it.
#[derive(Debug, Clone, Default)]
pub struct LivePerf {
    /// The connection's datagrams read into the world (the feed's pump: the packets, the ghosts' records, the events).
    pub decode: Duration,
    /// The scripts the connection's events ran (remote commands, the datablocks' and ghosts' callbacks, the game's own events).
    pub net_scripts: Duration,
    /// The ghosts' script objects kept in step with the world.
    pub ghost_sync: Duration,
    /// The prediction of the control object and the move packet.
    pub predict: Duration,
    /// The world's sounds and the listener.
    pub sounds: Duration,
    /// The audio state's step.
    pub audio: Duration,
    /// The server list's queries and pings.
    pub queries: Duration,
}

impl LivePerf {
    /// The sums so far, cleared.
    pub fn take(&mut self) -> LivePerf {
        std::mem::take(self)
    }
}

/// A window's frame breakdown: when it prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Only while frames run slow.
    Auto,
    /// Every report period.
    On,
    /// Never (the default).
    #[default]
    Off,
}

impl Mode {
    /// The pref's text as a mode (1 on, 0 auto; negative, empty or anything else: off).
    pub fn of_pref(text: &[u8]) -> Mode {
        let t = String::from_utf8_lossy(text);
        match t.trim().parse::<i64>() {
            Ok(n) if n > 0 => Mode::On,
            Ok(0) => Mode::Auto,
            _ => Mode::Off,
        }
    }

    /// The mode the VM's pref `pref` asks for.
    pub fn of_vm(vm: &Vm, pref: &str) -> Mode {
        Mode::of_pref(&vm.global(pref).unwrap_or_default())
    }

    pub fn number(self) -> i32 {
        match self {
            Mode::Auto => 0,
            Mode::On => 1,
            Mode::Off => -1,
        }
    }
}

/// Registers the console function `name([%mode])`: with an argument it sets the pref `pref` (1 on, 0 auto, -1 off) and prints
/// what it did; it returns the mode in effect.
pub fn register(vm: &mut Vm, name: &'static str, pref: &'static str) {
    vm.register(
        name,
        1,
        2,
        "([int mode]) The frame breakdown in the console: 1 every few seconds, 0 only while frames are slow, -1 never.",
        move |vm, c| {
            if let Some(arg) = c.argv.get(1) {
                let mode = Mode::of_pref(arg);
                vm.set_global(pref, &mode.number().to_string());
                let what = match mode {
                    Mode::On => "on: the frame breakdown prints every few seconds",
                    Mode::Auto => "auto: the frame breakdown prints while frames are slow",
                    Mode::Off => "off",
                };
                vm.print(format!("{name}: {what}").as_bytes());
                return Native::Int(mode.number());
            }
            Native::Int(Mode::of_vm(vm, pref).number())
        },
    );
}

#[cfg(test)]
#[path = "live_perf_tests.rs"]
mod tests;
