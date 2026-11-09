//! Server commands: the `serverCmd*` functions a client runs with `commandToServer`, declared by a plugin with the
//! arguments they take, how often a client may call them and how large the arguments may be.
//!
//! A plugin declares one with [`EngineBuilder::server_command`] (the handler is a native, called as
//! `serverCmdName(%client, ..)`) or [`EngineBuilder::server_command_limits`] (the function is the game's own script's, in
//! TorqueScript or Lua). The server's network host holds each call a client sends to its spec before the function runs:
//! a call over the connection's [`TokenBucket`] for that command, with too many or too few arguments, an argument longer
//! than [`max_arg_bytes`](CommandSpec::max_arg_bytes) or one that is not of its [`ArgType`] is refused, counted and never
//! reaches the function. Commands no plugin declared keep TGE's behavior (no limits of this kind) in Torque's protocol; in the
//! engine's protocol they share default limits ([`CommandSpec::check_unlisted`], one bucket for the connection).

use std::fmt;

use torque_ts_vm::convert::ArgKind;
use torque_ts_vm::text::until_nul;

use crate::script::Callee;
use crate::{BuildError, EngineBuilder, IntoNative, PluginId};

/// The prefix of a server command's function.
pub const SERVER_COMMAND_PREFIX: &str = "serverCmd";

/// The most arguments a command call has in the engine's protocol (what TGE's sender allows).
pub const MAX_COMMAND_ARGS: usize = 20;

/// How long a server tick is in milliseconds: the unit a [`TokenBucket`] counts in.
const TICK_MS: u64 = 32;

/// A token bucket counted in server ticks, so its answers depend on the tick count and nothing else (no wall clock).
///
/// It holds up to `burst` tokens and gains `rate` tokens a second (`rate * 32 / 1000` a tick, kept exactly in thousandths).
/// [`take`](Self::take) spends one. A bucket starts full.
///
/// ```
/// use torque_engine::command::TokenBucket;
///
/// let mut bucket = TokenBucket::new(10, 3, 0);
/// // Burst first: three calls in the first tick, then none.
/// assert_eq!((0..1000).filter(|_| bucket.take(0)).count(), 3);
/// // 600 ticks of 32 ms is 19.2 seconds: 192 tokens at 10 a second, held to the burst of 3.
/// assert_eq!((0..1000).filter(|_| bucket.take(600)).count(), 3);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TokenBucket {
    /// Tokens held, in thousandths.
    milli: u64,
    /// The tick the tokens are counted at.
    tick: u64,
    rate: u32,
    burst: u32,
}

impl TokenBucket {
    /// A full bucket at `tick`, of at most `burst` tokens (at least one) gaining `rate` a second.
    pub fn new(rate: u32, burst: u32, tick: u64) -> TokenBucket {
        let burst = burst.max(1);
        TokenBucket {
            milli: u64::from(burst) * 1000,
            tick,
            rate,
            burst,
        }
    }

    /// Tokens a second.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The most tokens it holds.
    pub fn burst(&self) -> u32 {
        self.burst
    }

    /// Refills the bucket up to `tick` (a tick before the last one adds nothing) and spends a token: false, and nothing
    /// spent, when it holds less than one.
    pub fn take(&mut self, tick: u64) -> bool {
        let elapsed = tick.saturating_sub(self.tick);
        self.tick = self.tick.max(tick);
        let gained = elapsed
            .saturating_mul(u64::from(self.rate))
            .saturating_mul(TICK_MS);
        self.milli = self
            .milli
            .saturating_add(gained)
            .min(u64::from(self.burst) * 1000);
        if self.milli < 1000 {
            return false;
        }
        self.milli -= 1000;
        true
    }
}

/// Ticks between two console lines about one connection's dropped calls (a second, rounded to whole ticks).
pub const REPORT_TICKS: u64 = 31;

/// The calls of one kind a connection had dropped or refused, and when the console is next told: at most one line a second
/// however many were dropped (what the server commands and the RPCs both report with).
#[derive(Clone, Copy, Debug, Default)]
pub struct DropCounter {
    total: u64,
    unreported: u64,
    last_report: Option<u64>,
}

