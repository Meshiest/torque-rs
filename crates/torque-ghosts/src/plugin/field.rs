//! The field types a plugin record is made of ([`NetField`]): how each travels, how a packer reads it from the host's view, how
//! an update merges into the kept record, and what script field values it gives a client's datablock.
//!
//! | type | on the wire | `bits` | `max_len` |
//! |---|---|---|---|
//! | `bool` | one flag | - | - |
//! | `u8`, `u16`, `u32` | `int(bits)`, the type's width when left out | 1 to the type's width | - |
//! | `i32` | `int(bits)` of the two's complement, sign-extended back | 1 to 32 | - |
//! | `f32` | the raw 32 bits | - | - |
//! | `[f32; N]` | `N` raw floats | - | - |
//! | `[u8; N]` | `N` bytes | - | - |
//! | `String` | `string(255)` through the packet's string context, cut to `max_len` bytes | - | 1 to 255 (255) |
//! | [`DatablockRef`] | `flag(non-null)`, then the id `ranged` over the connection's datablock-id range | - | - |
//! | [`Xf`] | a transform, 193 bits | - | - |
//! | `Option<T>` | `flag(present)`, then `T` | `T`'s | `T`'s |
//! | `Vec<T>` | a count of `bitlen(max_len)` bits, then each element as `T` (`bits` applies to the elements) | `T`'s | 1 to [`MAX_VEC_LEN`] ([`DEFAULT_VEC_LEN`] when left out) |
//!
//! **Hostile input.** A decode never panics and allocates nothing a wire count asks for without checking it: a `Vec`'s count
//! goes through `torque_net::cap` against its `max_len` and the bits left, a string longer than its `max_len` is refused.

use std::fmt::Debug;

use torque_net::NetError;
use torque_net::bits::bit_length;
use torque_net::cap::{NET_STRING_BYTES, cut_str as cut};

use crate::codec::{Codec, F32, R, Xf, datablock_id, opt};
use crate::{ObjectView, Value, boolean, xform};

/// The most elements a `Vec` field may be given (`max_len`).
pub const MAX_VEC_LEN: usize = 1024;

/// A `Vec` field's cap when `max_len` is left out.
pub const DEFAULT_VEC_LEN: usize = 16;

/// What a field's `#[net(...)]` says about it: its width and its cap (0: the type's own).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FieldSpec {
    /// `bits = N`: the width of an integer (0: the type's width).
    pub bits: u8,
    /// `max_len = N`: the most bytes of a string or elements of a `Vec` (0: the type's cap).
    pub max_len: usize,
}

impl FieldSpec {
    /// A spec that takes the type's own width and cap.
    pub const DEFAULT: FieldSpec = FieldSpec {
        bits: 0,
        max_len: 0,
    };

    /// The width, `full` when none was given.
    pub fn bits_or(self, full: u8) -> u8 {
        if self.bits == 0 { full } else { self.bits }
    }

    /// The cap, `cap` when none was given.
    pub fn len_or(self, cap: usize) -> usize {
        if self.max_len == 0 { cap } else { self.max_len }
    }
}

