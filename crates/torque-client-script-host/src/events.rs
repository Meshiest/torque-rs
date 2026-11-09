//! The events waiting for a realm's next frame. Each is checked against the format when it is queued, so a batch the
//! host hands over always decodes (the guest refuses a malformed batch whole, which would stop the realm). Repeated
//! state events (a hover, a scroll, a value change of one control, the screen) keep only the newest, so a realm that
//! falls behind gets the player's latest input, not a backlog of it.

use std::collections::VecDeque;

use torque_client_script_format::{
    MAX_BATCH_BYTES, MAX_BATCH_NODES, MAX_EVENTS, MAX_RECORD, NodeBudget, Reader, Records, Tier,
    Value, Writer, event,
};

/// The most events the host keeps waiting for a realm.
pub(crate) const MAX_WAITING: usize = MAX_EVENTS;

/// One waiting event.
#[derive(Clone, Debug)]
struct Waiting {
    tier: Tier,
    kind: u8,
    body: Vec<u8>,
    /// The values and keys its body decodes to.
    nodes: usize,
}

impl Waiting {
    /// What a newer event replaces this one by: its kind and its control.
    fn key(&self) -> Option<(u8, u32)> {
        if self.tier != Tier::Ui {
            return None;
        }
        match self.kind {
            event::ui::HOVER | event::ui::SCROLL | event::ui::CHANGE => {
                Reader::new(&self.body).u32().ok().map(|h| (self.kind, h))
            }
            event::ui::SCREEN => Some((self.kind, 0)),
            _ => None,
        }
    }

    /// Whether dropping it loses only input (the realm's own controls'), not a message or a revocation.
    fn is_input(&self) -> bool {
        self.tier == Tier::Ui
    }

    fn len(&self) -> usize {
        6 + self.body.len()
    }
}

/// The waiting events, oldest first.
#[derive(Debug, Default)]
pub(crate) struct EventQueue {
    waiting: VecDeque<Waiting>,
}

impl EventQueue {
    /// Queues an event whose body was checked; `false` when it has no room in a batch.
    fn push(&mut self, tier: Tier, kind: u8, body: Vec<u8>, nodes: usize) -> bool {
        if body.len() > MAX_RECORD {
            return false;
        }
        let waiting = Waiting {
            tier,
            kind,
            body,
            nodes,
        };
        if let Some(key) = waiting.key()
            && let Some(at) = self.waiting.iter().position(|w| w.key() == Some(key))
        {
            self.waiting.remove(at);
        }
        if self.waiting.len() >= MAX_WAITING {
            self.make_room();
        }
        self.waiting.push_back(waiting);
        true
    }

    /// Makes room for one event: drops the oldest input event, else the oldest `net` event (a dropped message leaves
    /// one `overflow` in its place); never a revocation.
    fn make_room(&mut self) {
        if let Some(at) = self.waiting.iter().position(Waiting::is_input) {
            self.waiting.remove(at);
            return;
        }
        let is_overflow = |w: &Waiting| w.tier == Tier::Net && w.kind == event::net::OVERFLOW;
        let droppable = |w: &Waiting| w.tier == Tier::Net && !is_overflow(w);
        let Some(at) = self.waiting.iter().position(droppable) else {
            return;
        };
        let dropped = self.waiting.remove(at);
        if dropped.is_none_or(|w| w.kind != event::net::MESSAGE)
            || self.waiting.iter().any(is_overflow)
        {
            return;
        }
        // The `overflow` needs a place of its own.
        if let Some(at) = self.waiting.iter().position(droppable) {
            self.waiting.remove(at);
        }
        self.waiting.push_back(Waiting {
            tier: Tier::Net,
            kind: event::net::OVERFLOW,
            body: Vec::new(),
            nodes: 0,
        });
    }

    /// Queues `realm.revoked(tier)` ahead of everything: the next batch carries it.
    pub(crate) fn revoked(&mut self, tier: Tier) {
        self.waiting.push_front(Waiting {
            tier: Tier::Realm,
            kind: event::realm::REVOKED,
            body: vec![tier.id()],
            nodes: 0,
        });
    }

    /// Drops every waiting event of `tier`.
    pub(crate) fn drop_tier(&mut self, tier: Tier) {
        self.waiting.retain(|w| w.tier != tier);
    }