impl DropCounter {
    /// Counts one more dropped call.
    pub fn note(&mut self) {
        self.total = self.total.saturating_add(1);
        self.unreported = self.unreported.saturating_add(1);
    }

    /// How many were dropped in all.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// The count a console line is due for at `tick` (those dropped since the last line), or `None` when nothing was dropped or
    /// a line went out less than [`REPORT_TICKS`] ago. Asking again after a `Some` gives `None` until more are dropped.
    pub fn report(&mut self, tick: u64) -> Option<u64> {
        if self.unreported == 0 {
            return None;
        }
        let due = self
            .last_report
            .is_none_or(|last| tick.saturating_sub(last) >= REPORT_TICKS);
        if !due {
            return None;
        }
        self.last_report = Some(tick);
        Some(std::mem::take(&mut self.unreported))
    }
}

/// What an argument of a server command must be, checked before the command runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ArgType {
    /// Any text.
    Text,
    /// A whole number in `i32`'s range: an optional minus sign and digits.
    Int,
    /// A finite number (`2`, `-0.5`, `1e3`).
    Float,
    /// `0`, `1`, `true` or `false` (any case).
    Bool,
}

impl ArgType {
    fn accepts(self, text: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(text) else {
            return self == ArgType::Text;
        };
        match self {
            ArgType::Text => true,
            ArgType::Int => {
                let digits = text.strip_prefix('-').unwrap_or(text);
                !digits.is_empty()
                    && digits.bytes().all(|b| b.is_ascii_digit())
                    && text.parse::<i32>().is_ok()
            }
            ArgType::Float => text.parse::<f32>().is_ok_and(f32::is_finite),
            ArgType::Bool => ["0", "1", "true", "false"]
                .iter()
                .any(|word| text.eq_ignore_ascii_case(word)),
        }
    }

    fn word(self) -> &'static str {
        match self {
            ArgType::Text => "text",
            ArgType::Int => "a whole number",
            ArgType::Float => "a number",
            ArgType::Bool => "0, 1, true or false",
        }
    }
}

/// One argument of a [`CommandSpec`]: its name (for the usage text and refusals), its type, and whether a call may leave
/// it out (only the last arguments may be optional).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommandArg {
    /// The name.
    pub name: String,
    /// What it must be.
    pub ty: ArgType,
    /// A call may leave it out.
    pub optional: bool,
}

/// What a server command takes and how often: the arguments after `%client`, the size of each, and a token bucket
/// per connection.
///
/// The defaults are 32 calls a second with a burst of 64 and arguments of at most 255 bytes (what a TGE command argument
/// can be).
///
/// ```
/// use torque_engine::command::{ArgType, CommandSpec};
///
/// let spec = CommandSpec::new()
///     .arg("count", ArgType::Int)
///     .optional("label", ArgType::Text)
///     .rate(4, 8)
///     .max_arg_bytes(32);
/// assert!(spec.check(&[b"3".to_vec()]).is_ok());
/// assert!(spec.check(&[b"three".to_vec()]).is_err());
/// assert!(spec.check(&[b"3".to_vec(), vec![b'x'; 33]]).is_err());
/// assert!(spec.check(&[b"3".to_vec(), b"a".to_vec(), b"b".to_vec()]).is_err());
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommandSpec {
    /// The arguments after the client, in order.
    pub args: Vec<CommandArg>,
    /// The most bytes one argument may have.
    pub max_arg_bytes: usize,
    /// Calls a second one connection may make, once its burst is spent.
    pub rate: u32,
    /// Calls one connection may make at once.
    pub burst: u32,
    /// What the command does, for its usage text.
    pub description: String,
    /// The permission a caller must hold (a dotted name such as `chat.mute`; a wildcard grant of the caller covers it), if the
    /// command is not for everyone. Checked by the network host before the arguments are.
    pub permission: Option<String>,
}

impl Default for CommandSpec {
    fn default() -> CommandSpec {
        CommandSpec {
            args: Vec::new(),
            max_arg_bytes: 255,
            rate: 32,
            burst: 64,
            description: String::new(),
            permission: None,
        }
    }
}