/// A field type of a plugin record (`#[derive(GhostRecord)]`, `#[derive(DatablockCodec)]`). See the [module docs](self).
pub trait NetField: Clone + Default + Debug + PartialEq + Send + Sync + 'static {
    /// The widest `bits` the type takes (0: it takes no `bits`).
    const MAX_BITS: u8 = 0;
    /// The longest `max_len` the type takes (0: it takes no `max_len`).
    const MAX_LEN: usize = 0;
    /// Whether the type is an `Option` (only such a field may have a dirty `mask`: it is not sent when the mask is clear).
    const OPTIONAL: bool = false;
    /// How many elements of the view one value takes (a `[f32; 3]` takes 3: its components).
    const ELEMENTS: usize = 1;
    /// Whether the type reads console fields of its own, named after the field (`start` reads `startMode` and `startPoint`)
    /// and not the field `start` itself: an `Option` of it is present whenever it is packed, as the field it is named for need not
    /// exist.
    const COMPOUND: bool = false;
    /// The fewest bits one value takes on the wire (what a `Vec`'s count is checked against).
    const MIN_BITS: usize = 1;
    /// Whether the type holds floats (a message may take them only with a finite check: `#[net(finite)]`).
    const HAS_FLOATS: bool = false;
    /// Whether the type must be given a `max_len` in a message (a `String` or a `Vec`: a message has no default cap).
    const NEEDS_CAP: bool = false;

    /// Writes or reads the field: one call per value, in wire order (`name` for the logging encoder).
    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, spec: FieldSpec) -> R;

    /// The value from the host's view: field `name`, element `index` (a value that is missing or of another kind reads as 0,
    /// false, "" or none, so a view cannot make a packer fail).
    fn from_view(view: &dyn ObjectView, name: &str, index: usize, spec: FieldSpec) -> Self;

    /// An update merged into the kept value: replaced, except an `Option` that was not sent keeps the kept one.
    fn merge_from(&mut self, sent: &Self) {
        self.clone_from(sent);
    }

    /// The script field values of the value, one per element (none for an `Option` that was not sent).
    fn values(&self, out: &mut Vec<Value>);

    /// Whether the value waits for something the packer could not give yet (a link to an object the connection has not ghosted):
    /// a `mask` field that is pending keeps its mask bits, so the next update sends it again.
    fn pending(&self) -> bool {
        false
    }

    /// The datablock id a [`DatablockRef`] names (the record's datablock is its first such field).
    fn datablock(&self) -> Option<u32> {
        None
    }

    /// The transform an [`Xf`] holds (the record's pose is its first such field).
    fn transform(&self) -> Option<Xf> {
        None
    }

    /// Appends the ghost indexes the value holds ([`GhostRef`]s), for a server to check against what it sent a connection.
    fn ghost_refs(&self, out: &mut Vec<u32>) {
        let _ = out;
    }

    /// Replaces each ghost index the value holds ([`GhostRef`]s) with what `map` makes of it: an index to the object it stands
    /// for, or an object to its index. False when `map` has no answer for one (the value is then not to be used).
    fn map_ghosts(&mut self, map: &mut dyn FnMut(u32) -> Option<u32>) -> bool {
        let _ = map;
        true
    }

    /// Whether every float in the value is finite.
    fn finite(&self) -> bool {
        true
    }
}

/// A view's value as a number: an integer, a float, a truth value, or text that reads as a number.
fn number(view: &dyn ObjectView, name: &str, index: usize) -> f64 {
    match view.get(name, index) {
        Value::Int(n) => n as f64,
        Value::F32(x) => f64::from(x),
        Value::Bool(b) => f64::from(u8::from(b)),
        Value::Id(Some(id)) => f64::from(id),
        Value::Str(s) => std::str::from_utf8(&s)
            .ok()
            .and_then(|t| t.trim().parse::<f64>().ok())
            .unwrap_or(0.0),
        _ => 0.0,
    }
}

/// A view's value as an integer: a float is cut toward zero, as a C cast does (NaN and out-of-range values saturate).
fn integer(view: &dyn ObjectView, name: &str, index: usize) -> i64 {
    match view.get(name, index) {
        Value::Int(n) => n,
        _ => number(view, name, index) as i64,
    }
}

fn present(view: &dyn ObjectView, name: &str, index: usize) -> bool {
    !matches!(view.get(name, index), Value::Null)
}

/// The low `bits` bits of `v` (32 or more: all of them).
fn low_bits(v: u32, bits: u8) -> u32 {
    if bits >= 32 {
        v
    } else {
        v & ((1u32 << bits) - 1)
    }
}

impl NetField for bool {
    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        c.flag(name, self)
    }

    fn from_view(view: &dyn ObjectView, name: &str, index: usize, _spec: FieldSpec) -> bool {
        match view.get(name, index) {
            Value::Str(s) => std::str::from_utf8(&s)
                .ok()
                .and_then(|t| t.trim().parse::<f64>().ok())
                .is_some_and(|x| x != 0.0),
            _ => boolean(view, name, index),
        }
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::Bool(*self));
    }
}

