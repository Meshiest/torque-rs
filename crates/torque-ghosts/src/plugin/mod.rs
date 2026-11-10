//! Plugin classes: ghost and datablock classes a plugin adds beside TGE's, carried by the `Plugin` arms of the engine's closed
//! sets ([`TgeClass::Plugin`](crate::TgeClass::Plugin), [`TgeRecord::Plugin`](crate::TgeRecord::Plugin) and their datablock
//! twins).
//!
//! - A record is a struct of [`field`]s: `#[derive(GhostRecord)]` makes it a [`GhostRecord`] (a ghost's update, packed from the
//!   object's fields), `#[derive(DatablockCodec)]` a [`DatablockCodec`] (a datablock's `packData`). Both are [`NetRecord`]s:
//!   a [`Layout`] like every record of this crate (one function run by the encoder, the decoder of hostile bytes and the logging
//!   encoder), with what the client's world asks of it. A record of any plugin class travels as a [`Box<dyn Record>`](Record).
//! - A class is a [`PluginClass`]: its name (the console class's: an object of that class is ghosted with this record), its kind
//!   and its codec. It is a small `Copy` handle, so the class of a record says how to read it.
//! - A game's plugin classes are a [`PluginClasses`], the same on the server and on its clients. **Ids**: each kind is sorted
//!   by name (bytes compared unsigned, as the net class table sorts) and a class's id is the id past the end of the
//!   connection's TGE table and the engine's own classes, plus its place: objects from `table + ENGINE_OBJECT_CLASSES.len()`
//!   ([`PluginClasses::ghost_base`]), datablocks from the table's end. Both sides derive them from the names alone, so TGE's own ids
//!   and records are unchanged, and a class id past the last plugin class is refused like any unknown class.
//! - **Room**: TGE's tables leave [`PLUGIN_GHOST_ROOM`] object ids and [`PLUGIN_DATABLOCK_ROOM`] datablock ids in their class id
//!   widths (6 bits each); a set with more is refused, since the widths are TGE's wire format.
//!
//! Like the engine's own classes ([`crate::light`]), plugin classes are ghosted to the engine's own clients only: a TGE client
//! cannot read their records.

pub mod draw;
pub mod field;
pub mod message;

use std::any::{Any, TypeId};
use std::fmt;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
use torque_net::classes::Kind;
use torque_net::huffman::Table;

use crate::block::BLOCK_MASK;
use crate::codec::{Layout, NetWidths, R, ReadCtx, Xf, decode, encode};
use crate::light::ENGINE_OBJECT_CLASSES;
use crate::tge::{TGE_DATABLOCK_CLASSES, TGE_OBJECT_CLASSES};
use crate::{ChargeFn, ObjectView, PackCtx, Value};

pub use draw::{DatablockFields, DrawnShapes, PluginStills, ShapeDraw, ShapeFn, Still};
pub use field::{DatablockRef, FieldSpec, GhostRef, NetField};
pub use message::{NetMessage, ScriptArgs};

/// How a client shows a plugin ghost between the server's updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Replicate {
    /// The client runs the class's own simulation ahead of the server (it must be bit-exact on every platform).
    Predicted,
    /// The client shows the server's last states, blended between updates.
    Interpolated,
}

impl Replicate {
    /// The mark's name, as a manifest or a log line shows it.
    pub fn label(self) -> &'static str {
        match self {
            Replicate::Predicted => "predicted",
            Replicate::Interpolated => "interpolated",
        }
    }
}

/// A record of a plugin class: a [`Layout`] (the derives write it) with what the client's world asks of it.
pub trait NetRecord: Layout + Debug + PartialEq + Send + Sync + 'static {
    /// An update merged into the kept record: each field replaced, an `Option` that was not sent kept.
    fn merge(&mut self, sent: &Self);

    /// The datablock the record names (its first [`DatablockRef`]).
    fn datablock(&self) -> Option<u32>;

    /// The record's transform (its first [`Xf`]).
    fn transform(&self) -> Option<Xf>;

    /// The record as script fields: (field, element, value), in the order of the struct (a client's datablock holds these).
    fn script_fields(&self) -> Vec<(String, u32, Value)>;
}

