//! The server's uploads: the offer handler and the per-tick pump.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};

use torque_net::handshake::ConnId;
use torque_transfer::{Counters, Offer, Out, StagingConfig, Subject, UploadServer};
use torque_ts_vm::vm::Vm;

use super::links::LinkError;
use super::{
    BeginReply, BeginUpload, Refusal, ServerLinks, StagingEntry, UploadCx, UploadEnd, UploadEntry,
    UploadInbox, UploadRequest,
};
use crate::permissions::SubjectKey;
use crate::rpc::RpcCx;
use crate::{Engine, TickInfo};

/// The longest reason that crosses (the RPC's strings are cut at 255 bytes).
pub(super) const MAX_REASON: usize = 200;
/// The most `end` messages the server keeps for connections that cannot take them yet.
const MAX_PENDING_ENDS: usize = 256;
/// The sinks' `finish` that run in one tick.
const FINISH_PER_TICK: usize = 1;

fn subject_of(key: &SubjectKey) -> Subject {
    match key {
        SubjectKey::Identity(identity) => Subject::Identity(identity.clone()),
        SubjectKey::Connection(conn) => Subject::Connection(ConnId(*conn)),
    }
}

fn key_of(subject: &Subject) -> SubjectKey {
    match subject {
        Subject::Identity(identity) => SubjectKey::Identity(identity.clone()),
        Subject::Connection(conn) => SubjectKey::Connection(conn.0),
    }
}

