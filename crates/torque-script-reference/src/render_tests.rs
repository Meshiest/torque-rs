//! The pages' text for a small reference.

use crate::model::{
    Arity, Class, Field, MessageField, Native, Pref, Reference, Rpc, RpcDirection, Side, Sides,
};
use crate::render::pages;

fn native(
    namespace: &str,
    name: &str,
    method: bool,
    usage: &str,
    arity: (u32, Option<u32>),
) -> Native {
    Native {
        namespace: namespace.to_string(),
        name: name.to_string(),
        method,
        arity: Arity {
            min: arity.0,
            max: arity.1,
        },
        usage: usage.to_string(),
        docs: String::new(),
        dev: false,
        sides: Sides::of(Side::Client),
        plugins: Vec::new(),
    }
}

fn reference() -> Reference {
    let mut set = native(
        "Pad",
        "setDeadZone",
        false,
        "Pad::setDeadZone(zone) - sets the dead zone",
        (1, Some(1)),
    );
    set.docs = "Kept between 0 and 0.75.".to_string();
    set.sides.0.insert(Side::Server);
    let mut gizmo = native("", "drawDebug", false, "", (0, Some(0)));
    gizmo.plugins = vec!["torque.gizmos".to_string()];
    let mut slot = native("", "setPredictor", false, "(id)", (1, Some(1)));
    slot.plugins = vec![crate::model::SLOT_TAG.to_string()];
    Reference {
        natives: vec![
            native("", "zeta", false, "(S32 sequence)", (1, Some(1))),
            native(
                "SimObject",
                "dump",
                true,
                "(string a)Prints.\n@note Long.",
                (0, Some(1)),
            ),
            native("", "Alpha", false, "alpha(a, b...);", (1, None)),
            set,
            gizmo,
            slot,
        ],
        classes: vec![Class {
            name: "RopeData".to_string(),
            parent: "SimDataBlock".to_string(),
            fields: vec![Field {
                name: "segments".to_string(),
                ty: "int".to_string(),
                count: 1,
                default: "8".to_string(),
                range: Some("2 to 64".to_string()),
                doc: "How many | pieces".to_string(),
                sides: Sides::of(Side::Client),
            }],
            declared: true,
            sides: Sides::of(Side::Client),
            plugins: Vec::new(),
        }],
        prefs: vec![Pref {
            name: "$Pref::Pad::DeadZone".to_string(),
            ty: "float".to_string(),
            default: "0.15".to_string(),
            range: Some("0 to 0.9".to_string()),
            doc: "How far.".to_string(),
            sides: Sides::of(Side::Client),
            plugins: Vec::new(),
        }],
        optional: vec![("torque.gizmos".to_string(), "debug drawing".to_string())],
        rpcs: vec![rpc()],
    }
}

fn message_field(name: &str, ty: &str, bits: u8, max_len: usize, finite: bool) -> MessageField {
    MessageField {
        name: name.to_string(),
        ty: ty.to_string(),
        bits,
        max_len,
        finite,
    }
}

fn rpc() -> Rpc {
    Rpc {
        name: "pad.rumble".to_string(),
        direction: RpcDirection::ToServer,
        reliable: true,
        rate: 4,
        burst: 8,
        max_bytes: 128,
        allow: "any connected client".to_string(),
        script_handler: Some("rpcServer_pad_rumble".to_string()),
        request: vec![
            message_field("label", "String", 0, 32, false),
            message_field("strength", "f32", 0, 0, true),
            message_field("steps", "u8", 8, 0, false),
        ],
        reply: vec![message_field("done", "bool", 0, 0, false)],
        has_reply: true,
    }
}

fn page(name: &str) -> String {
    pages(&reference())
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| text)
        .expect("the page")
}

#[test]
fn functions_are_sorted_without_case_and_show_their_signature() {
    let text = page("functions.md");
    let alpha = text.find("`alpha(a, b...)`").expect("alpha");
    let zeta = text.find("`zeta(S32 sequence)`").expect("zeta");
    assert!(alpha < zeta);
    assert!(
        text.contains("- `alpha(a, b...)`\n  *arguments: at least 1; client*"),
        "{text}"
    );
    assert!(
        text.contains(
            "- `Pad::setDeadZone(zone)` - sets the dead zone\n  *arguments: 1; server and client*"
        ),
        "{text}"
    );
    assert!(text.contains("  Kept between 0 and 0.75."), "{text}");
}

