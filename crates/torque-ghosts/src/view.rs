//! What a packer reads: the host's [`ObjectView`] of one object as one connection sees it, the [`PackCtx`], and the coercions
//! of a view's values (a missing value is 0, false, "" or null, so a view cannot make a packer fail).

use torque_net::bits::BitWriter;

use crate::Value;
use crate::codec::{F32, NetStr, NetWidths, Xf};

/// What `getGhostIndex` says about a pointer to another object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// A null pointer.
    Null,
    /// An object with no ghost index yet (not ghosted yet, still being ghosted, or being killed): `getGhostIndex`'s `-1`.
    NotGhosted,
    /// The ghost index.
    Ghost(u32),
}

/// The host's getters for one object, as one connection sees it. Any kind of value may come back for any field: the packers
/// coerce (a missing value is 0, false, "" or null), so a view cannot make a packer fail.
pub trait ObjectView {
    /// A field by name and element index (0 for a scalar).
    fn get(&self, field: &str, index: usize) -> Value;
    /// A pointer field resolved against the connection's ghosts.
    fn link(&self, field: &str) -> Link {
        let _ = field;
        Link::Null
    }
    /// A net string field (`index` for the element of an array field). The host maps text to a slot of the connection's string
    /// table as `NetConnection::packStringHandleU` does (and touches its LRU).
    fn net_string(&self, field: &str, index: usize) -> NetStr {
        let _ = (field, index);
        NetStr::Null
    }
    /// The number of mesh objects of the shape instance (`TSShapeInstance`'s), or `None` when the object has no instance.
    fn mesh_objects(&self) -> Option<usize> {
        None
    }
}

/// What a pack needs besides the view.
#[derive(Debug, Clone, Copy)]
pub struct PackCtx {
    /// The connection's id widths.
    pub widths: NetWidths,
    /// The simulation clock (ms) the time-keeping packers use (a timer measured against the Sim event clock).
    pub now_ms: u32,
    /// The stream's compression point, `[0; 3]` at the start of every packet and set by the control section.
    pub compression_point: [f32; 3],
}

impl PackCtx {
    /// The context for a record written into `w`: the stream's compression point is the writer's.
    pub fn for_writer(w: &BitWriter, widths: NetWidths, now_ms: u32) -> PackCtx {
        PackCtx {
            widths,
            now_ms,
            compression_point: w.compression_point(),
        }
    }

    /// The connection's ghost-index width.
    pub fn ghost_bits(&self) -> u8 {
        self.widths.ghost_bits
    }

    /// The wire id of a datablock pointer ([`NetWidths::wire_datablock`]).
    pub fn wire_db(&self, id: Option<u32>) -> u32 {
        self.widths.wire_datablock(id)
    }
}

impl PackCtx {
    /// A context for the connection's `widths` at time 0, the stream's compression point at zero.
    pub fn new(widths: NetWidths) -> PackCtx {
        PackCtx {
            widths,
            now_ms: 0,
            compression_point: [0.0; 3],
        }
    }
}

impl Default for PackCtx {
    /// torque-net's default widths ([`NetWidths::engine_default`]).
    fn default() -> PackCtx {
        PackCtx {
            widths: NetWidths::engine_default(),
            now_ms: 0,
            compression_point: [0.0; 3],
        }
    }
}

// Coercions of a view's values (the packers' vocabulary)

/// A float field.
pub fn fld(v: &dyn ObjectView, name: &str, i: usize) -> f32 {
    match v.get(name, i) {
        Value::F32(x) => x,
        Value::Int(n) => n as f32,
        Value::Bool(b) => f32::from(u8::from(b)),
        _ => 0.0,
    }
}

/// A float field as it travels.
pub fn raw(v: &dyn ObjectView, name: &str, i: usize) -> F32 {
    F32::new(fld(v, name, i))
}

/// `N` raw floats of a field, element `base + k`.
pub fn raws<const N: usize>(v: &dyn ObjectView, name: &str, base: usize) -> [F32; N] {
    let mut out = [F32::default(); N];
    for (k, slot) in out.iter_mut().enumerate() {
        *slot = raw(v, name, base + k);
    }
    out
}

/// An integer field (a float is converted as the `(S32)` cast does: `torque_net::library`'s `to_int`).
pub fn int(v: &dyn ObjectView, name: &str, i: usize) -> i64 {
    match v.get(name, i) {
        Value::Int(n) => n,
        Value::Bool(b) => i64::from(b),
        Value::F32(x) => i64::from((torque_net::library::get().to_int)(x)),
        Value::Id(Some(id)) => i64::from(id),
        _ => 0,
    }
}

/// An integer field's low 32 bits (what `write(4)` of an `S32` sends).
pub fn word(v: &dyn ObjectView, name: &str, i: usize) -> u32 {
    int(v, name, i) as u32
}

/// A boolean field (a float is true when it is not zero; a NaN is true).
pub fn boolean(v: &dyn ObjectView, name: &str, i: usize) -> bool {
    match v.get(name, i) {
        Value::Bool(b) => b,
        Value::Int(n) => n != 0,
        Value::F32(x) => x != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::Id(x) => x.is_some(),
        _ => false,
    }
}

/// A datablock pointer resolved to its object's id; `None` is null.
pub fn id(v: &dyn ObjectView, name: &str, i: usize) -> Option<u32> {
    match v.get(name, i) {
        Value::Id(x) => x,
        Value::Int(n) if n > 0 => u32::try_from(n).ok(),
        _ => None,
    }
}

/// A string field, cut at its first NUL and at 255 bytes.
pub fn text(v: &dyn ObjectView, name: &str, i: usize) -> Vec<u8> {
    match v.get(name, i) {
        Value::Str(s) => {
            let s = s.get(..s.len().min(255)).unwrap_or(&[]);
            s.split(|&b| b == 0).next().unwrap_or(&[]).to_vec()
        }
        _ => Vec::new(),
    }
}

/// A transform field.
pub fn xform(v: &dyn ObjectView, name: &str) -> Xf {
    match v.get(name, 0) {
        Value::Xform(Some(w)) => Xf {
            position: [F32(w[0]), F32(w[1]), F32(w[2])],
            quat: [F32(w[3]), F32(w[4]), F32(w[5])],
            negative_w: w[6] != 0,
        },
        _ => Xf::default(),
    }
}