impl CommandSpec {
    /// A spec with no arguments and the default limits.
    pub fn new() -> CommandSpec {
        CommandSpec::default()
    }

    /// Adds a required argument.
    #[must_use]
    pub fn arg(mut self, name: &str, ty: ArgType) -> CommandSpec {
        self.args.push(CommandArg {
            name: name.to_string(),
            ty,
            optional: false,
        });
        self
    }

    /// Adds an argument a call may leave out (after the required ones, and the optional ones before it).
    #[must_use]
    pub fn optional(mut self, name: &str, ty: ArgType) -> CommandSpec {
        self.args.push(CommandArg {
            name: name.to_string(),
            ty,
            optional: true,
        });
        self
    }

    /// Sets the calls a second and the burst of one connection's bucket.
    #[must_use]
    pub fn rate(mut self, rate: u32, burst: u32) -> CommandSpec {
        self.rate = rate;
        self.burst = burst;
        self
    }

    /// Sets the most bytes one argument may have.
    #[must_use]
    pub fn max_arg_bytes(mut self, bytes: usize) -> CommandSpec {
        self.max_arg_bytes = bytes;
        self
    }

    /// Asks for `permission` of a caller ([`CommandSpec::permission`]). The name is checked when the command is declared, and
    /// declared for the plugin when it is not already.
    #[must_use]
    pub fn requires(mut self, permission: &str) -> CommandSpec {
        self.permission = Some(permission.to_string());
        self
    }

    /// Sets the description of the usage text.
    #[must_use]
    pub fn describe(mut self, description: &str) -> CommandSpec {
        self.description = description.to_string();
        self
    }

    /// How many arguments a call must have at least.
    pub fn min_args(&self) -> usize {
        self.args.iter().filter(|a| !a.optional).count()
    }

    /// A fresh bucket for one connection, full at `tick`.
    pub fn bucket(&self, tick: u64) -> TokenBucket {
        TokenBucket::new(self.rate, self.burst, tick)
    }

    /// Whether `args` (after the client) are a call the spec allows: the count, each argument's size and type. The
    /// first problem is returned.
    pub fn check(&self, args: &[Vec<u8>]) -> Result<(), CommandRefusal> {
        if args.len() < self.min_args() {
            return Err(CommandRefusal::TooFewArguments {
                given: args.len(),
                needed: self.min_args(),
            });
        }
        if args.len() > self.args.len() {
            return Err(CommandRefusal::TooManyArguments {
                given: args.len(),
                max: self.args.len(),
            });
        }
        for (i, (given, arg)) in args.iter().zip(&self.args).enumerate() {
            let text = until_nul(given);
            if text.len() > self.max_arg_bytes {
                return Err(CommandRefusal::ArgumentTooLong {
                    number: i + 1,
                    len: text.len(),
                    max: self.max_arg_bytes,
                });
            }
            if !arg.ty.accepts(text) {
                return Err(CommandRefusal::BadArgument {
                    number: i + 1,
                    expected: arg.ty,
                });
            }
        }
        Ok(())
    }

    /// Whether `args` (after the client) are a call this spec's limits allow for a command no plugin declared a list of
    /// arguments for: any count up to [`MAX_COMMAND_ARGS`], each at most [`max_arg_bytes`](Self::max_arg_bytes) and of any
    /// type; a tag argument, which arrives expanded (up to 1,023 bytes), is not held to the size. The first problem is returned.
    pub fn check_unlisted(&self, args: &[Vec<u8>]) -> Result<(), CommandRefusal> {
        if args.len() > MAX_COMMAND_ARGS {
            return Err(CommandRefusal::TooManyArguments {
                given: args.len(),
                max: MAX_COMMAND_ARGS,
            });
        }
        for (i, given) in args.iter().enumerate() {
            let len = until_nul(given).len();
            if len > self.max_arg_bytes && given.first() != Some(&1) {
                return Err(CommandRefusal::ArgumentTooLong {
                    number: i + 1,
                    len,
                    max: self.max_arg_bytes,
                });
            }
        }
        Ok(())
    }
}

