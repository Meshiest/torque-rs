//! The console field types, following Torque3D's MIT-licensed `console/consoleTypes.cpp` and `math/mathTypes.cpp`
//! (Copyright (c) 2012 GarageGames, LLC):
//! how a value written to a static field is stored and how it reads back. Where a game
//! reads them differently, [`FieldRules`] says so.
//!
//! A value is stored typed, one [`Value`] per element of a field (an array field has several); [`set`] parses
//! the text a script writes into the element as the type's setter does (some types keep what the text does
//! not overwrite: the points), [`get`] formats it as the type's getter does ([`get_charged`] for a reader that
//! pays for the work in step-budget units: a list is as long as the script made it). What needs the rest of the
//! runtime (the string table, `./` in file names, finding a datablock, the console) goes through [`FieldEnv`].

use std::rc::Rc;

use crate::sim::ObjId;
use crate::text::{atof, atoi, format_fixed, format_g, scan_floats, to_f32, to_f64, until_nul};
use crate::vm::Level;
pub use crate::vm::vectors::Mat4;
use crate::vm::vectors::{
    axis_angle_from_quat, d, fdiv, fl, fmul, matrix_from_quat, normalize, quat_from_axis_angle,
    quat_from_matrix,
};

/// An enum field's labels and values, in the table's order (`EnumTable`).
#[derive(Debug, PartialEq, Eq)]
pub struct EnumTable {
    pub entries: &'static [(i32, &'static str)],
}