/// A plugin ghost's record (`#[derive(GhostRecord)]`): what a server sends of an object of the class.
pub trait GhostRecord: NetRecord {
    /// Packs the object as one connection sees it: the record and the mask bits to send again (none for a derived record).
    /// `mask` holds the dirty bits (all of them for a new ghost); a field with a `mask` is sent only when one of its bits is set.
    fn pack(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (Self, u32);
}

/// A plugin datablock's record (`#[derive(DatablockCodec)]`): its `packData`, read from the datablock's fields.
pub trait DatablockCodec: NetRecord {
    /// Packs the datablock from the host's view of it, in the connection's `widths`.
    fn pack(view: &dyn ObjectView, widths: NetWidths) -> Self;
}

/// A record of any plugin class, as the `Plugin` arms carry it.
pub trait Record: Debug + Send + Sync + 'static {
    /// Writes the record in the connection's `widths`.
    fn encode(&self, w: &mut BitWriter, table: &Table, widths: NetWidths) -> R;
    /// A copy.
    fn clone_record(&self) -> Box<dyn Record>;
    /// Whether `other` is a record of the same type with the same values.
    fn eq_record(&self, other: &dyn Record) -> bool;
    /// An update merged into this record ([`NetRecord::merge`]); one of another type replaces nothing (`false`).
    fn merge_record(&mut self, sent: &dyn Record) -> bool;
    /// [`NetRecord::datablock`].
    fn record_datablock(&self) -> Option<u32>;
    /// [`NetRecord::transform`].
    fn record_transform(&self) -> Option<Xf>;
    /// [`NetRecord::script_fields`].
    fn record_fields(&self) -> Vec<(String, u32, Value)>;
    /// The record, for a downcast to its type (`<dyn Record>::downcast_ref`).
    fn as_any(&self) -> &dyn Any;
}

impl<T: NetRecord> Record for T {
    fn encode(&self, w: &mut BitWriter, table: &Table, widths: NetWidths) -> R {
        encode(self, w, table, widths)
    }

    fn clone_record(&self) -> Box<dyn Record> {
        Box::new(self.clone())
    }

    fn eq_record(&self, other: &dyn Record) -> bool {
        other.as_any().downcast_ref::<T>() == Some(self)
    }

    fn merge_record(&mut self, sent: &dyn Record) -> bool {
        match sent.as_any().downcast_ref::<T>() {
            Some(sent) => {
                NetRecord::merge(self, sent);
                true
            }
            None => false,
        }
    }

    fn record_datablock(&self) -> Option<u32> {
        NetRecord::datablock(self)
    }

    fn record_transform(&self) -> Option<Xf> {
        NetRecord::transform(self)
    }