macro_rules! unsigned_field {
    ($($ty:ty => $bits:literal),+) => {$(
        impl NetField for $ty {
            const MAX_BITS: u8 = $bits;

            fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, spec: FieldSpec) -> R {
                let bits = spec.bits_or($bits);
                let mut v = low_bits(u32::from(*self), bits);
                c.int(name, &mut v, bits)?;
                // A decoded value is at most `bits` wide, and `bits` is at most the type's width.
                *self = <$ty>::try_from(low_bits(v, bits)).unwrap_or(<$ty>::MAX);
                Ok(())
            }

            fn from_view(view: &dyn ObjectView, name: &str, index: usize, spec: FieldSpec) -> $ty {
                let v = integer(view, name, index) as u32;
                <$ty>::try_from(low_bits(v, spec.bits_or($bits))).unwrap_or(<$ty>::MAX)
            }

            fn values(&self, out: &mut Vec<Value>) {
                out.push(Value::Int(i64::from(*self)));
            }
        }
    )+};
}

unsigned_field!(u8 => 8, u16 => 16, u32 => 32);

/// `v`'s low `bits` bits as a two's complement number of that width.
fn sign_extend(v: u32, bits: u8) -> i32 {
    if bits == 0 || bits >= 32 {
        return v as i32;
    }
    let shift = 32 - u32::from(bits);
    ((v << shift) as i32) >> shift
}

impl NetField for i32 {
    const MAX_BITS: u8 = 32;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, spec: FieldSpec) -> R {
        let bits = spec.bits_or(32);
        let mut v = low_bits(*self as u32, bits);
        c.int(name, &mut v, bits)?;
        *self = sign_extend(v, bits);
        Ok(())
    }

    fn from_view(view: &dyn ObjectView, name: &str, index: usize, spec: FieldSpec) -> i32 {
        let bits = spec.bits_or(32);
        sign_extend(low_bits(integer(view, name, index) as u32, bits), bits)
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::Int(i64::from(*self)));
    }
}

impl NetField for f32 {
    const MIN_BITS: usize = 32;
    const HAS_FLOATS: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let mut bits = self.to_bits();
        c.float32(name, &mut bits)?;
        *self = f32::from_bits(bits);
        Ok(())
    }

    fn from_view(view: &dyn ObjectView, name: &str, index: usize, _spec: FieldSpec) -> f32 {
        number(view, name, index) as f32
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::F32(*self));
    }

    fn finite(&self) -> bool {
        self.is_finite()
    }
}

impl<const N: usize> NetField for [f32; N]
where
    [f32; N]: Default,
{
    const ELEMENTS: usize = N;
    const MIN_BITS: usize = 32 * N;
    const HAS_FLOATS: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let mut raw = self.map(F32::new);
        c.floats(name, &mut raw)?;
        *self = raw.map(F32::get);
        Ok(())
    }

    fn from_view(view: &dyn ObjectView, name: &str, index: usize, _spec: FieldSpec) -> [f32; N] {
        // A vector the view holds as one text ("1 2 3") reads by its words; else each component is an element.
        if let Value::Str(s) = view.get(name, index) {
            let mut out = [0.0; N];
            let text = String::from_utf8_lossy(&s);
            for (slot, word) in out.iter_mut().zip(text.split_whitespace()) {
                *slot = word.parse().unwrap_or(0.0);
            }
            return out;
        }
        std::array::from_fn(|k| number(view, name, index + k) as f32)
    }

    /// One value: the components as the console prints a vector, "x y z".
    fn values(&self, out: &mut Vec<Value>) {
        let words: Vec<String> = self.iter().map(|x| x.to_string()).collect();
        out.push(Value::Str(words.join(" ").into_bytes()));
    }

    fn finite(&self) -> bool {
        self.iter().all(|x| x.is_finite())
    }
}

impl<const N: usize> NetField for [u8; N]
where
    [u8; N]: Default,
{
    const ELEMENTS: usize = N;
    const MIN_BITS: usize = 8 * N;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        for byte in self.iter_mut() {
            let mut v = u32::from(*byte);
            c.int(name, &mut v, 8)?;
            *byte = low_bits(v, 8) as u8;
        }
        Ok(())
    }

    fn from_view(view: &dyn ObjectView, name: &str, index: usize, _spec: FieldSpec) -> [u8; N] {
        // A color the view holds as one text ("1 2 3 4") reads by its words; else each component is an element.
        if let Value::Str(s) = view.get(name, index) {
            let mut out = [0u8; N];
            let text = String::from_utf8_lossy(&s);
            for (slot, word) in out.iter_mut().zip(text.split_whitespace()) {
                *slot = word.parse::<f64>().map_or(0, |x| x.clamp(0.0, 255.0) as u8);
            }
            return out;
        }
        std::array::from_fn(|k| integer(view, name, index + k).clamp(0, 255) as u8)
    }

    /// One value: the components as the console prints a color, "r g b a".
    fn values(&self, out: &mut Vec<Value>) {
        let words: Vec<String> = self.iter().map(|x| x.to_string()).collect();
        out.push(Value::Str(words.join(" ").into_bytes()));
    }
}

