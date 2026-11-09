//! A client's half: the book of the props its connection was sent ([`PropBook`]), the collision source its prediction meets
//! them through, and the shape each is drawn as.
//!
//! Everything here comes from a server, so it is held to caps: at most [`MAX_CLIENT_PROPS`] props in the book (a ghost past it is
//! left out), every state held to the wire's ranges by its decoder, a datablock's numbers clamped as the server's are, and a
//! shape file of at most [`crate::data::MAX_SHAPE_BYTES`] bytes drawn at a scale the renderer accepts.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use torque_collide::source::{
    CollisionShape, CollisionSource, MAX_SHAPES_PER_SOURCE, next_revision,
};
use torque_engine::net_classes::{DatablockFields, GhostEvent, ShapeDraw};
use torque_ghosts::Value;
use torque_ts_vm::vm::Vm;

use crate::data::{MAX_SHAPE_BYTES, PropNumbers, PropParams, draw_scale};
use crate::quant::Quantized;
use crate::records::PropGhost;

/// The most props a client keeps.
pub const MAX_CLIENT_PROPS: usize = 4096;

/// One prop as a client knows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClientProp {
    /// The datablock's id, and its numbers (none until the client has that datablock).
    pub data: Option<u32>,
    pub params: Option<PropParams>,
    /// The server's slot for it: its collision shape's key.
    pub slot: u16,
    /// Whether it is this client's own, and the predict id it was given.
    pub mine: bool,
    pub predict: u8,
    /// The state the server last sent: its own state, exactly.
    pub q: Quantized,
    pub asleep: bool,
    pub frozen: bool,
    /// Updates heard (a count, to see an update came).
    pub updates: u32,
    /// Where the client's prediction has it while it predicts it ([`crate::predict`]): what the client's own Player meets and a
    /// grapple's tether hangs from. None while it is shown as the server sent it.
    pub predicted: Option<Quantized>,
}

impl ClientProp {
    /// The state the client's own movers meet: the predicted one while it is predicted, else the server's.
    pub fn now(&self) -> &Quantized {
        self.predicted.as_ref().unwrap_or(&self.q)
    }
}

/// The props a client's connection was sent, by ghost index.
#[derive(Debug, Default)]
pub struct PropBook {
    props: BTreeMap<u32, ClientProp>,
    /// Ghosts left out because the book was full.
    pub dropped: u64,
    /// A revision of what the book holds ([`torque_collide::source::next_revision`] at every change; 0 for a book never
    /// changed): what its collision source answers.
    revision: u64,
}

impl PropBook {
    /// The revision of what the book holds: it changes at every record heard, removal and prediction.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn props(&self) -> &BTreeMap<u32, ClientProp> {
        &self.props
    }

    pub fn get(&self, index: u32) -> Option<&ClientProp> {
        self.props.get(&index)
    }

    pub fn len(&self) -> usize {
        self.props.len()
    }

    pub fn is_empty(&self) -> bool {
        self.props.is_empty()
    }

    /// A ghost's record, as a client's handler hears it (the whole record, updates merged); `params` reads a datablock's numbers.
    pub fn heard(
        &mut self,
        index: u32,
        record: &PropGhost,
        params: impl FnOnce(u32) -> Option<PropParams>,
    ) {
        let full = self.props.len() >= MAX_CLIENT_PROPS;
        self.revision = next_revision();
        let entry = match self.props.get_mut(&index) {
            Some(e) => e,
            None if full => {
                self.dropped += 1;
                return;
            }
            None => self.props.entry(index).or_insert(ClientProp {
                data: None,
                params: None,
                slot: 0,
                mine: false,
                predict: 0,
                q: Quantized::default(),
                asleep: false,
                frozen: false,
                updates: 0,
                predicted: None,
            }),
        };
        let data = record.data_block.0;
        if data != entry.data || entry.params.is_none() {
            entry.data = data;
            entry.params = data.and_then(params);
        }
        if let Some(init) = record.init {
            entry.slot = init.slot;
            entry.mine = init.mine;
            entry.predict = init.predict;
        }
        if let Some(state) = record.state {
            entry.q = state.0.held();
        }
        if let Some(flags) = record.flags {
            entry.asleep = flags.asleep;
            entry.frozen = flags.frozen;
        }
        entry.updates = entry.updates.wrapping_add(1);
    }

    /// A ghost the server deleted (or the connection ended).
    pub fn removed(&mut self, index: u32) {
        self.revision = next_revision();
        self.props.remove(&index);
    }

    /// Sets where the prediction has prop `index` (none: shown as the server sent it); false when the book has no such prop.
    pub fn set_predicted(&mut self, index: u32, q: Option<Quantized>) -> bool {
        let Some(p) = self.props.get_mut(&index) else {
            return false;
        };
        let q = q.map(Quantized::held);
        if p.predicted != q {
            p.predicted = q;
            self.revision = next_revision();
        }
        true
    }

    /// Every prop forgotten.
    pub fn clear(&mut self) {
        self.revision = next_revision();
        self.props.clear();
    }

    /// The props' states as words, in ghost order (the index, the slot, the thirteen integers, the flags), for a trace hash.
    pub fn state_words(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.props.len() * 16);
        for (&i, p) in &self.props {
            out.push(i);
            out.push(u32::from(p.slot));
            out.extend(p.q.words().map(|w| w as u32));
            out.push(u32::from(p.asleep) | u32::from(p.frozen) << 1 | u32::from(p.mine) << 2);
        }
        out
    }
}