    fn record_fields(&self) -> Vec<(String, u32, Value)> {
        NetRecord::script_fields(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl dyn Record {
    /// The record as its own type, when it is one.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.as_any().downcast_ref()
    }
}

impl Clone for Box<dyn Record> {
    fn clone(&self) -> Box<dyn Record> {
        self.clone_record()
    }
}

impl PartialEq for Box<dyn Record> {
    fn eq(&self, other: &Box<dyn Record>) -> bool {
        self.eq_record(&**other)
    }
}

/// Records compare by their values; a record type whose `PartialEq` is not reflexive (a float field holding NaN) breaks it as
/// that type's own comparison does.
impl Eq for Box<dyn Record> {}

/// Pushes each script field value of `value` as field `name`, its elements numbered from 0 (for the derives).
pub fn push_values<T: NetField>(out: &mut Vec<(String, u32, Value)>, name: &str, value: &T) {
    let mut values = Vec::new();
    value.values(&mut values);
    out.extend(
        values
            .into_iter()
            .enumerate()
            .map(|(k, v)| (name.to_string(), k as u32, v)),
    );
}

// ---- the classes

type DecodeFn =
    fn(&mut BitReader, &Table, &mut ChargeFn, &ReadCtx) -> Result<Box<dyn Record>, NetError>;
type PackFn = fn(&dyn ObjectView, u32, &PackCtx) -> (Box<dyn Record>, u32);

/// A class as a [`PluginClass`] points at it. Made once per (record type, name, kind) and kept for the process.
struct ClassDef {
    name: &'static str,
    kind: Kind,
    replicate: Option<Replicate>,
    record: TypeId,
    record_name: &'static str,
    decode: DecodeFn,
    pack: PackFn,
}

fn decode_as<T: NetRecord>(
    r: &mut BitReader,
    table: &Table,
    charge: &mut ChargeFn,
    ctx: &ReadCtx,
) -> Result<Box<dyn Record>, NetError> {
    decode::<T>(r, table, charge, ctx).map(|rec| Box::new(rec) as Box<dyn Record>)
}

fn pack_ghost<G: GhostRecord>(
    view: &dyn ObjectView,
    mask: u32,
    ctx: &PackCtx,
) -> (Box<dyn Record>, u32) {
    let (record, kept) = G::pack(view, mask, ctx);
    (Box::new(record), kept)
}

fn pack_datablock<D: DatablockCodec>(
    view: &dyn ObjectView,
    _mask: u32,
    ctx: &PackCtx,
) -> (Box<dyn Record>, u32) {
    (Box::new(D::pack(view, ctx.widths)), 0)
}

/// A view of an object with no fields: what the block-bit probe packs from.
struct EmptyView;

impl ObjectView for EmptyView {
    fn get(&self, _field: &str, _index: usize) -> Value {
        Value::Null
    }
}

/// The classes made so far, so making the same class again (another engine built in the process) shares it.
fn interned(def: ClassDef) -> &'static ClassDef {
    static DEFS: OnceLock<Mutex<Vec<&'static ClassDef>>> = OnceLock::new();
    let mut defs = DEFS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(&found) = defs.iter().find(|d| {
        d.name == def.name
            && d.kind == def.kind
            && d.replicate == def.replicate
            && d.record == def.record
    }) {
        return found;
    }
    let made: &'static ClassDef = Box::leak(Box::new(def));
    defs.push(made);
    made
}

/// A plugin's ghost or datablock class: a `Copy` handle to its name, kind and codec. Two handles are equal when they are the
/// same class (the same record type under the same name and marks).
#[derive(Clone, Copy)]
pub struct PluginClass(&'static ClassDef);

impl PluginClass {
    /// A ghost class `name` whose records are `G`.
    pub fn ghost<G: GhostRecord>(name: &str, replicate: Replicate) -> PluginClass {
        PluginClass(interned(ClassDef {
            name: leak_name(name),
            kind: Kind::Object,
            replicate: Some(replicate),
            record: TypeId::of::<G>(),
            record_name: std::any::type_name::<G>(),
            decode: decode_as::<G>,
            pack: pack_ghost::<G>,
        }))
    }

    /// Whether the class's packer is seen to read or keep a mask bit of the per-ghost block: packed from a view of nothing
    /// (every field null, as an object that lacks them gives it) with only those bits dirty, it gives another record than with
    /// none, or keeps one of them. A best-effort check for a hand-written [`GhostRecord`]: it cannot see a packer that uses
    /// those bits only beside others, only for values that are set, or only by its context. The derive's compile-time check
    /// is the guard.
    fn reads_block_bits(self) -> bool {
        let ctx = PackCtx::default();
        let (block, kept) = (self.0.pack)(&EmptyView, BLOCK_MASK, &ctx);
        let (none, _) = (self.0.pack)(&EmptyView, 0, &ctx);
        kept & BLOCK_MASK != 0 || !block.eq_record(&*none)
    }

    /// A datablock class `name` whose records are `D`.
    pub fn datablock<D: DatablockCodec>(name: &str) -> PluginClass {
        PluginClass(interned(ClassDef {
            name: leak_name(name),
            kind: Kind::DataBlock,
            replicate: None,
            record: TypeId::of::<D>(),
            record_name: std::any::type_name::<D>(),
            decode: decode_as::<D>,
            pack: pack_datablock::<D>,
        }))
    }

    /// The class's name: the console class's, and the net class table's.
    pub fn name(self) -> &'static str {
        self.0.name
    }

    /// [`Kind::Object`] for a ghost class, [`Kind::DataBlock`] for a datablock class.
    pub fn kind(self) -> Kind {
        self.0.kind
    }

    /// A ghost class's mark (none for a datablock class).
    pub fn replicate(self) -> Option<Replicate> {
        self.0.replicate
    }

    /// The record type's name (for messages).
    pub fn record_type(self) -> &'static str {
        self.0.record_name
    }

