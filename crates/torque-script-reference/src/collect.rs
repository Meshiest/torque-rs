//! Reads the reference's entries from the script engines an [`Engine`] makes.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use torque_engine::rpc::{Allow, Delivery, RpcDirection as Direction, RpcEntry};
use torque_engine::{Engine, PrefKind};
use torque_ts_vm::classes::Validator;
use torque_ts_vm::fields::{self, FieldType};
use torque_ts_vm::text::format_g;
use torque_ts_vm::vm::Vm;

use crate::model::{
    Arity, Class, Field, MessageField, Native, Pref, Reference, Rpc, RpcDirection, Side, Sides,
};

/// One script engine and the side it belongs to.
pub struct SideVm<'a> {
    pub side: Side,
    pub vm: &'a Vm,
}

/// What an engine's registry declared for a native, by lower-cased namespace and name: whether it is a method, whether it is
/// a development command, and the doc comment after its first line.
type Declared = BTreeMap<(String, String), (bool, bool, String)>;

/// The entries of the script engines `vms` (the server's and the client's of `engine`): the natives with the docs of the
/// declarations the engine kept, the console classes with their fields, and the prefs.
pub fn collect(engine: &Engine, vms: &[SideVm]) -> Reference {
    let mut reference = Reference::default();
    let class_names: BTreeSet<String> = vms
        .iter()
        .flat_map(|v| class_names(v.vm))
        .map(|n| lower(&n))
        .collect();
    let declared: Declared = engine
        .natives()
        .map(|n| {
            (
                (lower(n.namespace), lower(n.name)),
                (n.method, n.dev, after_first_line(n.docs)),
            )
        })
        .collect();
    for v in vms {
        for line in v.vm.native_listing().lines() {
            if let Some(native) = parse_listing_line(line, &class_names, &declared, v.side) {
                merge_native(&mut reference.natives, native);
            }
        }
        for class in classes(v.vm, v.side) {
            merge_class(&mut reference.classes, class);
        }
    }
    let declared: BTreeSet<String> = engine.classes().map(|c| lower(c.name)).collect();
    for class in &mut reference.classes {
        class.declared = declared.contains(&lower(&class.name));
    }
    for decl in engine.prefs() {
        let mut sides = Sides::default();
        for v in vms {
            if v.vm.global(decl.name).is_some() {
                sides.0.insert(v.side);
            }
        }
        reference.prefs.push(Pref {
            name: decl.name.to_string(),
            ty: match decl.kind {
                PrefKind::Bool => "bool",
                PrefKind::Int => "int",
                PrefKind::Float => "float",
                PrefKind::Text => "string",
            }
            .to_string(),
            default: decl.default.to_string(),
            range: decl.range_text(),
            doc: decl.doc.trim().to_string(),
            sides,
            plugins: Vec::new(),
        });
    }
    reference
}

/// The RPCs `engine` holds, in name order, with the shape of each one's messages.
///
/// # Errors
/// The engine's RPC table is invalid (more RPCs than the wire has ids for), or a message's layout text has an entry that does
/// not read (which would shift the documented argument positions).
pub fn rpcs(engine: &Engine) -> Result<Vec<Rpc>, String> {
    let table = engine.net_rpcs().map_err(|e| format!("{e:?}"))?;
    table.iter().map(rpc).collect()
}

fn rpc(entry: &Rc<RpcEntry>) -> Result<Rpc, String> {
    let fields = |schema| {
        message_fields(schema)
            .map_err(|bad| format!("RPC `{}`: layout entry `{bad}` does not read", entry.name))
    };
    Ok(Rpc {
        name: entry.name.clone(),
        direction: if entry.direction == Direction::ToServer {
            RpcDirection::ToServer
        } else {
            RpcDirection::ToClient
        },
        reliable: entry.delivery == Delivery::Reliable,
        rate: entry.rate,
        burst: entry.burst,
        max_bytes: entry.max_bytes,
        allow: match &entry.allow {
            Allow::Anyone => "any connected client".to_string(),
            Allow::Permission(permission) => format!("callers who hold `{permission}`"),
            _ => "callers the plugin's own check accepts".to_string(),
        },
        script_handler: entry.script_function().map(str::to_string),
        request: fields(entry.request_schema())?,
        reply: fields(entry.response_schema())?,
        has_reply: entry.reply,
    })
}

