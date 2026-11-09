//! The engine's side of a realm as the host's session sees it: a recorder. The session calls it while it runs a frame, with no
//! script engine in hand, so it only writes down what the realm asked for; [`Realms`](crate::Realms) applies the record right
//! after the frame, with the script engine and the GUI, in the order the format wants: the `ui` pieces first, then the `net` ops
//! that waited for them.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use torque_client_script_format::{MAX_FLUSH_BYTES, MAX_OPS, Records, Tier, Value};
use torque_client_script_host::{CallRoute, Effects, LogLevel, Stop, Violation};
use torque_engine::rpc::RpcTable;

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
    pub(crate) ui: Vec<Vec<u8>>,
    pub(crate) outgoing: VecDeque<Outgoing>,
    pub(crate) revoked: Vec<Tier>,
    /// The realm is gone: every control, marker and sound it made must go.
    pub(crate) torn_down: bool,
    pub(crate) stopped: Option<String>,
    /// Sends the host refused for their size, and calls it refused as not the realm's to make.
    pub(crate) refused_sends: u64,
    pub(crate) refused_calls: u64,
}

/// The recorder the session owns; [`Realms`](crate::Realms) holds the other end.
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

    fn check_ui(&mut self, records: &[u8]) -> Result<(), Violation> {
        let all = Records::new(records, MAX_OPS, MAX_FLUSH_BYTES)
            .and_then(Records::collect_all)
            .map_err(|_| Violation::Ui("ui records that do not follow the format"))?;
        if all.iter().any(|r| r.tier != Tier::Ui.id()) {
            return Err(Violation::Ui("a record of another tier among the ui ones"));
        }
        Ok(())
    }

    fn apply_ui(&mut self, records: &[u8]) -> Result<(), Violation> {
        self.record.borrow_mut().ui.push(records.to_vec());
        Ok(())
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