impl NetField for String {
    const MAX_LEN: usize = NET_STRING_BYTES;
    const NEEDS_CAP: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, spec: FieldSpec) -> R {
        let max = spec.len_or(NET_STRING_BYTES).min(NET_STRING_BYTES);
        let mut bytes = cut(self, max).as_bytes().to_vec();
        c.string(name, &mut bytes)?;
        if bytes.len() > max {
            return Err(NetError::Invalid("string longer than its field's max_len"));
        }
        *self = String::from_utf8_lossy(&bytes).into_owned();
        Ok(())
    }

    fn from_view(view: &dyn ObjectView, name: &str, index: usize, spec: FieldSpec) -> String {
        let text = crate::text(view, name, index);
        let max = spec.len_or(NET_STRING_BYTES).min(NET_STRING_BYTES);
        cut(&String::from_utf8_lossy(&text), max).to_string()
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::Str(self.as_bytes().to_vec()));
    }
}

/// A datablock pointer of a plugin record: the datablock's id (`None`: null). It travels as a TGE record's does,
/// `flag(non-null)` and the id over the connection's datablock-id range; the client's world takes a ghost's first one as the
/// ghost's datablock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct DatablockRef(pub Option<u32>);

impl NetField for DatablockRef {
    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        opt(c, name, &mut self.0, |c, id| datablock_id(c, name, id))
    }

    fn from_view(
        view: &dyn ObjectView,
        name: &str,
        index: usize,
        _spec: FieldSpec,
    ) -> DatablockRef {
        DatablockRef(crate::id(view, name, index))
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::Id(self.0));
    }

    fn datablock(&self) -> Option<u32> {
        self.0
    }
}

impl NetField for Xf {
    const MIN_BITS: usize = 193;
    const HAS_FLOATS: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        c.xform(name, self)
    }

    fn from_view(view: &dyn ObjectView, name: &str, _index: usize, _spec: FieldSpec) -> Xf {
        xform(view, name)
    }

    fn values(&self, out: &mut Vec<Value>) {
        let mut words = [0u32; 7];
        for (slot, f) in words.iter_mut().zip(self.position.iter().chain(&self.quat)) {
            *slot = f.0;
        }
        words[6] = u32::from(self.negative_w);
        out.push(Value::Xform(Some(words)));
    }

    fn transform(&self) -> Option<Xf> {
        Some(*self)
    }

    fn finite(&self) -> bool {
        self.position
            .iter()
            .chain(&self.quat)
            .all(|f| f.get().is_finite())
    }
}

impl<T: NetField> NetField for Option<T> {
    const MAX_BITS: u8 = T::MAX_BITS;
    const MAX_LEN: usize = T::MAX_LEN;
    const OPTIONAL: bool = true;
    const ELEMENTS: usize = T::ELEMENTS;
    const COMPOUND: bool = T::COMPOUND;
    const HAS_FLOATS: bool = T::HAS_FLOATS;
    const NEEDS_CAP: bool = T::NEEDS_CAP;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, spec: FieldSpec) -> R {
        opt(c, name, self, |c, inner| inner.layout(c, name, spec))
    }

    /// The value when the view has one (a null field is none).
    fn from_view(view: &dyn ObjectView, name: &str, index: usize, spec: FieldSpec) -> Option<T> {
        (T::COMPOUND || present(view, name, index)).then(|| T::from_view(view, name, index, spec))
    }

    fn pending(&self) -> bool {
        self.as_ref().is_some_and(NetField::pending)
    }

    fn merge_from(&mut self, sent: &Option<T>) {
        if let Some(sent) = sent {
            match self {
                Some(kept) => kept.merge_from(sent),
                None => *self = Some(sent.clone()),
            }
        }
    }

    fn values(&self, out: &mut Vec<Value>) {
        if let Some(v) = self {
            v.values(out);
        }
    }

    fn datablock(&self) -> Option<u32> {
        self.as_ref().and_then(NetField::datablock)
    }

    fn transform(&self) -> Option<Xf> {
        self.as_ref().and_then(NetField::transform)
    }

    fn ghost_refs(&self, out: &mut Vec<u32>) {
        if let Some(v) = self {
            v.ghost_refs(out);
        }
    }

    fn map_ghosts(&mut self, map: &mut dyn FnMut(u32) -> Option<u32>) -> bool {
        self.as_mut().is_none_or(|v| v.map_ghosts(map))
    }

    fn finite(&self) -> bool {
        self.as_ref().is_none_or(NetField::finite)
    }
}