/// A console field type, named as `dumpConsoleClasses()` prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// `string`: interned in the runtime-wide string table, which keeps the FIRST spelling of a text ignoring
    /// case (`"mixedcase"` reads `MixedCase` once that spelling was stored; `"scriptobject"` reads `ScriptObject`).
    String,
    /// `caseString`: kept as written.
    CaseString,
    /// `filename`: `./` and `~/` expanded against the running file (`expandScriptFilename`), then interned like
    /// a `string`.
    Filename,
    /// `U8`: `atoi`, the low byte kept.
    U8,
    /// `U16`: `atoi`, the low 16 bits kept.
    U16,
    /// `int`: `atoi` (saturating).
    S32,
    /// `float`: `atof` rounded to a float, read with `%g`.
    F32,
    /// `bool`: `dAtob` (`true` in any case, or a number other than 0 after `atof` has been converted to a float:
    /// `1e-50` is false), read `1`/`0`.
    Bool,
    /// `intList`: numbers separated by spaces.
    S32Vector,
    /// `floatList`.
    F32Vector,
    /// `enumval`: a label of the table, matched ignoring case; any other text stores 0.
    Enum(&'static EnumTable),
    /// `ColorI`: `"r g b a"` bytes, alpha 255 when only three are given.
    ColorI,
    /// `ColorF`: four floats, alpha 1 when only three are given; read with `%g` (see [`FieldRules`]).
    ColorF,
    /// `Point2I`: two ints, those the text does not give keep their value.
    Point2I,
    /// `Point2F`: two floats, in place like `Point2I`; read with `%g` (see [`FieldRules`]).
    Point2F,
    /// `Point3F`: three floats, in place (`"1 2 x"` over `"1.5 2.25 1000"` gives `1 2 1000`).
    Point3F,
    /// `MatrixPosition`: the translation of a matrix the field shares with a `MatrixRotation`.
    MatrixPosition,
    /// `MatrixRotation`: the rotation of that matrix, as axis and angle in degrees.
    MatrixRotation,
    /// A datablock pointer (a `<Class>Ptr` type): the class the object must be.
    DataBlock(&'static str),
    /// `GuiProfile`: a `GuiControlProfile` found by name or id.
    GuiProfile,
    /// A type this crate does not model yet: the text is kept as written.
    Opaque(&'static str),
    /// `deprecated`: writes are ignored.
    Deprecated,
}

/// One element of a field as it is stored.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(Rc<[u8]>),
    Int(i32),
    Float(f32),
    Bool(bool),
    ColorI([u8; 4]),
    ColorF([f32; 4]),
    Ints2([i32; 2]),
    Floats2([f32; 2]),
    Floats3([f32; 3]),
    /// The whole 4x4 matrix a position and a rotation field share.
    Matrix(Box<Mat4>),
    Object(Option<ObjId>),
    IntList(Rc<[i32]>),
    FloatList(Rc<[f32]>),
}

/// The identity matrix, a fresh object's transform.
pub const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// What a field type needs from the runtime around it.
pub trait FieldEnv {
    /// `StringTable->insert(text)`: the first spelling of `text` the table holds, ignoring case.
    fn intern(&mut self, text: &[u8]) -> Rc<[u8]>;
    /// `Con::expandScriptFilename`: None for a path it refuses (the setter then warns and keeps the old value).
    fn expand_filename(&mut self, text: &[u8]) -> Option<Vec<u8>>;
    /// `Sim::findObject` of `text` when the object is of `class` or a subclass of it.
    fn find_object(&mut self, text: &[u8], class: &str) -> Option<ObjId>;
    /// A console line.
    fn message(&mut self, level: Level, text: String);
    /// Charges `units` of work (one per list element a text makes) to the step budget BEFORE that work is
    /// done; false once the budget is spent, and the caller stops: a hostile text must not allocate past the
    /// limit (a run of a million spaces before a number is a million elements).
    fn charge(&mut self, units: u64) -> bool;
    /// How the types parse and read (see [`FieldRules`]).
    fn rules(&self) -> FieldRules {
        FieldRules::default()
    }
}

/// How the types parse and read back, where a game does it differently. The defaults print every float with `%g`
/// and use this crate's number parsing and rotation maths.
#[derive(Clone, Copy)]
pub struct FieldRules {
    /// `ColorF` reads with `%f` (six decimals).
    pub fixed_color_f: bool,
    /// `Point2F` reads with `%f`.
    pub fixed_point2f: bool,
    /// `floatList` reads with `%f`.
    pub fixed_float_list: bool,
    /// The integers of the integer types (`sscanf("%d %d ...")` into the slots): see [`scan_ints`].
    pub scan_ints: fn(&[u8], &mut [i32]) -> usize,
    /// The 3x3 rotation of a `MatrixRotation` write: from an axis and an angle in degrees (the axis as written).
    pub rotation_matrix: fn([f32; 4]) -> Mat4,
    /// What a `MatrixRotation` reads: the matrix's rotation as a normalised axis and an angle in degrees.
    pub rotation_of: fn(&Mat4) -> [f32; 4],
    /// The line a datablock field prints for a name that is not an object of its class (the name, the class).
    pub not_a_datablock: fn(&str, &str) -> String,
}

impl Default for FieldRules {
    fn default() -> FieldRules {
        FieldRules {
            fixed_color_f: false,
            fixed_point2f: false,
            fixed_float_list: false,
            scan_ints,
            rotation_matrix,
            rotation_of,
            not_a_datablock: |name, class| format!("'{name}' is not a datablock of class {class}"),
        }
    }
}

impl std::fmt::Debug for FieldRules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FieldRules")
            .field("fixed_color_f", &self.fixed_color_f)
            .field("fixed_point2f", &self.fixed_point2f)
            .field("fixed_float_list", &self.fixed_float_list)
            .finish_non_exhaustive()
    }
}

/// [`FieldRules::rotation_matrix`]'s default: the axis-angle rotation (degrees) as a matrix.
pub fn rotation_matrix(aa: [f32; 4]) -> Mat4 {
    matrix_from_quat(quat_from_axis_angle(
        [aa[0], aa[1], aa[2]],
        deg_to_rad(aa[3]),
    ))
}

/// [`FieldRules::rotation_of`]'s default: the matrix's rotation as a normalised axis and an angle in degrees.
pub fn rotation_of(m: &Mat4) -> [f32; 4] {
    let (axis, angle) = axis_angle_from_quat(quat_from_matrix(m));
    let [x, y, z] = normalize(axis);
    [x, y, z, rad_to_deg(angle)]
}

