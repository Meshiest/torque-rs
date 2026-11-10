//! The engine's side of a realm as the host's session sees it. During a frame [`Realms`](crate::Realms) hands the session an
//! [`Applying`]: the script engine and the GUI in hand, so the realm's `ui` records are checked whole and applied to its controls
//! as the session applies them, inside the realm's frame budget, its slow rule and its strikes. Everything else is written down
//! ([`Recording`]) and carried out right after the frame: the log lines, then the `net` ops, which the session hands over only
//! once the `ui` pieces before them applied.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use torque_client_script_format::{MAX_FLUSH_BYTES, MAX_OPS, Records, Tier, Value};
use torque_client_script_host::{CallRoute, Effects, LogLevel, Stop, Violation};
use torque_engine::rpc::RpcTable;
use torque_gui::Gui;
use torque_gui::client_script::Realm;
use torque_ts_vm::vm::Vm;

use crate::call;
use crate::messages::ScriptUp;

/// A realm's `net` op, kept for the frame's end.
#[derive(Debug)]
pub(crate) enum Outgoing {
    /// A `net.send`.
    Send(Value),
    /// A `net.call` that was let through, with its script arguments.
    Call {
        id: u32,
        name: String,
        args: Vec<Vec<u8>>,
    },
}

/// What the realm asked for during a frame.
#[derive(Debug, Default)]
pub(crate) struct Record {
    pub(crate) logs: Vec<(LogLevel, String)>,
    pub(crate) outgoing: VecDeque<Outgoing>,
    pub(crate) revoked: Vec<Tier>,
    /// The realm is gone: every control, marker and sound it made must go.
    pub(crate) torn_down: bool,
    pub(crate) stopped: Option<String>,
    /// Sends the host refused for their size, and calls it refused as not the realm's to make.
    pub(crate) refused_sends: u64,
    pub(crate) refused_calls: u64,
}

/// The recorder the session owns; [`Realms`](crate::Realms) holds the other end, and a copy for its frames' [`Applying`].
#[derive(Clone)]
pub(crate) struct Recording {
    record: Rc<RefCell<Record>>,
    table: RpcTable,
}

impl Recording {
    pub(crate) fn new(table: RpcTable) -> (Recording, Rc<RefCell<Record>>) {
        let record = Rc::new(RefCell::new(Record::default()));
        (
            Recording {
                record: Rc::clone(&record),
                table,
            },
            record,
        )
    }
}

impl Effects for Recording {
    fn log(&mut self, level: LogLevel, line: &str) {
        self.record
            .borrow_mut()
            .logs
            .push((level, line.to_string()));
    }

    /// `ui` records reach the GUI only through a frame's [`Applying`]: any other way is refused.
    fn check_ui(&mut self, _records: &[u8]) -> Result<(), Violation> {
        Err(Violation::Ui(OUTSIDE_A_FRAME))
    }

    fn apply_ui(&mut self, _records: &[u8]) -> Result<(), Violation> {
        Err(Violation::Ui(OUTSIDE_A_FRAME))
    }

    fn send(&mut self, message: Value) {
        let mut record = self.record.borrow_mut();
        // The host re-checks the size the guest checked: it never trusts the guest.
        if ScriptUp::of(&message).is_err() {
            record.refused_sends += 1;
            return;
        }
        record.outgoing.push_back(Outgoing::Send(message));
    }

    fn call(&mut self, id: u32, name: &str, args: Value) -> CallRoute {
        let mut record = self.record.borrow_mut();
        let args =
            call::callable(&self.table, name).and_then(|entry| call::script_args(entry, &args));
        let Some(args) = args else {
            record.refused_calls += 1;
            return CallRoute::Refused;
        };
        record.outgoing.push_back(Outgoing::Call {
            id,
            name: name.to_string(),
            args,
        });
        CallRoute::Sent
    }

    fn revoke(&mut self, tier: Tier) {
        self.record.borrow_mut().revoked.push(tier);
    }

    fn teardown(&mut self) {
        self.record.borrow_mut().torn_down = true;
    }

    fn stopped(&mut self, why: &Stop) {
        self.record.borrow_mut().stopped = Some(why.to_string());
    }
}

const OUTSIDE_A_FRAME: &str = "ui records outside the engine's frame";

/// Checks that `records` follow the format and are all `ui` ones.
fn check_framing(records: &[u8]) -> Result<(), Violation> {
    let all = Records::new(records, MAX_OPS, MAX_FLUSH_BYTES)
        .and_then(Records::collect_all)
        .map_err(|_| Violation::Ui("ui records that do not follow the format"))?;
    if all.iter().any(|r| r.tier != Tier::Ui.id()) {
        return Err(Violation::Ui("a record of another tier among the ui ones"));
    }
    Ok(())
}

/// The realm's effects for one frame, with the script engine and the GUI in hand: a flush's `ui` records are checked whole
/// against the realm's controls ([`Realm::check_flush`]) before any of it applies, and each piece is applied when the session
/// applies it, so the time it takes is the realm's. The rest goes to the [`Recording`].
pub(crate) struct Applying<'a> {
    pub(crate) recording: &'a mut Recording,
    pub(crate) realm: &'a mut Realm,
    pub(crate) vm: &'a mut Vm,
    pub(crate) gui: &'a Gui,
    pub(crate) now_us: u64,
}

impl Effects for Applying<'_> {
    fn log(&mut self, level: LogLevel, line: &str) {
        self.recording.log(level, line);
    }

    fn check_ui(&mut self, records: &[u8]) -> Result<(), Violation> {
        check_framing(records)?;
        self.realm
            .check_flush(self.gui, records)
            .map_err(|why| Violation::Ui(why.0))
    }

    /// A piece of a flush [`Effects::check_ui`] passed whole: its ops are not checked again before they apply.
    fn apply_ui(&mut self, records: &[u8]) -> Result<(), Violation> {
        self.realm
            .apply_checked(self.vm, self.gui, records, self.now_us)
            .map(|_| ())
            .map_err(|why| Violation::Ui(why.0))
    }

    fn send(&mut self, message: Value) {
        self.recording.send(message);
    }

    fn call(&mut self, id: u32, name: &str, args: Value) -> CallRoute {
        self.recording.call(id, name, args)
    }

    fn revoke(&mut self, tier: Tier) {
        self.recording.revoke(tier);
    }

    fn teardown(&mut self) {
        self.recording.teardown();
    }

    fn stopped(&mut self, why: &Stop) {
        self.recording.stopped(why);
    }
}