/// The fields of a message from its layout text: `name:type:bits:max_len:finite` entries joined by `;`. A type may hold a `;`
/// of its own (`[f32;3]`), so only a `;` outside brackets ends an entry.
///
/// # Errors
/// The text of the first entry that does not read.
pub(crate) fn message_fields(schema: &str) -> Result<Vec<MessageField>, String> {
    let mut entries = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in schema.char_indices() {
        match c {
            '[' | '<' => depth += 1,
            ']' | '>' => depth = depth.saturating_sub(1),
            ';' if depth == 0 => {
                entries.push(&schema[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    entries.push(&schema[start..]);
    if schema.is_empty() {
        return Ok(Vec::new());
    }
    entries
        .into_iter()
        .map(|entry| message_field(entry).ok_or_else(|| entry.to_string()))
        .collect()
}

fn message_field(entry: &str) -> Option<MessageField> {
    let mut parts = entry.rsplitn(4, ':');
    let finite = parts.next()? == "1";
    let max_len = parts.next()?.parse().ok()?;
    let bits = parts.next()?.parse().ok()?;
    let (name, ty) = parts.next()?.split_once(':')?;
    Some(MessageField {
        name: name.to_string(),
        ty: ty.to_string(),
        bits,
        max_len,
        finite,
    })
}

fn lower(text: &str) -> String {
    text.to_ascii_lowercase()
}

/// The doc comment after its first line (which the usage text already has), without the blank lines around.
fn after_first_line(docs: &str) -> String {
    docs.split_once('\n')
        .map_or("", |(_, rest)| rest)
        .trim()
        .to_string()
}

/// A native of the listing: `ns::name min..max usage` (see `Vm::native_listing`).
fn parse_listing_line(
    line: &str,
    classes: &BTreeSet<String>,
    declared: &Declared,
    side: Side,
) -> Option<Native> {
    let (head, rest) = line.split_once(' ')?;
    let (counts, usage) = rest.split_once(' ').unwrap_or((rest, ""));
    let (min, max) = counts.split_once("..")?;
    let (min, max): (u32, Option<u32>) = (min.parse().ok()?, max.parse().ok());
    let (namespace, name) = head.split_once("::").unwrap_or(("", head));
    let key = (lower(namespace), lower(name));
    let declared = declared.get(&key);
    let method = declared.map_or_else(|| classes.contains(&key.0), |d| d.0);
    // The counts include the function's own name, and a method's object.
    let own = if method { 2 } else { 1 };
    let arity = Arity {
        min: min.saturating_sub(own),
        max: max.map(|m| m.saturating_sub(own)),
    };
    Some(Native {
        namespace: namespace.to_string(),
        name: name.to_string(),
        method,
        arity,
        usage: unescape(usage),
        docs: declared.map(|d| d.2.clone()).unwrap_or_default(),
        dev: declared.is_some_and(|d| d.1),
        sides: Sides::of(side),
        plugins: Vec::new(),
    })
}

/// The inverse of the listing's escapes for backslash, line feed and carriage return.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn merge_native(natives: &mut Vec<Native>, native: Native) {
    let same = |n: &&mut Native| {
        n.namespace.eq_ignore_ascii_case(&native.namespace)
            && n.name.eq_ignore_ascii_case(&native.name)
            && n.arity == native.arity
            && n.usage == native.usage
    };
    match natives.iter_mut().find(same) {
        Some(existing) => existing.sides.0.extend(native.sides.0),
        None => natives.push(native),
    }
}

fn class_names(vm: &Vm) -> Vec<String> {
    let classes = &vm.objects().classes;
    (0..classes.len() as u32)
        .filter_map(|id| classes.get(id))
        .map(|def| String::from_utf8_lossy(&def.name).into_owned())
        .collect()
}

/// The console classes of `vm`, each with the fields it adds to its parent's.
fn classes(vm: &Vm, side: Side) -> Vec<Class> {
    let classes = &vm.objects().classes;
    let mut out = Vec::new();
    for id in 0..classes.len() as u32 {
        let Some(def) = classes.get(id) else { continue };
        let parent = def
            .parent
            .map(|p| String::from_utf8_lossy(classes.name(p)).into_owned())
            .unwrap_or_default();
        let fields = def
            .fields
            .iter()
            .map(|field| {
                let slot = def
                    .layout
                    .field(&field.name)
                    .and_then(|(_, slots)| slots.first().copied());
                let default = slot
                    .and_then(|s| def.layout.initial.get(s))
                    .map(|value| {
                        let text =
                            fields::get_with(classes.field_rules, field.ty, value, &|_| None);
                        String::from_utf8_lossy(&text).into_owned()
                    })
                    .unwrap_or_default();
                Field {
                    name: String::from_utf8_lossy(&field.name).into_owned(),
                    ty: type_name(field.ty),
                    count: field.count,
                    default,
                    range: field.validator.map(range_text),
                    doc: field.doc.map(|d| d.trim().to_string()).unwrap_or_default(),
                    sides: Sides::of(side),
                }
            })
            .collect();
        out.push(Class {
            name: String::from_utf8_lossy(&def.name).into_owned(),
            parent,
            fields,
            declared: false,
            sides: Sides::of(side),
            plugins: Vec::new(),
        });
    }
    out
}

fn merge_class(classes: &mut Vec<Class>, class: Class) {
    let Some(existing) = classes
        .iter_mut()
        .find(|c| c.name.eq_ignore_ascii_case(&class.name))
    else {
        classes.push(class);
        return;
    };
    existing.sides.0.extend(class.sides.0);
    for field in class.fields {
        match existing
            .fields
            .iter_mut()
            .find(|f| f.name.eq_ignore_ascii_case(&field.name))
        {
            Some(have) => have.sides.0.extend(field.sides.0),
            None => existing.fields.push(field),
        }
    }
}

/// A validator's range as a reference shows it: `0 to 0.9`.
fn range_text(validator: Validator) -> String {
    match validator {
        Validator::FloatRange { min, max } => {
            format!(
                "{} to {}",
                format_g(f64::from(min)),
                format_g(f64::from(max))
            )
        }
        Validator::ScaledIntRange { min, max, .. } => format!("{min} to {max}"),
    }
}

/// A field type as `dumpConsoleClasses()` names it.
fn type_name(ty: FieldType) -> String {
    match ty {
        FieldType::String => "string".into(),
        FieldType::CaseString => "caseString".into(),
        FieldType::Filename => "filename".into(),
        FieldType::U8 => "U8".into(),
        FieldType::U16 => "U16".into(),
        FieldType::S32 => "int".into(),
        FieldType::F32 => "float".into(),
        FieldType::Bool => "bool".into(),
        FieldType::S32Vector => "intList".into(),
        FieldType::F32Vector => "floatList".into(),
        FieldType::Enum(table) => {
            let labels: Vec<&str> = table.entries.iter().map(|(_, label)| *label).collect();
            format!("enum ({})", labels.join(", "))
        }
        FieldType::ColorI => "ColorI".into(),
        FieldType::ColorF => "ColorF".into(),
        FieldType::Point2I => "Point2I".into(),
        FieldType::Point2F => "Point2F".into(),
        FieldType::Point3F => "Point3F".into(),
        FieldType::MatrixPosition => "MatrixPosition".into(),
        FieldType::MatrixRotation => "MatrixRotation".into(),
        FieldType::DataBlock(class) => format!("{class}Ptr"),
        FieldType::GuiProfile => "GuiProfile".into(),
        FieldType::Opaque(name) => name.to_string(),
        FieldType::Deprecated => "deprecated".into(),
    }
}