#[test]
fn the_prediction_slots_functions_need_a_predictor_not_one_plugin() {
    let text = page("functions.md");
    assert!(
        text.contains(
            "*arguments: 1; client; needs a plugin with a predictor or a prediction part*"
        ),
        "{text}"
    );
}

#[test]
fn an_opt_in_plugin_is_named_and_a_native_without_a_signature_gets_parentheses() {
    let text = page("functions.md");
    assert!(
        text.contains("- `drawDebug()`\n  *arguments: none; client; needs `torque.gizmos`*"),
        "{text}"
    );
}

#[test]
fn a_methods_extra_lines_go_in_a_block() {
    let text = page("methods.md");
    assert!(
        text.contains("- `obj.dump(string a)` - Prints.\n"),
        "{text}"
    );
    assert!(text.contains("  ```text\n  @note Long.\n  ```\n"), "{text}");
}

#[test]
fn classes_and_prefs_have_tables() {
    let classes = page("classes.md");
    assert!(
        classes.contains("| `segments` | int | `8` | 2 to 64 | How many \\| pieces |"),
        "{classes}"
    );
    let prefs = page("prefs.md");
    assert!(
        prefs
            .contains("| `$Pref::Pad::DeadZone` | float | `0.15` | 0 to 0.9 | How far. | client |"),
        "{prefs}"
    );
}

#[test]
fn the_index_counts_and_lists_the_opt_in_plugins() {
    let text = page("index.md");
    assert!(
        text.contains("| [Functions](functions.md) | console functions, by namespace | 5 |"),
        "{text}"
    );
    assert!(text.contains("- `torque.gizmos`: debug drawing"), "{text}");
}

#[test]
fn the_lua_page_shows_the_calls_as_lua_writes_them() {
    let text = page("lua.md");
    assert!(
        text.contains("- `ts.call(\"Alpha\", a, b...)` *at least 1; client*"),
        "{text}"
    );
    assert!(
        text.contains("- `ts.call(\"Pad::setDeadZone\", zone)`"),
        "{text}"
    );
    assert!(text.contains("- `obj:dump(string a)`"), "{text}");
}

#[test]
fn the_rpc_page_shows_each_rpc_and_the_shape_of_its_fields() {
    let text = page("rpcs.md");
    assert!(
        text.contains(
            "### pad.rumble
"
        ),
        "{text}"
    );
    assert!(
        text.contains("a call with a reply; reliable; 4 calls a second, a burst of 8; at most 128 bytes; may be called by any connected client*"),
        "{text}"
    );
    assert!(
        text.contains("`rpcServer_pad_rumble(%client, label, strength, steps)`"),
        "{text}"
    );
    assert!(
        text.contains(
            "| 1 | `label` | `String` |  | 32 |  |
| 2 | `strength` | `f32` |  |  | yes |
| 3 | `steps` | `u8` | 8 |  |  |
"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "Reply:

| Argument | Field | Type | Bits | Most | Finite |
|---|---|---|---|---|---|
| 1 | `done` | `bool` |"
        ),
        "{text}"
    );
}

#[test]
fn the_rpc_functions_need_a_plugin_that_registers_an_rpc() {
    let mut reference = reference();
    let mut call = native("", "rpcToServer", false, "(name, fields..)", (1, None));
    call.plugins = vec![crate::model::RPC_TAG.to_string()];
    reference.natives.push(call);
    let text = pages(&reference)
        .into_iter()
        .find(|(n, _)| *n == "functions.md")
        .map(|(_, text)| text)
        .expect("the page");
    assert!(
        text.contains("*arguments: at least 1; client; needs a plugin that registers an RPC*"),
        "{text}"
    );
}

#[test]
fn rendering_twice_gives_the_same_text() {
    assert_eq!(pages(&reference()), pages(&reference()));
}