    /// Drops everything.
    pub(crate) fn clear(&mut self) {
        self.waiting.clear();
    }

    /// The events waiting.
    pub(crate) fn len(&self) -> usize {
        self.waiting.len()
    }

    /// The next batch: the oldest events, within the format's caps on records, bytes and values.
    pub(crate) fn take_batch(&mut self) -> Vec<u8> {
        let mut out = Writer::new();
        let (mut count, mut nodes) = (0, 0);
        while let Some(next) = self.waiting.front() {
            if count == MAX_EVENTS
                || out.len() + next.len() > MAX_BATCH_BYTES
                || nodes + next.nodes > MAX_BATCH_NODES
            {
                break;
            }
            let next = self.waiting.pop_front().expect("the front was there");
            count += 1;
            nodes += next.nodes;
            out.u8(next.tier.id());
            out.u8(next.kind);
            out.u32(next.body.len() as u32);
            out.raw(&next.body);
        }
        out.into_bytes()
    }

    /// Queues the `ui` events of `batch` (the GUI's records for the realm) that follow the format; the number dropped.
    pub(crate) fn ui_batch(&mut self, batch: &[u8]) -> usize {
        let Ok(records) = Records::new(batch, usize::MAX, usize::MAX) else {
            return 0;
        };
        let mut dropped = 0;
        for record in records {
            let Ok(record) = record else {
                return dropped + 1;
            };
            let nodes = (record.tier == Tier::Ui.id())
                .then(|| ui_event_nodes(record.kind, record.body))
                .flatten();
            let queued =
                nodes.is_some_and(|n| self.push(Tier::Ui, record.kind, record.body.to_vec(), n));
            dropped += usize::from(!queued);
        }
        dropped
    }

    /// Queues a `net` event whose body `write` makes, with `values` in it; `false` when they have no encoding or no
    /// room in a batch.
    pub(crate) fn net(
        &mut self,
        kind: u8,
        write: impl FnOnce(&mut Writer),
        values: &[&Value],
    ) -> bool {
        let mut out = Writer::new();
        write(&mut out);
        for value in values {
            if value.encode(&mut out).is_err() {
                return false;
            }
        }
        let body = out.into_bytes();
        let Some(nodes) = net_event_nodes(kind, &body) else {
            return false;
        };
        self.push(Tier::Net, kind, body, nodes)
    }
}

/// The values a `net` event's body decodes to, or `None` when it does not follow the format.
fn net_event_nodes(kind: u8, body: &[u8]) -> Option<usize> {
    let mut r = Reader::new(body);
    let mut budget = NodeBudget::default();
    match kind {
        event::net::MESSAGE => {}
        event::net::REPLY => {
            r.u32().ok()?;
            r.bool().ok()?;
        }
        event::net::RPC => {
            r.text().ok()?;
        }
        _ => return None,
    }
    Value::decode(&mut r, &mut budget).ok()?;
    r.finish().ok()?;
    Some(MAX_BATCH_NODES - budget.left())
}

/// The values a `ui` event's body decodes to, or `None` when it does not follow the format.
fn ui_event_nodes(kind: u8, body: &[u8]) -> Option<usize> {
    let mut r = Reader::new(body);
    let mut budget = NodeBudget::default();
    match kind {
        event::ui::CLICK | event::ui::PRESS => {
            r.u32().ok()?;
        }
        event::ui::HOVER => {
            r.u32().ok()?;
            r.bool().ok()?;
        }
        event::ui::CHANGE => {
            r.u32().ok()?;
            Value::decode(&mut r, &mut budget).ok()?;
        }
        event::ui::SUBMIT => {
            r.u32().ok()?;
            r.text().ok()?;
        }
        event::ui::SELECT => {
            r.u32().ok()?;
            r.u32().ok()?;
        }
        event::ui::SCROLL => {
            r.u32().ok()?;
            r.finite().ok()?;
            r.finite().ok()?;
        }
        event::ui::SCREEN => {
            r.finite().ok()?;
            r.finite().ok()?;
            r.finite().ok()?;
        }
        event::ui::CLOSED => {}
        _ => return None,
    }
    r.finish().ok()?;
    Some(MAX_BATCH_NODES - budget.left())
}