/// The engine resource that holds a client's book. One client per engine (as with every plugin's client state).
#[derive(Clone, Debug, Default)]
pub struct ClientProps(pub Rc<RefCell<PropBook>>);

impl ClientProps {
    /// Reads the book, if no one is changing it.
    pub fn with<R>(&self, f: impl FnOnce(&PropBook) -> R) -> Option<R> {
        self.0.try_borrow().ok().map(|b| f(&b))
    }
}

/// A client datablock's field as a number.
fn number(vm: &Vm, id: u32, field: &[u8], index: usize) -> f32 {
    let text = vm.read_field_of(torque_ts_vm::vm::FieldTarget::Live(id), field, b"");
    let word = text
        .split(|b| *b == b' ' || *b == b'\t')
        .filter(|w| !w.is_empty())
        .nth(index);
    word.map_or(f32::NAN, |w| torque_ts_text::atof(w) as f32)
}

/// The numbers of the client's datablock `id` (the record's fields, by their names), none when there is no such object.
pub fn params_of(vm: &Vm, id: u32) -> Option<PropParams> {
    if !vm.objects().contains(id) {
        return None;
    }
    let n = |f: &[u8]| number(vm, id, f, 0);
    let numbers = PropNumbers {
        size: [0, 1, 2].map(|k| number(vm, id, b"size", k)),
        radius: n(b"radius"),
        mass: n(b"mass"),
        density: n(b"density"),
        friction: n(b"friction"),
        restitution: n(b"restitution"),
        linear_damping: n(b"linearDamping"),
        angular_damping: n(b"angularDamping"),
        max_speed: n(b"maxSpeed"),
        sleep_linear: n(b"sleepLinear"),
        sleep_angular: n(b"sleepAngular"),
        can_freeze: n(b"canFreeze") != 0.0,
        max_per_player: 0,
    };
    Some(numbers.held())
}

/// The client's handler of `Prop` ghosts.
pub fn heard(vm: &mut Vm, book: &ClientProps, event: GhostEvent<'_, PropGhost>) {
    let Ok(mut book) = book.0.try_borrow_mut() else {
        return;
    };
    match event {
        GhostEvent::Added { index, record } | GhostEvent::Changed { index, record } => {
            book.heard(index, record, |id| params_of(vm, id));
        }
        GhostEvent::Removed { index } => book.removed(index),
    }
}

/// The props as the predicting client's collision source: the boxes the server's source gives, from the states it sent (where
/// the client predicts a prop, from its predicted state), keyed by the server's slots, so both sides have the same shapes.
#[derive(Debug, Clone)]
pub struct ClientSource(pub ClientProps);

impl CollisionSource for ClientSource {
    fn shapes(&self, out: &mut Vec<CollisionShape>) {
        let _ = self.0.with(|book| {
            for p in book.props().values() {
                let Some(params) = p.params else { continue };
                if usize::from(p.slot) >= MAX_SHAPES_PER_SOURCE {
                    continue;
                }
                out.push(crate::server::shape_of(
                    p.slot,
                    p.now(),
                    params.shape.half_extents(),
                ));
            }
        });
    }

    /// The book's [`PropBook::revision`]; none while it is being changed.
    fn revision(&self) -> Option<u64> {
        self.0.with(PropBook::revision)
    }
}

/// What a prop is drawn as: its datablock's `shapeFile` at its `drawScale` (none for no shape, or a datablock the client lacks).
pub fn draw(record: &PropGhost, datablocks: &dyn DatablockFields) -> Option<ShapeDraw> {
    let id = record.data_block.0?;
    let shape = datablocks.text(id, "shapeFile")?;
    if shape.is_empty() || shape.len() > MAX_SHAPE_BYTES {
        return None;
    }
    let element = |k: u32| match datablocks.field(id, "drawScale", k) {
        Some(Value::F32(v)) => *v,
        _ => 1.0,
    };
    let scale = draw_scale([element(0), element(1), element(2)]);
    Some(ShapeDraw::new(shape).scaled(scale))
}