/// The value a field holds before anything is written to it (the constructor defaults of a class come from the
/// class table and are written over this).
pub fn zero(ty: FieldType) -> Value {
    match ty {
        FieldType::String
        | FieldType::CaseString
        | FieldType::Filename
        | FieldType::Opaque(_)
        | FieldType::Deprecated => Value::Text(Rc::from(&b""[..])),
        FieldType::U8 | FieldType::U16 | FieldType::S32 | FieldType::Enum(_) => Value::Int(0),
        FieldType::F32 => Value::Float(0.0),
        FieldType::Bool => Value::Bool(false),
        FieldType::S32Vector => Value::IntList(Rc::from(&[][..])),
        FieldType::F32Vector => Value::FloatList(Rc::from(&[][..])),
        FieldType::ColorI => Value::ColorI([0, 0, 0, 255]),
        FieldType::ColorF => Value::ColorF([0.0, 0.0, 0.0, 1.0]),
        FieldType::Point2I => Value::Ints2([0, 0]),
        FieldType::Point2F => Value::Floats2([0.0, 0.0]),
        FieldType::Point3F => Value::Floats3([0.0, 0.0, 0.0]),
        FieldType::MatrixPosition | FieldType::MatrixRotation => Value::Matrix(Box::new(IDENTITY)),
        FieldType::DataBlock(_) | FieldType::GuiProfile => Value::Object(None),
    }
}

/// `sscanf(s, "%d %d ...")` into the slots of `out`: fills the slots it converts, leaves the others, and
/// returns how many it converted. A conversion is optional white space, a sign and at least one digit; a number
/// outside the `i32` range saturates at its end.
pub fn scan_ints(s: &[u8], out: &mut [i32]) -> usize {
    let s = until_nul(s);
    let mut at = 0;
    let mut count = 0;
    for slot in out.iter_mut() {
        while s.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
            at += 1;
        }
        let negative = s.get(at) == Some(&b'-');
        if matches!(s.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        let start = at;
        let mut value: i64 = 0;
        while let Some(digit) = s.get(at).filter(|b| b.is_ascii_digit()) {
            value = value
                .saturating_mul(10)
                .saturating_add(i64::from(digit - b'0'));
            at += 1;
        }
        if at == start {
            break;
        }
        let value = if negative { -value } else { value };
        *slot = i32::try_from(value).unwrap_or(if negative { i32::MIN } else { i32::MAX });
        count += 1;
    }
    count
}

/// `dAtob`: `true` ignoring case, or a number other than 0 once `atof`'s double has been converted to a float
/// (`(F32)atof(s) != 0`: `1e-50` and `7e-46` read false, `7.1e-46` and `1e-45` true).
pub fn atob(s: &[u8]) -> bool {
    until_nul(s).eq_ignore_ascii_case(b"true") || to_f32(atof(s)) != 0.0
}

/// A float as `%g` prints it.
fn g(x: f32) -> String {
    format_g(to_f64(x))
}

/// A float as `%f` prints it.
fn f6(x: f32) -> String {
    format_fixed(to_f64(x), 6)
}

/// A float as `%f` prints it when `fixed`, else as `%g` does.
fn float_text(x: f32, fixed: bool) -> String {
    if fixed { f6(x) } else { g(x) }
}

/// `mDegToRad` on a float: in doubles, stored as a float.
fn deg_to_rad(x: f32) -> f32 {
    fl(fdiv(fmul(d(x), std::f64::consts::PI), 180.0))
}

/// `mRadToDeg` on a float.
fn rad_to_deg(x: f32) -> f32 {
    fl(fdiv(fmul(d(x), 180.0), std::f64::consts::PI))
}