    /// Whether the class's records are of type `G`.
    pub fn is_record<G: NetRecord>(self) -> bool {
        TypeId::of::<G>() == self.0.record
    }

    /// Whether `record` is a record of this class's type.
    pub fn holds(self, record: &dyn Record) -> bool {
        record.as_any().type_id() == self.0.record
    }

    /// `mNetFlags` of an object of the class: Ghostable (a datablock class: none).
    pub fn net_flags(self) -> u32 {
        match self.0.kind {
            Kind::Object => torque_net::ghost::net_flags::GHOSTABLE,
            _ => 0,
        }
    }

    /// Reads one record of the class from hostile bytes.
    pub fn decode(
        self,
        r: &mut BitReader,
        table: &Table,
        charge: &mut ChargeFn,
        ctx: &ReadCtx,
    ) -> Result<Box<dyn Record>, NetError> {
        (self.0.decode)(r, table, charge, ctx)
    }

    /// Packs an object (or a datablock: `mask` unused) of the class as one connection sees it: the record and the mask bits
    /// to send again.
    pub fn pack(self, view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (Box<dyn Record>, u32) {
        (self.0.pack)(view, mask, ctx)
    }
}

/// A class name kept for the process (the interned classes hold it).
fn leak_name(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<Vec<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(&found) = names.iter().find(|n| **n == name) {
        return found;
    }
    let made: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.push(made);
    made
}

impl PartialEq for PluginClass {
    fn eq(&self, other: &PluginClass) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl Eq for PluginClass {}

impl Hash for PluginClass {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(self.0, state);
    }
}

impl fmt::Debug for PluginClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PluginClass({:?})", self.0.name)
    }
}

// ---- the registry

/// The object class ids TGE's table leaves in its width past its own classes and the engine's (6 bits: 64 ids, 45 TGE
/// classes, 3 of the engine's).
pub const PLUGIN_GHOST_ROOM: usize = room(TGE_OBJECT_CLASSES.len(), ENGINE_OBJECT_CLASSES.len());

/// The datablock class ids TGE's table leaves in its width past its own classes (6 bits: 64 ids, 35 TGE classes).
pub const PLUGIN_DATABLOCK_ROOM: usize = room(TGE_DATABLOCK_CLASSES.len(), 0);

const fn room(table: usize, engine: usize) -> usize {
    let width = 32 - (table as u32).leading_zeros();
    (1usize << width) - table - engine
}

/// The longest name a plugin class may have.
pub const MAX_CLASS_NAME: usize = 64;

/// The ghost registry's numbers of the plugin classes ([`PluginClasses::ghost_code`]): from here up, past every number of
/// [`TgeClass::code`](crate::TgeClass::code).
pub const PLUGIN_CODE_BASE: u16 = 0x8000;

