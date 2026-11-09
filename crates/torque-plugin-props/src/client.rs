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
    /// A player holds it: its holder passes through it ([`ClientSource`]).
    pub held: bool,
    /// Updates heard (a count, to see an update came).
    pub updates: u32,
    /// Where the client's prediction has it while it predicts it ([`crate::predict`]): what the client's own Player meets and a
    /// grapple's tether hangs from. None while it is shown as the server sent it.
    pub predicted: Option<Quantized>,
    /// Its place in the scene hierarchy ([`Placed`]), as the client last found it.
    pub place: Placed,
}

/// Where a prop is in the scene hierarchy, as a client's ghosts have it (found once a tick, [`PropBook::set_place`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Placed {
    /// It hangs on a parent: its record's pose is its world pose when it was sent, and `pose` is where the parent and its last local
    /// pose put it.
    pub child: bool,
    /// Other ghosts hang on it.
    pub carries: bool,
    /// A child's pose for a prediction tick: its parents' latest poses times its local pose.
    pub pose: Option<Quantized>,
}

impl ClientProp {
    /// The state the client's own movers meet: the predicted one while it is predicted, a child's where its parent puts it, else
    /// the server's.
    pub fn now(&self) -> &Quantized {
        self.predicted
            .as_ref()
            .or(self.place.pose.as_ref())
            .unwrap_or(&self.q)
    }

    /// Whether the prop is in the hierarchy (a child or a parent): the client's prediction does not simulate it, as its body is not
    /// the one its record says.
    pub fn is_linked(&self) -> bool {
        self.place.child || self.place.carries
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
    /// Each prop by its slot and ghost index: a slot's shape is its lowest ghost's.
    by_slot: std::collections::BTreeSet<(u16, u32)>,
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

    /// The prop whose collision shape is keyed `slot` (the lowest ghost of two that hold it while a slot changes hands).
    pub fn in_slot(&self, slot: u16) -> Option<(u32, &ClientProp)> {
        let &(_, g) = self.by_slot.range((slot, 0)..=(slot, u32::MAX)).next()?;
        Some((g, self.props.get(&g)?))
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
            None => {
                self.by_slot.insert((0, index));
                self.props.entry(index).or_insert(ClientProp {
                    data: None,
                    params: None,
                    slot: 0,
                    mine: false,
                    predict: 0,
                    q: Quantized::default(),
                    asleep: false,
                    frozen: false,
                    held: false,
                    updates: 0,
                    predicted: None,
                    place: Placed::default(),
                })
            }
        };
        let data = record.data_block.0;
        if data != entry.data || entry.params.is_none() {
            entry.data = data;
            entry.params = data.and_then(params);
        }
        if let Some(init) = record.init {
            if entry.slot != init.slot {
                self.by_slot.remove(&(entry.slot, index));
                self.by_slot.insert((init.slot, index));
            }
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
            entry.held = flags.held;
        }
        entry.updates = entry.updates.wrapping_add(1);
    }

    /// A ghost the server deleted (or the connection ended).
    pub fn removed(&mut self, index: u32) {
        self.revision = next_revision();
        if let Some(p) = self.props.remove(&index) {
            self.by_slot.remove(&(p.slot, index));
        }
    }

    /// Sets where prop `index` is in the hierarchy; false when the book has no such prop.
    pub fn set_place(&mut self, index: u32, place: Placed) -> bool {
        let Some(p) = self.props.get_mut(&index) else {
            return false;
        };
        if p.place != place {
            p.place = place;
            self.revision = next_revision();
        }
        true
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
        self.by_slot.clear();
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

/// Finds where each prop of the book is in the scene hierarchy: a child's pose for a prediction tick, from its parents' latest
/// records and its last local pose, and whether it carries others. Once a tick, after the client's other work (the client's
/// state is not being changed then, so the lookup answers).
pub fn refresh_places(vm: &Vm, book: &ClientProps) {
    let Some(lookup) = torque_engine::net_classes::ClientGhostLookup::of(vm) else {
        return;
    };
    let Ok(mut book) = book.0.try_borrow_mut() else {
        return;
    };
    let indexes: Vec<u32> = book.props().keys().copied().collect();
    for index in indexes {
        let child = lookup.0.parent(index).is_some();
        let carries = lookup.0.has_children(index);
        let pose = if child {
            lookup
                .0
                .tick_pose(index)
                .and_then(|m| crate::quant::pose_of(&m))
                .map(|(position, rotation)| {
                    Quantized::of(&torque_physics_rapier::BodyState {
                        position,
                        rotation,
                        ..Default::default()
                    })
                })
        } else {
            None
        };
        book.set_place(
            index,
            Placed {
                child,
                carries,
                pose,
            },
        );
    }
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
/// the client predicts a prop, from its predicted state), keyed by the server's slots, so both sides have the same shapes; a
/// held prop's holder (its ghost, from the joints' book) passes through it, as on the server.
#[derive(Debug, Clone)]
pub struct ClientSource {
    pub book: ClientProps,
    pub joints: crate::joints::client::ClientJoints,
}

impl ClientSource {
    /// The shape of prop ghost `ghost`, its holder passing through it.
    fn shape_of(&self, ghost: u32, p: &ClientProp) -> Option<CollisionShape> {
        let params = p.params?;
        let mut shape = crate::server::shape_of(p.slot, p.now(), params.shape.half_extents());
        if p.held {
            shape.passes = self
                .joints
                .0
                .try_borrow()
                .ok()
                .and_then(|j| j.holder_of(ghost));
        }
        Some(shape)
    }
}

impl CollisionSource for ClientSource {
    fn shapes(&self, out: &mut Vec<CollisionShape>) {
        let _ = self.book.with(|book| {
            for (&g, p) in book.props() {
                if usize::from(p.slot) >= MAX_SHAPES_PER_SOURCE {
                    continue;
                }
                out.extend(self.shape_of(g, p));
            }
        });
    }

    /// The prop in slot `key`, as [`ClientSource::shapes`] lists it.
    fn shape(&self, key: u32) -> Option<CollisionShape> {
        let slot = u16::try_from(key).ok()?;
        self.book
            .with(|book| {
                let (g, p) = book.in_slot(slot)?;
                self.shape_of(g, p)
            })
            .flatten()
    }

    /// The book's [`PropBook::revision`] (a hold made or let go changes its prop's `held` flag, so the book's revision); none
    /// while it is being changed.
    fn revision(&self) -> Option<u64> {
        self.book.with(PropBook::revision)
    }
}

/// What a prop is drawn as: its datablock's `shapeFile` at its `drawScale` (none for no shape, or a datablock the client
/// lacks), resting while it sleeps or is frozen (a client may then draw it with its still geometry).
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
    let rests = record.flags.is_some_and(|f| f.asleep || f.frozen);
    Some(ShapeDraw::new(shape).scaled(scale).resting(rests))
}