/// Why a call to a server command was refused before its function ran.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum CommandRefusal {
    /// The connection's bucket for the command was empty.
    RateLimited,
    /// The caller does not hold the command's permission.
    NotPermitted,
    /// Fewer arguments than the command's required ones.
    TooFewArguments {
        /// How many the call had.
        given: usize,
        /// How many it needs.
        needed: usize,
    },
    /// More arguments than the command takes.
    TooManyArguments {
        /// How many the call had.
        given: usize,
        /// How many it takes.
        max: usize,
    },
    /// An argument longer than [`CommandSpec::max_arg_bytes`].
    ArgumentTooLong {
        /// Its 1-based number after the client.
        number: usize,
        /// Its length.
        len: usize,
        /// The cap.
        max: usize,
    },
    /// An argument that is not of its [`ArgType`].
    BadArgument {
        /// Its 1-based number after the client.
        number: usize,
        /// What it must be.
        expected: ArgType,
    },
}

impl fmt::Display for CommandRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandRefusal::RateLimited => f.write_str("too many calls"),
            CommandRefusal::NotPermitted => f.write_str("not permitted"),
            CommandRefusal::TooFewArguments { given, needed } => {
                write!(f, "{given} argument(s), at least {needed} needed")
            }
            CommandRefusal::TooManyArguments { given, max } => {
                write!(f, "{given} argument(s), at most {max} taken")
            }
            CommandRefusal::ArgumentTooLong { number, len, max } => {
                write!(f, "argument {number} is {len} bytes, at most {max} allowed")
            }
            CommandRefusal::BadArgument { number, expected } => {
                write!(f, "argument {number} must be {}", expected.word())
            }
        }
    }
}

/// A server command a plugin declared ([`EngineBuilder::server_command`]): the function's name and its spec, in the
/// engine's list of them (`engine.list::<ServerCommand>()`), in the order they were declared.
#[derive(Clone, Debug)]
pub struct ServerCommand {
    /// The function's name: `serverCmd` and the command's name.
    pub function: String,
    /// The plugin that declared it.
    pub plugin: PluginId,
    /// What it takes and how often.
    pub spec: CommandSpec,
}

/// The function a command `name` is: `serverCmd` and the name (a name that has the prefix already keeps it).
pub fn command_function(name: &str) -> String {
    match name.get(..SERVER_COMMAND_PREFIX.len()) {
        Some(head)
            if head.eq_ignore_ascii_case(SERVER_COMMAND_PREFIX) && name.len() > head.len() =>
        {
            name.to_string()
        }
        _ => format!("{SERVER_COMMAND_PREFIX}{name}"),
    }
}