/// Writes `text` into `slot` as the setter of `ty` does. The caller has checked the element index; a
/// `slot` of the wrong shape (never made by [`zero`]) is replaced by the type's zero first.
pub fn set(ty: FieldType, slot: &mut Value, text: &[u8], env: &mut dyn FieldEnv) {
    let text = until_nul(text);
    if !matches!(
        (ty, &*slot),
        (
            FieldType::String
                | FieldType::CaseString
                | FieldType::Filename
                | FieldType::Opaque(_)
                | FieldType::Deprecated,
            Value::Text(_)
        ) | (
            FieldType::U8 | FieldType::U16 | FieldType::S32 | FieldType::Enum(_),
            Value::Int(_)
        ) | (FieldType::F32, Value::Float(_))
            | (FieldType::Bool, Value::Bool(_))
            | (FieldType::S32Vector, Value::IntList(_))
            | (FieldType::F32Vector, Value::FloatList(_))
            | (FieldType::ColorI, Value::ColorI(_))
            | (FieldType::ColorF, Value::ColorF(_))
            | (FieldType::Point2I, Value::Ints2(_))
            | (FieldType::Point2F, Value::Floats2(_))
            | (FieldType::Point3F, Value::Floats3(_))
            | (
                FieldType::MatrixPosition | FieldType::MatrixRotation,
                Value::Matrix(_)
            )
            | (
                FieldType::DataBlock(_) | FieldType::GuiProfile,
                Value::Object(_)
            )
    ) {
        *slot = zero(ty);
    }
    match ty {
        FieldType::String => *slot = Value::Text(env.intern(text)),
        // `StringTable->insert(text, true)`: the field keeps its own spelling, and the table learns it when it has no
        // spelling of the text yet (a later insert that ignores case finds the oldest entry: a `caseString` written
        // "Tool" before any `tool` makes a later `string` field written "tool" read "Tool").
        FieldType::CaseString => {
            env.intern(text);
            *slot = Value::Text(Rc::from(text));
        }
        FieldType::Opaque(_) => *slot = Value::Text(Rc::from(text)),
        FieldType::Filename => match env.expand_filename(text) {
            Some(path) => *slot = Value::Text(env.intern(&path)),
            None => env.message(
                Level::Warning,
                format!(
                    "(TypeFilename) illegal filename detected: {}",
                    String::from_utf8_lossy(text)
                ),
            ),
        },
        FieldType::Deprecated => {}
        FieldType::U8 => *slot = Value::Int(i32::from(atoi(text) as u8)),
        FieldType::U16 => *slot = Value::Int(i32::from(atoi(text) as u16)),
        FieldType::S32 => *slot = Value::Int(atoi(text)),
        FieldType::F32 => *slot = Value::Float(to_f32(atof(text))),
        FieldType::Bool => *slot = Value::Bool(atob(text)),
        FieldType::Enum(table) => {
            let value = table
                .entries
                .iter()
                .find(|(_, label)| label.as_bytes().eq_ignore_ascii_case(text))
                .map_or(0, |(value, _)| *value);
            *slot = Value::Int(value);
        }
        FieldType::S32Vector => {
            let scan = env.rules().scan_ints;
            *slot = Value::IntList(list(text, env, |s| {
                let mut v = [0];
                (scan(s, &mut v) == 1).then_some(v[0])
            }))
        }
        FieldType::F32Vector => {
            *slot = Value::FloatList(list(text, env, |s| {
                let mut v = [0.0];
                (scan_floats(s, &mut v) == 1).then_some(v[0])
            }))
        }
        FieldType::ColorI => {
            // `set(0, 0, 0, 255)`, then r, g, b from the text (what it does not give reads 0), the alpha only when all
            // four are there.
            let mut parts = [0i32; 4];
            let count = (env.rules().scan_ints)(text, &mut parts);
            let alpha = if count == 4 { parts[3] as u8 } else { 255 };
            *slot = Value::ColorI([parts[0] as u8, parts[1] as u8, parts[2] as u8, alpha]);
        }
        FieldType::ColorF => {
            let mut parts = [0.0f32; 4];
            let count = scan_floats(text, &mut parts);
            let alpha = if count == 4 { parts[3] } else { 1.0 };
            *slot = Value::ColorF([parts[0], parts[1], parts[2], alpha]);
        }
        FieldType::Point2I => {
            if let Value::Ints2(p) = slot {
                (env.rules().scan_ints)(text, p);
            }
        }
        FieldType::Point2F => {
            if let Value::Floats2(p) = slot {
                scan_floats(text, p);
            }
        }
        FieldType::Point3F => {
            if let Value::Floats3(p) = slot {
                scan_floats(text, p);
            }
        }
        FieldType::MatrixPosition => {
            if let Value::Matrix(m) = slot {
                // The column is reset to (0, 0, 0, 1), then up to four numbers are read into it (
                // `"1"` gives `1 0 0`, `"1 2 3 4"` keeps the 4 and reads back four numbers).
                let mut col = [0.0, 0.0, 0.0, 1.0];
                scan_floats(text, &mut col);
                for (row, value) in m.iter_mut().zip(col) {
                    row[3] = value;
                }
            }
        }
        FieldType::MatrixRotation => {
            if let Value::Matrix(m) = slot {
                // An axis and an angle in degrees; the axis is not normalised (`"2 0 0 30"` reads
                // `1 0 0 62.3479`). Missing numbers read 0, as TGE's `AngAxisF(Point3F(0,0,0),0)` gives (the
                // identity for `"1 2 3"`).
                let mut aa = [0.0f32; 4];
                scan_floats(text, &mut aa);
                // The whole translation column is kept, its fourth element too (`position = "1 2 3 4"` then a
                // rotation write still reads `1 2 3 4`).
                let mut rotated = (env.rules().rotation_matrix)(aa);
                for (row, old) in rotated.iter_mut().zip(m.iter()) {
                    row[3] = old[3];
                }
                **m = rotated;
            }
        }
        FieldType::DataBlock(class) => {
            // The pointer is cleared first, then looked up; a name that is not an object of the class prints
            // its line and leaves it cleared.
            let found = if text.is_empty() {
                None
            } else {
                let found = env.find_object(text, class);
                if found.is_none() {
                    env.message(
                        Level::Normal,
                        (env.rules().not_a_datablock)(&String::from_utf8_lossy(text), class),
                    );
                }
                found
            };
            *slot = Value::Object(found);
        }
        FieldType::GuiProfile => {
            // TGE 1.4: an unknown profile leaves the old one.
            if let Some(found) = env.find_object(text, "GuiControlProfile") {
                *slot = Value::Object(Some(found));
            }
        }
    }
}

