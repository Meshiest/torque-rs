//! The limits of the server commands plugins declare ([`ServerCommand`]): what the host checks a client's call against
//! before the command's function runs.
//!
//! The host holds a [`CommandTable`] (set with [`NetShared::set_commands`](super::NetShared::set_commands)) and each
//! connection a [`ConnCommands`]: one token bucket per declared command, counted in server ticks, and a count of the calls
//! refused. A call goes through [`ConnCommands::admit`]: the bucket first (a flood of malformed calls is limited like
//! any other), then the arguments' count, size and types. A refused call never reaches the function and is counted for
//! the connection and the command ([`commandDrops`](register_command_drops)). While calls keep being refused the host prints one
//! console line a second.
//!
//! A command no plugin declared is not in the table: its calls are [`Admission::Unlimited`] and run as TGE's do.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use torque_engine::command::{
    CommandRefusal, DropCounter, ServerCommand, TokenBucket, command_function,
};
use torque_engine::permissions::{NoPermissions, PermissionView};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::{Native, Vm};

use super::NetGame;
use super::NetShared;

pub use torque_engine::command::REPORT_TICKS;

/// The server commands plugins declared, by function name (without case).
#[derive(Debug, Default)]
pub struct CommandTable {
    by_name: HashMap<String, ServerCommand>,
}

impl CommandTable {
    /// The table of `commands`.
    pub fn new(commands: &[ServerCommand]) -> CommandTable {
        CommandTable {
            by_name: commands
                .iter()
                .map(|c| (c.function.to_ascii_lowercase(), c.clone()))
                .collect(),
        }
    }

    /// No command is declared.
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// The command whose function is `function`, if one was declared.
    pub fn get(&self, function: &[u8]) -> Option<&ServerCommand> {
        let text = std::str::from_utf8(function).ok()?;
        self.by_name.get(&text.to_ascii_lowercase())
    }
}

/// What the host does with a client's call of a command.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Admission {
    /// No plugin declared the command: it runs as TGE's does.
    Unlimited,
    /// Declared, within its limits: it runs.
    Run,
    /// Declared and refused: the function does not run.
    Refused {
        /// Why.
        reason: CommandRefusal,
        /// The console line to print now, if one is due.
        report: Option<String>,
    },
}

#[derive(Clone, Debug)]
struct CommandState {
    bucket: TokenBucket,
    drops: DropCounter,
}

/// One connection's standing with the declared commands.
#[derive(Clone, Debug, Default)]
pub struct ConnCommands {
    states: HashMap<String, CommandState>,
}

impl ConnCommands {
    /// [`admit_for`](Self::admit_for) for a caller who holds no permission (a command that asks for one is refused).
    pub fn admit(
        &mut self,
        table: &CommandTable,
        obj: ObjId,
        function: &[u8],
        args: &[Vec<u8>],
        tick: u64,
    ) -> Admission {
        self.admit_for(table, obj, function, args, tick, &NoPermissions)
    }

    /// Judges `obj`'s call of `function` with `args` (after the client) at server tick `tick`, by a caller whose permissions
    /// are `perms`: the bucket first, then the command's permission, then the arguments.
    pub fn admit_for(
        &mut self,
        table: &CommandTable,
        obj: ObjId,
        function: &[u8],
        args: &[Vec<u8>],
        tick: u64,
        perms: &dyn PermissionView,
    ) -> Admission {
        let Some(command) = table.get(function) else {
            return Admission::Unlimited;
        };
        let state = self
            .states
            .entry(command.function.to_ascii_lowercase())
            .or_insert_with(|| CommandState {
                bucket: command.spec.bucket(tick),
                drops: DropCounter::default(),
            });
        let verdict = if !state.bucket.take(tick) {
            Err(CommandRefusal::RateLimited)
        } else if command
            .spec
            .permission
            .as_deref()
            .is_some_and(|p| !perms.has_permission(p))
        {
            Err(CommandRefusal::NotPermitted)
        } else {
            command.spec.check(args)
        };
        let Err(reason) = verdict else {
            return Admission::Run;
        };
        state.drops.note();
        let report = state.drops.report(tick).map(|count| {
            format!(
                "Client {obj} {}: {count} call(s) refused ({reason}).",
                command.function
            )
        });
        Admission::Refused { reason, report }
    }

    /// How many calls of `command` (its name with or without `serverCmd`) were refused.
    pub fn drops(&self, command: &str) -> u64 {
        self.states
            .get(&command_function(command).to_ascii_lowercase())
            .map_or(0, |state| state.drops.total())
    }
}

/// Registers `commandDrops(client, command)` on `vm`: how many of `client`'s calls of the server command `command` (with or
/// without the `serverCmd` prefix) were refused, 0 for a command no plugin declared or a client that is not connected.
/// Held weakly: the host's state holds the script engine's.
pub fn register_command_drops<G: NetGame>(vm: &mut Vm, shared: &Rc<RefCell<NetShared<G>>>) {
    let weak: Weak<RefCell<NetShared<G>>> = Rc::downgrade(shared);
    vm.register(
        "commandDrops",
        3,
        3,
        "commandDrops(client, command) - how many calls of a server command the client sent that were refused",
        move |vm, call| {
            let count = (|| {
                let shared = weak.upgrade()?;
                let client = super::commands::find_net_connection(vm, super::commands::arg(call, 1))?;
                let command = String::from_utf8_lossy(super::commands::arg(call, 2)).into_owned();
                let sh = shared.borrow();
                sh.conns.get(&client).map(|rec| rec.plugin_cmds.drops(&command))
            })()
            .unwrap_or(0);
            Native::Str(count.to_string().into_bytes())
        },
    );
}

#[cfg(test)]
#[path = "cmdlimit_tests.rs"]
mod tests;