impl EngineBuilder {
    /// Declares a server command the plugin handles natively: `serverCmd<name>(%client, ..)` is registered on every
    /// server's script engine as `handler`, whose script arguments are the client (first) and the spec's arguments in
    /// order, and the network host holds every call a client sends to `spec` ([`CommandSpec`]) before it runs.
    ///
    /// `name` is the command without its `serverCmd` prefix. The handler's script arguments must match the spec: one for
    /// the client, then one for each spec argument, required or `Option<T>` as the spec says. A handler that does not, an
    /// invalid name, or a command another plugin declared fails the build, naming the plugins
    /// ([`BuildError::DuplicateCommand`]; a function of that name another plugin registered is a
    /// [`BuildError::DuplicateScriptName`]).
    ///
    /// A script's own call (`serverCmdName(%client, ..)` from a script) is not limited: only what clients send is.
    ///
    /// ```
    /// use torque_engine::command::{ArgType, CommandSpec};
    /// use torque_engine::{EngineBuilder, Plugin, PluginId, ServerStage};
    ///
    /// fn wave(client: i32, times: i32) -> String {
    ///     format!("client {client} waves {times} times")
    /// }
    ///
    /// #[derive(PluginId)]
    /// struct Wave;
    /// impl Plugin for Wave {
    ///     fn build(&self, engine: &mut EngineBuilder) {
    ///         let spec = CommandSpec::new().arg("times", ArgType::Int).rate(2, 4);
    ///         engine.server_command("Wave", spec, wave);
    ///     }
    /// }
    ///
    /// let engine = EngineBuilder::new().add_plugins(Wave).build()?;
    /// let mut vm = torque_ts_vm::vm::Vm::new();
    /// for &stage in ServerStage::ALL {
    ///     engine.replay(stage, &mut vm)?;
    /// }
    /// assert_eq!(vm.eval(b"return serverCmdWave(7, 3);")?, b"client 7 waves 3 times");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn server_command<M>(
        &mut self,
        name: &str,
        spec: CommandSpec,
        handler: impl IntoNative<M>,
    ) -> &mut EngineBuilder {
        let function = command_function(name);
        let native = handler.into_native();
        let wanted: Vec<ArgKind> = std::iter::once(ArgKind::Required)
            .chain(spec.args.iter().map(|a| {
                if a.optional {
                    ArgKind::Optional
                } else {
                    ArgKind::Required
                }
            }))
            .collect();
        if native.args() != wanted.as_slice() {
            let plugin = self.current_plugin();
            self.fail(BuildError::ScriptSignature {
                plugin,
                message: format!(
                    "`{function}` takes the client and {} argument(s) in its spec, but its handler's script arguments differ",
                    spec.args.len()
                ),
            });
            return self;
        }
        let names: Vec<String> = std::iter::once("client".to_string())
            .chain(spec.args.iter().map(|a| a.name.clone()))
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let description = spec.description.clone();
        if !self.declare_command(&function, spec) {
            return self;
        }
        self.server_script().native_at_run_time(
            Callee::Function {
                namespace: "",
                name: &function,
            },
            &names,
            &description,
            None,
            false,
            "",
            native,
        );
        self
    }

    /// Declares the limits of a server command whose function a script defines (TorqueScript or Lua):
    /// `serverCmd<name>` is the script's, and the network host holds every call a client sends to `spec` before it
    /// runs, as for [`server_command`](Self::server_command). The spec's argument names and description are not used.
    pub fn server_command_limits(&mut self, name: &str, spec: CommandSpec) -> &mut EngineBuilder {
        let function = command_function(name);
        self.declare_command(&function, spec);
        self
    }

    /// Records `spec` for `function`; false (and the build failed) for an invalid name or spec, or a name taken.
    fn declare_command(&mut self, function: &str, spec: CommandSpec) -> bool {
        let plugin = self.current_plugin();
        let valid = function.len() > SERVER_COMMAND_PREFIX.len()
            && function
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !valid {
            self.fail(BuildError::Rejected {
                plugin,
                message: format!(
                    "`{function}` is not a command name (letters, digits and `_`, after the `serverCmd` prefix)"
                ),
            });
            return false;
        }
        if let Some(permission) = &spec.permission {
            if let Err(error) = torque_permissions::Permission::parse(permission) {
                self.fail(BuildError::Rejected {
                    plugin,
                    message: format!("`{function}` asks for a permission that is not one: {error}"),
                });
                return false;
            }
            if !self
                .list::<crate::permissions::DeclaredPermission>()
                .iter()
                .any(|d| d.name == *permission)
            {
                self.push(crate::permissions::DeclaredPermission {
                    plugin,
                    name: permission.clone(),
                    description: format!("Use the {function} command"),
                });
            }
        }
        let optional_first = spec
            .args
            .windows(2)
            .any(|pair| pair[0].optional && !pair[1].optional);
        if optional_first {
            self.fail(BuildError::Rejected {
                plugin,
                message: format!("`{function}` has a required argument after an optional one"),
            });
            return false;
        }
        let taken = self
            .list::<ServerCommand>()
            .iter()
            .find(|c| c.function.eq_ignore_ascii_case(function))
            .map(|c| c.plugin);
        if let Some(first) = taken {
            self.fail(BuildError::DuplicateCommand {
                name: function.to_string(),
                first,
                second: plugin,
            });
            return false;
        }
        self.push(ServerCommand {
            function: function.to_string(),
            plugin,
            spec,
        });
        true
    }
}
