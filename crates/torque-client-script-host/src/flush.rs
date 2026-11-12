//! The host's check of a flush: every record read whole and checked against the format and the realm's tiers, and all
//! its `ui` records checked by the GUI's applier ([`Effects::check_ui`], the enforcement point of their handles, props
//! and caps), before anything of it is applied. One violation stops the realm and applies nothing of the flush. The
//! actions come out with the flush's `ui` pieces first, so its `net` ops go out only once its `ui` ops applied.

use std::collections::HashMap;

use torque_client_script_format::{
    Grants, MAX_FLUSH_BYTES, MAX_LOG_LINE, MAX_OPS, NodeBudget, Record, Records, Tier, Value,
    Writer, is_hidden, net, op,
    rate::{LineWindow, LogLine, SendBucket},
};

use crate::effects::{Effects, LogLevel, Violation};

/// The most `ui` records the host hands the GUI in one piece: the unit the frame budget is checked between, so a frame holds
/// at most one piece past its budget.
pub(crate) const UI_CHUNK: usize = 64;

/// One checked op (or a run of `ui` ones), for the engine.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Action {
    Log(LogLevel, String),
    /// A callback ran out of steps or memory: a strike.
    Limit,
    /// `ui` records, as a flush of their own.
    Ui(Vec<u8>),
    Send(Value),
    Call {
        id: u32,
        name: String,
        args: Value,
    },
}

/// The realm's tiers as one flush sees them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tiers {
    /// The tiers granted when the realm was made.
    pub(crate) granted: Grants,
    /// The tiers the player took away since.
    pub(crate) revoked: Grants,
    /// The revocations the guest had been handed when the entry started: an op of one of those is a violation, an op
    /// of a tier revoked later only too late to apply.
    pub(crate) told: Grants,
}

/// The host's account of the realm's `net` ops: the guest keeps the same rules, so an honest guest never breaks one.
#[derive(Debug)]
pub(crate) struct NetCheck {
    last_call: u32,
    /// Calls waiting for their reply: id to name.
    calls: HashMap<u32, String>,
    /// Calls waiting, by name.
    outstanding: HashMap<String, usize>,
    /// The guest's rate of `net.send` and `net.call` together. The host takes only what the guest emitted, at the entry's
    /// time, so it never has less left than the guest.
    bucket: SendBucket,
}

impl Default for NetCheck {
    fn default() -> NetCheck {
        NetCheck {
            last_call: 0,
            calls: HashMap::new(),
            outstanding: HashMap::new(),
            bucket: SendBucket::new(),
        }
    }
}

impl NetCheck {
    /// Takes the call `id` off the waiting ones: `false` when it is not one.
    pub(crate) fn answered(&mut self, id: u32) -> bool {
        let Some(name) = self.calls.remove(&id) else {
            return false;
        };
        if let Some(n) = self.outstanding.get_mut(&name) {
            *n -= 1;
            if *n == 0 {
                self.outstanding.remove(&name);
            }
        }
        true
    }

    /// Forgets every waiting call (the tier was revoked: their replies are not delivered).
    pub(crate) fn clear(&mut self) {
        self.calls.clear();
        self.outstanding.clear();
    }

    fn send(&mut self, now_us: u64, len: usize) -> Result<(), Violation> {
        if len > net::MAX_SEND_BYTES {
            return Err(Violation::TooLarge);
        }
        if !self.bucket.take(now_us, len) {
            return Err(Violation::TooFast);
        }
        Ok(())
    }

    fn call(&mut self, now_us: u64, id: u32, name: &str, len: usize) -> Result<(), Violation> {
        if id != self.last_call.wrapping_add(1) || id == 0 {
            return Err(Violation::CallOrder);
        }
        net::check_rpc_name(name).map_err(|_| Violation::RpcName)?;
        if len > net::MAX_CALL_BYTES {
            return Err(Violation::TooLarge);
        }
        if self.outstanding.get(name).copied().unwrap_or(0) >= net::MAX_OUTSTANDING
            || self.calls.len() >= net::MAX_OUTSTANDING_TOTAL
        {
            return Err(Violation::TooManyCalls);
        }
        if !self.bucket.take(now_us, len) {
            return Err(Violation::TooFast);
        }
        self.last_call = id;
        self.calls.insert(id, name.to_string());
        *self.outstanding.entry(name.to_string()).or_default() += 1;
        Ok(())
    }
}

/// The host's own cap on log lines: [`LOG_LINES_PER_SECOND`](torque_client_script_format::LOG_LINES_PER_SECOND) and the
/// guest's one warning, per second from the first line; past it lines are dropped (the guest counts with the same
/// [`LineWindow`], so only a dishonest one loses lines).
#[derive(Debug, Default)]
pub(crate) struct LogRate(LineWindow);

impl LogRate {
    fn admit(&mut self, now_us: u64) -> bool {
        self.0.log_line(now_us) != LogLine::Dropped
    }
}