/// C `isspace`.
fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// The list types' parse: a number at the start of each space-separated piece, stopping at the first piece
/// that does not start with one (`dSscanf` then `dStrchr(' ')`, TGE 1.4). Each piece is the rest of the text
/// after the previous space, and `sscanf` skips its leading white space, so a run of `k` spaces before a
/// number gives that number `k + 1` times (`" 7 8"` reads `7 7 8`). TGE walks that run piece by piece,
/// reading the rest of the text each time (quadratic in a hostile text); here the number is read once, from
/// its own token, and repeated, which gives the same list in time linear in the text.
///
/// Every element is charged to the step budget (`FieldEnv::charge`) before it is stored; when the budget is
/// spent the list ends there (the interpreter fails the run at its next instruction).
fn list<T: Copy>(text: &[u8], env: &mut dyn FieldEnv, one: impl Fn(&[u8]) -> Option<T>) -> Rc<[T]> {
    let mut out = Vec::new();
    let mut rest = text;
    loop {
        let lead = rest
            .iter()
            .position(|&b| !is_space(b))
            .unwrap_or(rest.len());
        let spaces = rest
            .get(..lead)
            .unwrap_or_default()
            .iter()
            .filter(|&&b| b == b' ')
            .count();
        let tail = rest.get(lead..).unwrap_or_default();
        let token = tail.iter().position(|&b| is_space(b)).unwrap_or(tail.len());
        let Some(value) = one(tail.get(..token).unwrap_or_default()) else {
            break;
        };
        let repeat = spaces.saturating_add(1);
        if !env.charge(repeat as u64) {
            break;
        }
        out.extend(std::iter::repeat_n(value, repeat));
        match tail.iter().position(|&b| b == b' ') {
            Some(space) => rest = tail.get(space + 1..).unwrap_or_default(),
            None => break,
        }
    }
    Rc::from(out)
}

/// Reads `slot` as the getter of `ty` does; `names` gives a live object's name (None for an unnamed
/// or deleted one: a datablock field then reads ""). Not charged: for a host, and for values whose
/// size is small by construction. A read the step budget must pay for goes through [`get_charged`].
pub fn get(ty: FieldType, slot: &Value, names: &dyn Fn(ObjId) -> Option<Vec<u8>>) -> Vec<u8> {
    get_with(FieldRules::default(), ty, slot, names)
}

/// [`get`] under `rules`.
pub fn get_with(
    rules: FieldRules,
    ty: FieldType,
    slot: &Value,
    names: &dyn Fn(ObjId) -> Option<Vec<u8>>,
) -> Vec<u8> {
    get_charged(rules, ty, slot, names, &mut |_| true)
}