/// `text` cut to at most [`MAX_REASON`] bytes on a character boundary.
pub(super) fn short(text: &str) -> String {
    let mut end = text.len().min(MAX_REASON);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn refused(reason: &Refusal) -> BeginReply {
    BeginReply {
        accepted: false,
        reason: short(&reason.to_string()),
        ..BeginReply::default()
    }
}

/// What the server has to say to a client later: an end that its connection could not take yet.
struct PendingEnd {
    conn: ConnId,
    end: UploadEnd,
}

/// What a tick carries over to the next.
#[derive(Default)]
struct Carry {
    ends: VecDeque<PendingEnd>,
    finishing: VecDeque<u32>,
    /// What the channel delivered in a tick whose transfer state was in use, handled in the next.
    inbox: UploadInbox,
}

/// The server's uploads: an extension of its script engine, made by the engine's recipes.
pub struct ServerUploads {
    entries: Vec<UploadEntry>,
    server: RefCell<UploadServer>,
    carry: RefCell<Carry>,
    /// What each connection had delivered on the upload channel when the last tick looked.
    seen: RefCell<HashMap<ConnId, u64>>,
    finished: Cell<u64>,
}

impl ServerUploads {
    pub(super) fn new(
        engine: &Engine,
        host: Option<StagingConfig>,
    ) -> Result<ServerUploads, String> {
        let entries = engine.list::<UploadEntry>().to_vec();
        let kinds = entries
            .iter()
            .map(|e| (e.name.clone(), e.spec.clone()))
            .collect();
        let staging = engine
            .list::<StagingEntry>()
            .last()
            .map(|s| s.0.clone())
            .or(host)
            .unwrap_or_default();
        let server = UploadServer::new(kinds, staging)?;
        Ok(ServerUploads {
            entries,
            server: RefCell::new(server),
            carry: RefCell::new(Carry::default()),
            seen: RefCell::new(HashMap::new()),
            finished: Cell::new(0),
        })
    }

    /// What is worth telling whoever runs the server when it starts: why staging cannot use its folder, and which kinds therefore
    /// refuse uploads that do not fit in memory.
    pub(super) fn startup_warnings(&self) -> Vec<String> {
        let Ok(server) = self.server.try_borrow() else {
            return Vec::new();
        };
        let mut warnings: Vec<String> = server
            .staging_warning()
            .map(str::to_string)
            .into_iter()
            .collect();
        let staging = server.staging();
        if staging.folder.is_none() {
            let memory = staging.memory_per_upload as u64;
            for entry in &self.entries {
                if entry.spec.max_bytes > memory {
                    warnings.push(format!(
                        "uploads `{}`: there is no staging folder, so uploads above {memory} bytes are refused (the kind takes up to {})",
                        entry.name, entry.spec.max_bytes
                    ));
                }
            }
        }
        warnings
    }

    /// The transfer state, for a test that holds it as a running sink would.
    #[cfg(test)]
    pub(super) fn state_for_test(&self) -> &RefCell<UploadServer> {
        &self.server
    }

    /// What the transfer server counted since it started; `None` while a sink that was handed the script engine is running.
    pub fn counters(&self) -> Option<Counters> {
        self.server.try_borrow().ok().map(|s| s.counters())
    }

    /// The staging caps and folder in use; `None` while a sink is running.
    pub fn staging(&self) -> Option<StagingConfig> {
        self.server.try_borrow().ok().map(|s| s.staging().clone())
    }

    /// The staging folder's problem, if it could not be used; `None` too while a sink is running.
    pub fn staging_warning(&self) -> Option<String> {
        self.server
            .try_borrow()
            .ok()
            .and_then(|s| s.staging_warning().map(str::to_string))
    }

    fn entry(&self, name: &str) -> Option<&UploadEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Runs `f` on the transfer server, or says on `vm`'s console that `what` could not run because a sink has it in use.
    fn with_server<R>(
        &self,
        vm: &mut Vm,
        what: &str,
        f: impl FnOnce(&mut UploadServer) -> R,
    ) -> Option<R> {
        match self.server.try_borrow_mut() {
            Ok(mut server) => Some(f(&mut server)),
            Err(_) => {
                vm.warn(format!(
                    "uploads: {what} waits: a sink holds the transfer state while it runs script"
                ));
                None
            }
        }
    }

    /// A client's offer: the kind's permission, the limits and the sink's rules, then a session or a reason.
    pub(super) fn begin(&self, cx: &mut RpcCx<'_>, req: &BeginUpload) -> BeginReply {
        let Some(conn) = cx.caller() else {
            return refused(&Refusal::Invalid("no connection"));
        };
        let Some(entry) = self.entry(&req.kind) else {
            return refused(&Refusal::UnknownKind);
        };
        if let Some(permission) = &entry.spec.permission
            && !cx.has_permission(permission)
        {
            return refused(&Refusal::NotPermitted);
        }
        let key = cx.subject().unwrap_or(SubjectKey::Connection(conn.0));
        let subject = subject_of(&key);
        let size = u64::from(req.size_low) | (u64::from(req.size_high) << 32);
        let offer = Offer {
            kind: &req.kind,
            meta: &req.meta,
            size,
            hash: req.hash,
        };
        let tick = cx.tick();
        let now = tick * TickInfo::MS;
        let perms = cx.permission_view();
        let Ok(mut server) = self.server.try_borrow_mut() else {
            cx.vm.warn(
                "uploads: an offer was refused: a sink holds the transfer state while it runs script"
                    .to_string(),
            );
            return refused(&Refusal::ServerBusy);
        };
        let result = server.begin(now, conn, &subject, &offer, |meta| {
            let mut sink = entry
                .sink
                .try_borrow_mut()
                .map_err(|_| "The server is busy; try again.".to_string())?;
            let mut ucx = UploadCx {
                vm: &mut *cx.vm,
                conn,
                subject: &key,
                tick,
                perms,
            };
            sink.accept(&mut ucx, meta)
        });
        match result {
            Ok(accepted) => BeginReply {
                accepted: true,
                id: accepted.id,
                has_list: accepted.has_list,
                have: accepted.have,
                reason: String::new(),
            },
            Err(reason) => refused(&reason),
        }
    }

    pub(super) fn cancel(&self, vm: &mut Vm, tick: u64, conn: ConnId, id: u32) {
        self.with_server(vm, "a cancel", |server| {
            server.cancel(tick * TickInfo::MS, conn, id);
        });
    }

    /// One tick's work ([module docs](crate::upload)).
    pub(super) fn pump(&self, vm: &mut Vm, links: &dyn ServerLinks, tick: TickInfo) {
        let now = tick.time_ms;
        let mut inbox = std::mem::take(&mut self.carry.borrow_mut().inbox);
        inbox.absorb(links.take_inbox());
        let mut drops: Vec<ConnId> = std::mem::take(&mut inbox.flooders);
        let mut outs: Vec<Out> = Vec::new();
        let ran = self.with_server(vm, "the tick's messages", |server| {
            self.note_deliveries(now, links, server);
            for (conn, bytes) in std::mem::take(&mut inbox.messages) {
                if drops.contains(&conn) {
                    continue;
                }
                match server.on_message(now, conn, &bytes) {
                    Ok(out) => outs.extend(out),
                    Err(_) => drops.push(conn),
                }
            }
            for conn in inbox.gone.iter().chain(&drops) {
                server.disconnected(now, *conn);
            }
            outs.extend(server.poll(now));
        });
        if ran.is_none() {
            // Nothing was handled; what the channel delivered waits for the next tick.
            inbox.flooders = drops;
            self.carry.borrow_mut().inbox.absorb(inbox);
            return;
        }
        self.run_outs(vm, links, outs, &mut drops);
        self.finish_some(vm, links, tick);
        self.send_ends(links);
        drops.sort_unstable();
        drops.dedup();
        if !drops.is_empty() {
            let violations = self.counters().map(|c| c.violations).unwrap_or_default();
            vm.set_global("$Server::UploadViolations", &violations.to_string());
        }
        for conn in drops {
            links.drop_conn(vm, conn, b"Packet Error.");
        }
    }

    /// Tells the transfer server how much each connection with an upload has delivered since the last tick, so a slow link that is
    /// still delivering is not taken for a stalled one.
    fn note_deliveries(&self, now: u64, links: &dyn ServerLinks, server: &mut UploadServer) {
        let connections = server.connections();
        let mut seen = self.seen.borrow_mut();
        seen.retain(|conn, _| connections.contains(conn));
        for conn in connections {
            let total = links.received(conn);
            let before = seen.insert(conn, total).unwrap_or(total);
            server.delivered(now, conn, total.saturating_sub(before));
        }
    }

    fn run_outs(
        &self,
        vm: &mut Vm,
        links: &dyn ServerLinks,
        outs: Vec<Out>,
        drops: &mut Vec<ConnId>,
    ) {
        let mut retract: Vec<UploadRequest> = Vec::new();
        {
            let mut carry = self.carry.borrow_mut();
            for out in outs {
                match out {
                    Out::Request {
                        conn,
                        id,
                        start,
                        count,
                    } => {
                        let request = UploadRequest { id, start, count };
                        if links.request(conn, request.clone()).is_err() {
                            retract.push(request);
                        }
                    }
                    Out::End { conn, id, result } => {
                        carry.ends.push_back(PendingEnd {
                            conn,
                            end: end_message(id, result),
                        });
                    }
                    Out::Finish { id } => carry.finishing.push_back(id),
                    Out::Drop { conn } => drops.push(conn),
                    _ => {}
                }
            }
            while carry.ends.len() > MAX_PENDING_ENDS {
                carry.ends.pop_front();
            }
        }
        if !retract.is_empty() {
            self.with_server(
                vm,
                "taking back requests the connection could not send",
                |server| {
                    for r in retract {
                        server.retract(r.id, r.start, r.count);
                    }
                },
            );
        }
    }

    /// Runs the sink of at most [`FINISH_PER_TICK`] finished uploads.
    fn finish_some(&self, vm: &mut Vm, links: &dyn ServerLinks, tick: TickInfo) {
        for _ in 0..FINISH_PER_TICK {
            let Some(id) = self.carry.borrow_mut().finishing.pop_front() else {
                return;
            };
            let Ok(mut server) = self.server.try_borrow_mut() else {
                vm.warn(
                    "uploads: a sink waits: another holds the transfer state while it runs script"
                        .to_string(),
                );
                self.carry.borrow_mut().finishing.push_front(id);
                return;
            };
            let Some((conn, subject)) = server.owner(id).map(|(c, s)| (c, s.clone())) else {
                server.complete(id, Err("The upload was lost.".to_string()));
                continue;
            };
            let key = key_of(&subject);
            let meta = server.meta(id).unwrap_or_default().to_vec();
            let entry = server.kind_of(id).and_then(|name| self.entry(name));
            let result = match (entry, server.content(id)) {
                (Some(entry), Some(bytes)) => {
                    let perms = links.permissions(conn);
                    // The player may have lost the permission while the upload was coming: it is checked again.
                    let allowed = entry
                        .spec
                        .permission
                        .as_ref()
                        .is_none_or(|p| perms.has_permission(p));
                    if allowed {
                        let mut ucx = UploadCx {
                            vm: &mut *vm,
                            conn,
                            subject: &key,
                            tick: tick.number,
                            perms: perms.as_ref(),
                        };
                        match entry.sink.try_borrow_mut() {
                            Ok(mut sink) => sink.finish(&mut ucx, &meta, bytes),
                            Err(_) => Err("The server is busy; try again.".to_string()),
                        }
                    } else {
                        Err(Refusal::NotPermitted.to_string())
                    }
                }
                _ => Err("The upload was lost.".to_string()),
            };
            if result.is_ok() {
                self.finished.set(self.finished.get() + 1);
            }
            if let Some(Out::End { conn, id, result }) = server.complete(id, result) {
                self.carry.borrow_mut().ends.push_back(PendingEnd {
                    conn,
                    end: end_message(id, result),
                });
            }
            vm.set_global("$Server::UploadsFinished", &self.finished.get().to_string());
        }
    }

    /// Sends the queued ends, in order; one the connection cannot take yet waits for the next tick, and one for a connection that is
    /// gone is dropped.
    fn send_ends(&self, links: &dyn ServerLinks) {
        let mut carry = self.carry.borrow_mut();
        while let Some(pending) = carry.ends.front() {
            match links.end(pending.conn, pending.end.clone()) {
                Err(LinkError::Busy) => break,
                Ok(()) | Err(LinkError::Gone) => {
                    carry.ends.pop_front();
                }
            }
        }
    }
}

fn end_message(id: u32, result: Result<(), String>) -> UploadEnd {
    match result {
        Ok(()) => UploadEnd {
            id,
            ok: true,
            reason: String::new(),
        },
        Err(reason) => UploadEnd {
            id,
            ok: false,
            reason: short(&reason),
        },
    }
}