/// Why a set of plugin classes was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginClassError {
    /// A name that is empty, longer than [`MAX_CLASS_NAME`], or not ASCII letters, digits and `_` starting with a letter or `_`.
    BadName(String),
    /// A name of one of TGE's or the engine's own net classes.
    Taken(String),
    /// Two classes with one name.
    Duplicate(String),
    /// More classes of a kind than its table has ids for.
    TooMany {
        kind: Kind,
        count: usize,
        room: usize,
    },
    /// A ghost class whose packer was seen to read or keep a mask bit of the per-ghost block (bits 30 and 31,
    /// [`crate::block::BLOCK_MASK`]): packed from a view of nothing, it gave another record, or kept one of those bits, when
    /// only they were dirty. A best-effort check of hand-written records; `#[derive(GhostRecord)]` refuses such a mask at
    /// compile time.
    BlockMask(String),
}

impl fmt::Display for PluginClassError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PluginClassError::BadName(n) => write!(
                f,
                "\"{n}\" is not a class name (1 to {MAX_CLASS_NAME} ASCII letters, digits and '_', not starting with a digit)"
            ),
            PluginClassError::Taken(n) => write!(f, "{n} is a net class of the engine's own"),
            PluginClassError::Duplicate(n) => write!(f, "two plugin net classes are named {n}"),
            PluginClassError::BlockMask(n) => write!(
                f,
                "the records of {n} use mask bits 30 or 31, which belong to the per-ghost block"
            ),
            PluginClassError::TooMany { kind, count, room } => write!(
                f,
                "{count} plugin {} classes, and the class table has ids for {room}",
                match kind {
                    Kind::Object => "ghost",
                    _ => "datablock",
                }
            ),
        }
    }
}

impl std::error::Error for PluginClassError {}

/// Whether `name` can be a plugin class's name.
pub fn is_class_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let first_ok = bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_');
    first_ok
        && name.len() <= MAX_CLASS_NAME
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Whether `name` is one of TGE's or the engine's own net classes (of any kind).
pub fn is_engine_class(name: &str) -> bool {
    TGE_OBJECT_CLASSES.contains(&name)
        || ENGINE_OBJECT_CLASSES.contains(&name)
        || TGE_DATABLOCK_CLASSES.contains(&name)
}

/// A game's plugin classes, each kind in id order (sorted by name). See the [module docs](self).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginClasses {
    ghosts: Vec<PluginClass>,
    datablocks: Vec<PluginClass>,
}

impl PluginClasses {
    /// The set of `classes`: each kind sorted by name (unsigned bytes). Refused: a bad name, a name of TGE's or the engine's,
    /// a name given twice (across both kinds: they are console class names), more classes than a kind has room for.
    pub fn new(
        classes: impl IntoIterator<Item = PluginClass>,
    ) -> Result<PluginClasses, PluginClassError> {
        let mut set = PluginClasses::default();
        for class in classes {
            let name = class.name();
            if !is_class_name(name) {
                return Err(PluginClassError::BadName(name.to_string()));
            }
            if is_engine_class(name) {
                return Err(PluginClassError::Taken(name.to_string()));
            }
            if set.all().any(|c| c.name() == name) {
                return Err(PluginClassError::Duplicate(name.to_string()));
            }
            if class.kind() == Kind::Object && class.reads_block_bits() {
                return Err(PluginClassError::BlockMask(name.to_string()));
            }
            match class.kind() {
                Kind::Object => set.ghosts.push(class),
                _ => set.datablocks.push(class),
            }
        }
        for (kind, list, room) in [
            (Kind::Object, &mut set.ghosts, PLUGIN_GHOST_ROOM),
            (Kind::DataBlock, &mut set.datablocks, PLUGIN_DATABLOCK_ROOM),
        ] {
            if list.len() > room {
                return Err(PluginClassError::TooMany {
                    kind,
                    count: list.len(),
                    room,
                });
            }
            list.sort_by(|a, b| a.name().as_bytes().cmp(b.name().as_bytes()));
        }
        Ok(set)
    }