impl<T: NetField> NetField for Vec<T> {
    const MAX_BITS: u8 = T::MAX_BITS;
    const MAX_LEN: usize = MAX_VEC_LEN;
    const HAS_FLOATS: bool = T::HAS_FLOATS;
    const NEEDS_CAP: bool = true;

    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, spec: FieldSpec) -> R {
        let max = spec.len_or(DEFAULT_VEC_LEN).min(MAX_VEC_LEN);
        let each = FieldSpec {
            bits: spec.bits,
            max_len: 0,
        };
        c.seq(
            name,
            self,
            bit_length(max as u32),
            max,
            T::MIN_BITS,
            |c, item| item.layout(c, name, each),
        )
    }

    /// The elements from element `index` on, up to the first the view does not have or the cap.
    fn from_view(view: &dyn ObjectView, name: &str, index: usize, spec: FieldSpec) -> Vec<T> {
        let max = spec.len_or(DEFAULT_VEC_LEN).min(MAX_VEC_LEN);
        let each = FieldSpec {
            bits: spec.bits,
            max_len: 0,
        };
        (0..max)
            .map(|k| index + k * T::ELEMENTS)
            .take_while(|&at| present(view, name, at))
            .map(|at| T::from_view(view, name, at, each))
            .collect()
    }

    fn pending(&self) -> bool {
        self.iter().any(NetField::pending)
    }

    fn ghost_refs(&self, out: &mut Vec<u32>) {
        for v in self {
            v.ghost_refs(out);
        }
    }

    fn map_ghosts(&mut self, map: &mut dyn FnMut(u32) -> Option<u32>) -> bool {
        let mut all = true;
        for v in self.iter_mut() {
            all &= v.map_ghosts(map);
        }
        all
    }

    fn finite(&self) -> bool {
        self.iter().all(NetField::finite)
    }

    fn values(&self, out: &mut Vec<Value>) {
        for v in self {
            v.values(out);
        }
    }
}

/// A reference to a ghost, for a message that names an object the connection can see (an RPC's argument): the ghost's index in
/// the connection, `ghost bits` wide. What an index means belongs to one connection, so a server resolves it only against the
/// ghosts it has sent that connection ([`NetField::ghost_refs`] lists them for the check), and a client can name only what it sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct GhostRef(pub u32);

impl NetField for GhostRef {
    fn layout<C: Codec>(&mut self, c: &mut C, name: &'static str, _spec: FieldSpec) -> R {
        let bits = c.ghost_bits();
        let mut v = low_bits(self.0, bits);
        c.int(name, &mut v, bits)?;
        self.0 = low_bits(v, bits);
        Ok(())
    }

    fn from_view(view: &dyn ObjectView, name: &str, index: usize, _spec: FieldSpec) -> GhostRef {
        GhostRef(u32::try_from(integer(view, name, index).max(0)).unwrap_or(0))
    }

    fn values(&self, out: &mut Vec<Value>) {
        out.push(Value::Int(i64::from(self.0)));
    }

    fn ghost_refs(&self, out: &mut Vec<u32>) {
        out.push(self.0);
    }

    fn map_ghosts(&mut self, map: &mut dyn FnMut(u32) -> Option<u32>) -> bool {
        match map(self.0) {
            Some(to) => {
                self.0 = to;
                true
            }
            None => false,
        }
    }
}