/// The checked actions of `flush`, an entry's at `now_us`: its `ui` pieces first, then its other actions in order.
pub(crate) fn check(
    flush: &[u8],
    tiers: Tiers,
    now_us: u64,
    net: &mut NetCheck,
    logs: &mut LogRate,
    effects: &mut dyn Effects,
) -> Result<Vec<Action>, Violation> {
    let records =
        Records::new(flush, MAX_OPS, MAX_FLUSH_BYTES).map_err(|_| Violation::FlushCaps)?;
    let mut budget = NodeBudget::default();
    let mut rest = Vec::new();
    let mut ui = UiRun::default();
    for record in records {
        let record = record.map_err(|_| Violation::Framing)?;
        let tier = Tier::from_id(record.tier).ok_or(Violation::NoTier)?;
        if !tiers.granted.has(tier) {
            return Err(Violation::Ungranted);
        }
        if tiers.revoked.has(tier) {
            if tiers.told.has(tier) {
                return Err(Violation::Revoked);
            }
            continue;
        }
        match tier {
            Tier::Realm => rest.extend(realm_op(&record, now_us, logs)?),
            Tier::Ui if record.kind > op::ui::FOCUS => return Err(Violation::NoKind),
            Tier::Ui => ui.push(&record),
            Tier::Net => rest.push(net_op(&record, now_us, net, &mut budget)?),
            _ => return Err(Violation::NotInFormat),
        }
    }
    if !ui.all.is_empty() {
        effects.check_ui(ui.all.bytes())?;
    }
    let mut actions = ui.pieces();
    actions.append(&mut rest);
    Ok(actions)
}

/// The `ui` records of a flush: all of them, for the GUI's check, and the ends of its pieces of at most [`UI_CHUNK`].
#[derive(Default)]
struct UiRun {
    all: Writer,
    count: usize,
    /// Where each full piece ends in `all`.
    cuts: Vec<usize>,
}

impl UiRun {
    fn push(&mut self, record: &Record<'_>) {
        self.all.u8(record.tier);
        self.all.u8(record.kind);
        self.all.u32(record.body.len() as u32);
        self.all.raw(record.body);
        self.count += 1;
        if self.count.is_multiple_of(UI_CHUNK) {
            self.cuts.push(self.all.len());
        }
    }

    /// The pieces, as actions.
    fn pieces(self) -> Vec<Action> {
        let bytes = self.all.bytes();
        let mut start = 0;
        let mut pieces = Vec::with_capacity(self.cuts.len() + 1);
        for end in self.cuts.into_iter().chain([bytes.len()]) {
            if end > start {
                pieces.push(Action::Ui(bytes[start..end].to_vec()));
            }
            start = end;
        }
        pieces
    }
}

/// A `realm` op: a log line (`None` when the host's own cap drops it) or a limit stop.
fn realm_op(
    record: &Record<'_>,
    now_us: u64,
    logs: &mut LogRate,
) -> Result<Option<Action>, Violation> {
    let malformed = |_| Violation::Body;
    let mut r = record.reader();
    let action = match record.kind {
        op::realm::LOG => {
            let level = match r.u8().map_err(malformed)? {
                op::realm::LEVEL_INFO => LogLevel::Info,
                op::realm::LEVEL_WARNING => LogLevel::Warning,
                op::realm::LEVEL_ERROR => LogLevel::Error,
                _ => return Err(Violation::Body),
            };
            let line = r.text().map_err(malformed)?;
            if line.len() > MAX_LOG_LINE || line.chars().any(is_hidden) {
                return Err(Violation::LogNotPlain);
            }
            logs.admit(now_us)
                .then(|| Action::Log(level, line.to_string()))
        }
        op::realm::LIMIT => {
            let which = r.u8().map_err(malformed)?;
            let entry = r.u8().map_err(malformed)?;
            if which > op::realm::LIMIT_MEMORY || entry > op::realm::ENTRY_FRAME {
                return Err(Violation::Body);
            }
            Some(Action::Limit)
        }
        _ => return Err(Violation::NoKind),
    };
    r.finish().map_err(malformed)?;
    Ok(action)
}

/// A `net` op.
fn net_op(
    record: &Record<'_>,
    now_us: u64,
    net: &mut NetCheck,
    budget: &mut NodeBudget,
) -> Result<Action, Violation> {
    let malformed = |_| Violation::Body;
    let mut r = record.reader();
    let action = match record.kind {
        op::net::SEND => {
            let len = r.rest().len();
            let message = Value::decode(&mut r, budget).map_err(malformed)?;
            net.send(now_us, len)?;
            Action::Send(message)
        }
        op::net::CALL => {
            let id = r.u32().map_err(malformed)?;
            let name = r.text().map_err(malformed)?;
            let len = r.rest().len();
            let args = Value::decode(&mut r, budget).map_err(malformed)?;
            net.call(now_us, id, name, len)?;
            Action::Call {
                id,
                name: name.to_string(),
                args,
            }
        }
        _ => return Err(Violation::NoKind),
    };
    r.finish().map_err(malformed)?;
    Ok(action)
}