/// Formatting a list is charged to the budget in chunks of at least this many bytes ([`get_charged`]).
const CHARGE_CHUNK: usize = 4096;

/// [`get`] for a reader that pays for the work: one work unit per byte it produces. A text (a `string` field, an
/// object's name) is charged before it is copied, so a read the budget cannot pay for copies nothing and gives
/// ""; a list is charged as it is formatted, in chunks of `CHARGE_CHUNK` bytes, and stops where the budget
/// ran out (the partial text is of no use: `charge` returned false, the caller's run is over). `charge`
/// returns false once the budget is spent. The small types (numbers, colours, points, matrices) are a few
/// bytes and are not charged.
pub fn get_charged(
    rules: FieldRules,
    ty: FieldType,
    slot: &Value,
    names: &dyn Fn(ObjId) -> Option<Vec<u8>>,
    charge: &mut dyn FnMut(u64) -> bool,
) -> Vec<u8> {
    let text = match (ty, slot) {
        (_, Value::Text(t)) => {
            return if charge(t.len() as u64) {
                t.to_vec()
            } else {
                Vec::new()
            };
        }
        (FieldType::Enum(table), Value::Int(v)) => table
            .entries
            .iter()
            .find(|(value, _)| value == v)
            .map_or(String::new(), |(_, label)| (*label).to_string()),
        (_, Value::Int(v)) => v.to_string(),
        (_, Value::Float(x)) => g(*x),
        (_, Value::Bool(b)) => if *b { "1" } else { "0" }.to_string(),
        (_, Value::ColorI(c)) => format!("{} {} {} {}", c[0], c[1], c[2], c[3]),
        (_, Value::ColorF(c)) => c.map(|x| float_text(x, rules.fixed_color_f)).join(" "),
        (_, Value::Ints2(p)) => format!("{} {}", p[0], p[1]),
        (_, Value::Floats2(p)) => p.map(|x| float_text(x, rules.fixed_point2f)).join(" "),
        (_, Value::Floats3(p)) => p.map(g).join(" "),
        (FieldType::MatrixRotation, Value::Matrix(m)) => (rules.rotation_of)(m).map(g).join(" "),
        (_, Value::Matrix(m)) => {
            let col = [m[0][3], m[1][3], m[2][3], m[3][3]];
            if col[3] == 1.0 {
                col[..3].iter().map(|x| g(*x)).collect::<Vec<_>>().join(" ")
            } else {
                col.map(g).join(" ")
            }
        }
        (_, Value::Object(id)) => {
            let name = id.and_then(names).unwrap_or_default();
            return if charge(name.len() as u64) {
                name
            } else {
                Vec::new()
            };
        }
        (_, Value::IntList(v)) => join_capped(v.iter().map(i32::to_string), charge),
        (_, Value::FloatList(v)) => join_capped(
            v.iter().map(|x| float_text(*x, rules.fixed_float_list)),
            charge,
        ),
    };
    text.into_bytes()
}

/// The numbers joined by spaces, charged by the bytes formatted ([`CHARGE_CHUNK`] at a time, the rest when the
/// list ends), and cut once the text passes what the string stack holds (the caller's delivery then fails the
/// run with `StringTooLong`, as it would for the whole text): a hostile list is never turned into several
/// hundred megabytes of text first, nor formatted past the budget.
fn join_capped(
    numbers: impl Iterator<Item = String>,
    charge: &mut dyn FnMut(u64) -> bool,
) -> String {
    let mut out = String::new();
    let mut unpaid = 0usize;
    for (i, number) in numbers.enumerate() {
        let before = out.len();
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&number);
        unpaid = unpaid.saturating_add(out.len() - before);
        if out.len() > crate::vm::MAX_STRING_BYTES {
            break;
        }
        if unpaid >= CHARGE_CHUNK {
            if !charge(unpaid as u64) {
                return out;
            }
            unpaid = 0;
        }
    }
    if unpaid > 0 {
        charge(unpaid as u64);
    }
    out
}

#[cfg(test)]
#[path = "fields_tests.rs"]
mod tests;