    /// Whether the set has no class.
    pub fn is_empty(&self) -> bool {
        self.ghosts.is_empty() && self.datablocks.is_empty()
    }

    fn all(&self) -> impl Iterator<Item = PluginClass> + '_ {
        self.ghosts.iter().chain(&self.datablocks).copied()
    }

    /// The ghost classes in id order.
    pub fn ghosts(&self) -> &[PluginClass] {
        &self.ghosts
    }

    /// The datablock classes in id order.
    pub fn datablocks(&self) -> &[PluginClass] {
        &self.datablocks
    }

    /// The classes of `kind` in id order (none for events).
    pub fn of_kind(&self, kind: Kind) -> &[PluginClass] {
        match kind {
            Kind::Object => &self.ghosts,
            Kind::DataBlock => &self.datablocks,
            Kind::Event => &[],
        }
    }

    /// The names of the classes of `kind` in id order (what both sides must agree on).
    pub fn names(&self, kind: Kind) -> impl Iterator<Item = &'static str> + '_ {
        self.of_kind(kind).iter().map(|c| c.name())
    }

    /// The place of the class `name` of `kind` among its kind, and the class.
    pub fn find(&self, kind: Kind, name: &str) -> Option<(usize, PluginClass)> {
        self.of_kind(kind)
            .iter()
            .position(|c| c.name() == name)
            .map(|k| (k, self.of_kind(kind)[k]))
    }

    /// The ghost class `name`.
    pub fn ghost(&self, name: &str) -> Option<PluginClass> {
        self.find(Kind::Object, name).map(|(_, c)| c)
    }

    /// The datablock class `name`.
    pub fn datablock(&self, name: &str) -> Option<PluginClass> {
        self.find(Kind::DataBlock, name).map(|(_, c)| c)
    }

    /// The first plugin object class id of a connection whose TGE object table has `table_len` classes: past the table and
    /// the engine's own classes.
    pub fn ghost_base(table_len: usize) -> u32 {
        u32::try_from(table_len + ENGINE_OBJECT_CLASSES.len()).unwrap_or(u32::MAX)
    }

    /// The first plugin datablock class id of a connection whose TGE datablock table has `table_len` classes.
    pub fn datablock_base(table_len: usize) -> u32 {
        u32::try_from(table_len).unwrap_or(u32::MAX)
    }

    /// The class id of the class `name` of `kind`, the kind's ids starting at `base`.
    pub fn id_of(&self, kind: Kind, name: &str, base: u32) -> Option<u32> {
        let (k, _) = self.find(kind, name)?;
        base.checked_add(u32::try_from(k).ok()?)
    }

    /// The class of `kind` a peer's class `id` names, the kind's ids starting at `base` (`None`: no plugin class has it).
    pub fn by_id(&self, kind: Kind, id: u32, base: u32) -> Option<PluginClass> {
        let k = usize::try_from(id.checked_sub(base)?).ok()?;
        self.of_kind(kind).get(k).copied()
    }

    /// The ghost registry's number of the ghost class `name` ([`PLUGIN_CODE_BASE`] plus its place).
    pub fn ghost_code(&self, name: &str) -> Option<u16> {
        let (k, _) = self.find(Kind::Object, name)?;
        PLUGIN_CODE_BASE.checked_add(u16::try_from(k).ok()?)
    }

    /// The ghost class of a ghost registry number ([`ghost_code`](Self::ghost_code)).
    pub fn ghost_of_code(&self, code: u16) -> Option<PluginClass> {
        let k = code.checked_sub(PLUGIN_CODE_BASE)?;
        self.ghosts.get(usize::from(k)).copied()
    }
}

#[cfg(test)]
mod tests;
