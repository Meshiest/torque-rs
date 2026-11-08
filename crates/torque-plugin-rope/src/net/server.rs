//! The server's half: the `Rope` console class a networked rope is (its fields are what [`RopeGhost`](super::records::RopeGhost) is
//! packed from) and the `ropeNet*` functions a server script makes and changes ropes with. Nothing here simulates a rope: the
//! server keeps what a rope was told, and each client simulates and draws it.

use std::cell::RefCell;
use std::collections::HashMap;

use torque_engine::{EngineClass, Res, ScriptClass, ScriptError, VmMut, function};
use torque_ts_text::atof;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use super::records::{
    MASK_CURVE, MASK_END, MASK_LENGTH, MASK_LIFE, MASK_LOOK, MASK_START, MAX_NODE_BYTES,
    MAX_TEXTURE_BYTES, parse_tangent,
};
use crate::data::{RopeData, RopeStyle, parse_color};
use crate::sim::{MAX_COORD, MAX_SECONDS, MIN_LENGTH, RopeEnd};

/// The most networked ropes a server holds at once.
pub const MAX_NET_ROPES: usize = 256;

/// The most seconds a one-shot rope (`ropeNetFx`) lasts.
pub const MAX_FX_SECONDS: f32 = 30.0;

/// The most one-shot ropes the server sends one client in a second; past it a call drops them for that client.
pub const FX_PER_SECOND: u32 = 32;

/// One client's second of one-shot ropes.
struct FxWindow {
    start: u32,
    count: u32,
    warned: bool,
}

/// What each client has been sent in the second just now: a script that calls `ropeNetFx` in a loop sends each client at most
/// [`FX_PER_SECOND`] a second (the client shows at most 32 at once, and every one is a message on its connection).
#[derive(Default)]
pub struct FxBudget(RefCell<HashMap<ObjId, FxWindow>>);

/// What [`FxBudget::spend`] decided.
enum Spend {
    Send,
    /// Over the budget; true the first time this client is.
    Drop(bool),
}

impl FxBudget {
    fn spend(&self, vm: &Vm, client: ObjId) -> Spend {
        let now = vm.sim_time();
        let mut windows = self.0.borrow_mut();
        if windows.len() > MAX_NET_ROPES {
            windows.retain(|c, _| vm.objects().contains(*c));
        }
        let window = windows.entry(client).or_insert(FxWindow {
            start: now,
            count: 0,
            warned: false,
        });
        if now.wrapping_sub(window.start) >= 1000 {
            window.start = now;
            window.count = 0;
        }
        if window.count < FX_PER_SECOND {
            window.count += 1;
            return Spend::Send;
        }
        Spend::Drop(!std::mem::replace(&mut window.warned, true))
    }
}

/// A networked rope: the object a server script makes with `ropeNetCreate` and changes with the other `ropeNet*` functions, and
/// every client of the engine sees as a rope. The fields are the server's own record of it, which a ghost is packed from; a script
/// sets them through the functions (they keep the marks that send only what changed), not by writing them.
#[allow(dead_code)]
#[derive(EngineClass)]
#[class(name = "Rope", parent = "SceneObject")]
pub struct NetRope {
    /// The `RopeData` datablock's object id.
    data_block: i32,
    /// What holds the start: 0 nothing, 1 a point, 2 an object.
    start_mode: i32,
    start_point: [f32; 3],
    /// The object the start hangs from (its id).
    start_object: i32,
    start_node: String,
    /// What holds the far end: as `startMode`.
    end_mode: i32,
    end_point: [f32; 3],
    end_object: i32,
    end_node: String,
    /// The length the rope goes to (0: its datablock's), the seconds it was given to get there and the server time it was set.
    length: f32,
    length_reel: f32,
    length_at: i32,
    /// The rope's own look: `"r g b a"` (empty: the datablock's), a file (empty: the datablock's), a number (empty: the
    /// datablock's) and 1 or 0 (-1: the datablock's).
    look_color: String,
    look_texture: String,
    look_scroll: String,
    #[field(default = -1)]
    look_emissive: i32,
    /// 1 when the rope has a driven curve, and how its ends leave.
    curve_mode: i32,
    curve_start: String,
    curve_end: String,
    curve_bend: f32,
    /// The server time the rope ends (0: it lasts), and the seconds it fades over before.
    life_end: i32,
    life_fade: f32,
    /// The scheduled call that deletes the rope at its end (0: none): a new lifetime takes the place of it.
    life_event: i32,
    /// The id the owner's client gave its predicted rope (0: none), and the owner's connection object.
    predict: i32,
    predict_owner: i32,
}

/// The class name as scripts see it.
const CLASS: &str = <NetRope as ScriptClass>::NAME;

/// `"x y z"` as a point, every coordinate a finite number held to the world.
fn parse_point(text: &str, what: &str) -> Result<[f32; 3], ScriptError> {
    let mut words = text.split_ascii_whitespace();
    let mut next = || words.next().map(|w| atof(w.as_bytes()) as f32);
    match (next(), next(), next()) {
        (Some(x), Some(y), Some(z)) => Ok([x, y, z].map(|c| {
            if c.is_finite() {
                c.clamp(-MAX_COORD, MAX_COORD)
            } else {
                0.0
            }
        })),
        _ => Err(ScriptError::new(format!("{what} must be \"x y z\""))),
    }
}

/// The networked rope `id`, if it is one.
fn rope_of(vm: &Vm, id: i32) -> Result<ObjId, ScriptError> {
    let obj = ObjId::try_from(id).unwrap_or(0);
    let objects = vm.objects();
    let is_rope = objects
        .get(obj)
        .is_some_and(|o| objects.classes.name(o.class()) == CLASS.as_bytes());
    if is_rope {
        Ok(obj)
    } else {
        Err(ScriptError::new(format!("{id} is not a networked rope")))
    }
}

fn put(vm: &mut Vm, obj: ObjId, field: &str, value: &str) {
    vm.set_object_member(obj, field.as_bytes(), b"", value.as_bytes());
}

fn mark(vm: &Vm, obj: ObjId, bits: u32) {
    torque_game::scene::ghost::mark(vm, obj, bits);
}

fn end_names(end: &str) -> Result<(&'static str, u32), ScriptError> {
    match RopeEnd::parse(end) {
        Some(RopeEnd::Start) => Ok(("start", MASK_START)),
        Some(RopeEnd::End) => Ok(("end", MASK_END)),
        None => Err(ScriptError::new("the end must be 0 or start, 1 or end")),
    }
}

/// How many networked ropes there are.
fn count(vm: &Vm) -> usize {
    let objects = vm.objects();
    objects
        .iter()
        .filter(|(_, o)| objects.classes.name(o.class()) == CLASS.as_bytes())
        .count()
}

/// makes a networked rope of the datablock between two points, and returns its object id
///
/// Every client of the engine sees the rope (the clients simulate and draw it). `start` and `end` are points `"x y z"` the two ends
/// are pinned to; `datablock` is a `RopeData` (name or id). `predictId` and `owner` tie the rope to a rope the client `owner` (a
/// connection) already made and gave that id with `ropePredict`: that client keeps its own rope instead of making a second one. At
/// most 256 networked ropes exist; past that the call prints an error and returns 0. The rope is an object like any other: put it
/// in a group the mission cleans up, and delete it with `ropeNetDelete` or `delete`.
#[function(name = "ropeNetCreate")]
fn net_create(
    mut vm: VmMut,
    start: &str,
    end: &str,
    datablock: &str,
    predict_id: Option<i32>,
    owner: Option<i32>,
) -> Result<i32, ScriptError> {
    let start = parse_point(start, "the start")?;
    let end = parse_point(end, "the end")?;
    let db = vm
        .objects()
        .find(datablock.as_bytes())
        .ok_or_else(|| ScriptError::new(format!("there is no object {datablock:?}")))?;
    let data = RopeData::read(&vm, db).map_err(|e| ScriptError::new(e.to_string()))?;
    if RopeStyle::parse(&data.style).is_none() {
        vm.warn(format!(
            "ropeNetCreate: the style {:?} is not tube, ribbon or line; the rope is a tube",
            data.style
        ));
    }
    if count(&vm) >= MAX_NET_ROPES {
        return Err(ScriptError::new(format!(
            "there are already {MAX_NET_ROPES} networked ropes"
        )));
    }
    let owner = owner
        .and_then(|o| ObjId::try_from(o).ok())
        .filter(|&o| vm.objects().contains(o));
    let text = vm
        .eval(format!("return new {CLASS}();").as_bytes())
        .map_err(|e| ScriptError::new(e.to_string()))?;
    let id = atof(&text) as i32;
    let obj = rope_of(&vm, id)?;
    put(&mut vm, obj, "dataBlock", &db.to_string());
    for (p, at) in [("start", start), ("end", end)] {
        put(&mut vm, obj, &format!("{p}Mode"), "1");
        put(
            &mut vm,
            obj,
            &format!("{p}Point"),
            &format!("{} {} {}", at[0], at[1], at[2]),
        );
    }
    let predict = predict_id.filter(|p| (1..=255).contains(p));
    if let (Some(predict), Some(owner)) = (predict, owner) {
        put(&mut vm, obj, "predict", &predict.to_string());
        put(&mut vm, obj, "predictOwner", &owner.to_string());
    }
    Ok(id)
}

/// takes a networked rope away from every client; true if there was one
#[function(name = "ropeNetDelete")]
fn net_delete(mut vm: VmMut, rope: i32) -> bool {
    let Ok(obj) = rope_of(&vm, rope) else {
        return false;
    };
    vm.delete_object(obj).is_ok()
}

/// pins an end of a networked rope to a point; `end` is 0 or start, 1 or end
#[function(name = "ropeNetPin")]
fn net_pin(mut vm: VmMut, rope: i32, end: &str, point: &str) -> Result<(), ScriptError> {
    let obj = rope_of(&vm, rope)?;
    let (p, bit) = end_names(end)?;
    let at = parse_point(point, "the point")?;
    put(&mut vm, obj, &format!("{p}Mode"), "1");
    put(
        &mut vm,
        obj,
        &format!("{p}Point"),
        &format!("{} {} {}", at[0], at[1], at[2]),
    );
    mark(&vm, obj, bit);
    Ok(())
}

/// hangs an end of a networked rope from an object (name or id), that every client follows as it draws it
///
/// `node` is a number (the middle of that mesh of the shape), `muzzle` (or `muzzle1`) for the muzzle of the object's weapon, or
/// empty for its origin; at most 24 bytes. The object is followed by its ghost, so a client that does not have it yet is sent
/// the end again once it does; a client with no ghost of it sees the end loose.
#[function(name = "ropeNetAttach")]
fn net_attach(
    mut vm: VmMut,
    rope: i32,
    end: &str,
    object: &str,
    node: Option<&str>,
) -> Result<(), ScriptError> {
    let obj = rope_of(&vm, rope)?;
    let (p, bit) = end_names(end)?;
    let target = vm
        .objects()
        .find(object.as_bytes())
        .ok_or_else(|| ScriptError::new(format!("there is no object {object:?}")))?;
    let mut node = node.unwrap_or_default().trim();
    if node.len() > MAX_NODE_BYTES {
        let mut cut = MAX_NODE_BYTES;
        while !node.is_char_boundary(cut) {
            cut -= 1;
        }
        node = &node[..cut];
    }
    put(&mut vm, obj, &format!("{p}Mode"), "2");
    put(&mut vm, obj, &format!("{p}Object"), &target.to_string());
    put(&mut vm, obj, &format!("{p}Node"), node);
    mark(&vm, obj, bit);
    Ok(())
}

/// lets an end of a networked rope go, so it falls with the rest on every client
#[function(name = "ropeNetRelease")]
fn net_release(mut vm: VmMut, rope: i32, end: &str) -> Result<(), ScriptError> {
    let obj = rope_of(&vm, rope)?;
    let (p, bit) = end_names(end)?;
    put(&mut vm, obj, &format!("{p}Mode"), "0");
    mark(&vm, obj, bit);
    Ok(())
}

/// sets the length of a networked rope; clients reel it from its length now to `length` evenly over `seconds`
///
/// Only the target and the time cross the network, once; each client makes the rope longer or shorter itself. A rope whose ends
/// are farther apart than its length is drawn taut and straight between them (it pulls nothing). `seconds` of 0 is at once; the most
/// is an hour.
#[function(name = "ropeNetSetLength")]
fn net_set_length(
    mut vm: VmMut,
    rope: i32,
    length: f32,
    seconds: Option<f32>,
) -> Result<(), ScriptError> {
    let obj = rope_of(&vm, rope)?;
    if !length.is_finite() || length <= 0.0 {
        return Err(ScriptError::new("the length must be a positive number"));
    }
    let seconds = seconds.filter(|s| s.is_finite() && *s > 0.0).unwrap_or(0.0);
    put(&mut vm, obj, "length", &length.max(MIN_LENGTH).to_string());
    put(
        &mut vm,
        obj,
        "lengthReel",
        &seconds.min(MAX_SECONDS).to_string(),
    );
    let now = vm.sim_time() as i32;
    put(&mut vm, obj, "lengthAt", &now.to_string());
    mark(&vm, obj, MASK_LENGTH);
    Ok(())
}

/// sets the color one networked rope is drawn with, `"r g b a"` from 0 to 255; an empty string puts the datablock's back
#[function(name = "ropeNetSetColor")]
fn net_set_color(mut vm: VmMut, rope: i32, color: &str) -> Result<(), ScriptError> {
    let obj = rope_of(&vm, rope)?;
    if !color.trim().is_empty() && parse_color(color).is_none() {
        return Err(ScriptError::new(
            "the color must be \"r g b a\" from 0 to 255",
        ));
    }
    put(&mut vm, obj, "lookColor", color.trim());
    mark(&vm, obj, MASK_LOOK);
    Ok(())
}

/// sets the texture one networked rope is drawn with (a .png or .jpg, at most 96 bytes of name); an empty string puts the
/// datablock's back
#[function(name = "ropeNetSetTexture")]
fn net_set_texture(mut vm: VmMut, rope: i32, file: &str) -> Result<(), ScriptError> {
    let obj = rope_of(&vm, rope)?;
    if file.trim().len() > MAX_TEXTURE_BYTES {
        return Err(ScriptError::new(format!(
            "a texture file name is at most {MAX_TEXTURE_BYTES} bytes"
        )));
    }
    put(&mut vm, obj, "lookTexture", file.trim());
    mark(&vm, obj, MASK_LOOK);
    Ok(())
}

/// sets how fast one networked rope's texture runs along it, in repeats a second; an empty string puts the datablock's back
#[function(name = "ropeNetSetTextureScroll")]
fn net_set_texture_scroll(mut vm: VmMut, rope: i32, speed: &str) -> Result<(), ScriptError> {
    let obj = rope_of(&vm, rope)?;
    let speed = speed.trim();
    if !speed.is_empty() && !speed.parse::<f32>().is_ok_and(f32::is_finite) {
        return Err(ScriptError::new("the speed must be a number"));
    }
    put(&mut vm, obj, "lookScroll", speed);
    mark(&vm, obj, MASK_LOOK);
    Ok(())
}

/// sets whether one networked rope glows; -1 puts the datablock's back
#[function(name = "ropeNetSetEmissive")]
fn net_set_emissive(mut vm: VmMut, rope: i32, on: i32) -> Result<(), ScriptError> {
    let obj = rope_of(&vm, rope)?;
    put(&mut vm, obj, "lookEmissive", &on.clamp(-1, 1).to_string());
    mark(&vm, obj, MASK_LOOK);
    Ok(())
}

/// puts everything one networked rope was given of its own (color, texture, speed, glow) back to the datablock's
#[function(name = "ropeNetClearLook")]
fn net_clear_look(mut vm: VmMut, rope: i32) -> Result<(), ScriptError> {
    let obj = rope_of(&vm, rope)?;
    for field in ["lookColor", "lookTexture", "lookScroll"] {
        put(&mut vm, obj, field, "");
    }
    put(&mut vm, obj, "lookEmissive", "-1");
    mark(&vm, obj, MASK_LOOK);
    Ok(())
}

/// gives a networked rope a driven curve (see `ropeSetCurve`): each of `startDir` and `endDir` is empty, `"mount"` or `"x y z"`
#[function(name = "ropeNetSetCurve")]
fn net_set_curve(
    mut vm: VmMut,
    rope: i32,
    start_dir: &str,
    end_dir: &str,
    bend: f32,
) -> Result<(), ScriptError> {
    let obj = rope_of(&vm, rope)?;
    if !bend.is_finite() {
        return Err(ScriptError::new("the bend must be a number"));
    }
    for (what, text) in [("start", start_dir), ("end", end_dir)] {
        let text = text.trim();
        let ok = text.is_empty()
            || text.eq_ignore_ascii_case("mount")
            || parse_tangent(text) != crate::sim::Tangent::Auto;
        if !ok {
            return Err(ScriptError::new(format!(
                "the {what} direction must be \"x y z\", \"mount\" or empty"
            )));
        }
    }
    put(&mut vm, obj, "curveMode", "1");
    put(&mut vm, obj, "curveStart", start_dir.trim());
    put(&mut vm, obj, "curveEnd", end_dir.trim());
    put(
        &mut vm,
        obj,
        "curveBend",
        &bend.clamp(-4.0, 4.0).to_string(),
    );
    mark(&vm, obj, MASK_CURVE);
    Ok(())
}

/// takes a networked rope's driven curve away
#[function(name = "ropeNetClearCurve")]
fn net_clear_curve(mut vm: VmMut, rope: i32) -> Result<(), ScriptError> {
    let obj = rope_of(&vm, rope)?;
    put(&mut vm, obj, "curveMode", "0");
    mark(&vm, obj, MASK_CURVE);
    Ok(())
}

/// makes a networked rope a temporary one: it is deleted `seconds` from now, and every client shows less of it over its last
/// `fade` seconds (a tube thins, a ribbon or a line grows fainter); `seconds` of 0 makes it last again
#[function(name = "ropeNetSetLifetime")]
fn net_set_lifetime(
    mut vm: VmMut,
    rope: i32,
    seconds: f32,
    fade: Option<f32>,
) -> Result<(), ScriptError> {
    let obj = rope_of(&vm, rope)?;
    if !seconds.is_finite() {
        return Err(ScriptError::new("the lifetime must be a number"));
    }
    // The call an earlier lifetime scheduled is taken back: a rope has one expiry, however often it is changed.
    let before = vm
        .objects()
        .field(obj, b"lifeEvent", b"")
        .map_or(0, |t| atof(&t) as i32);
    if before > 0 {
        let cancel = format!("if (isEventPending({before})) cancel({before});");
        vm.eval(cancel.as_bytes())
            .map_err(|e| ScriptError::new(e.to_string()))?;
        put(&mut vm, obj, "lifeEvent", "0");
    }
    if seconds <= 0.0 {
        put(&mut vm, obj, "lifeEnd", "0");
        put(&mut vm, obj, "lifeFade", "0");
    } else {
        let seconds = seconds.min(MAX_SECONDS);
        let fade = fade
            .filter(|f| f.is_finite())
            .unwrap_or(0.0)
            .clamp(0.0, seconds);
        let ms = (seconds * 1000.0) as u32;
        // A time of 0 means "lasts"; the rare end that lands on it is a millisecond later.
        let end = vm.sim_time().wrapping_add(ms).max(1);
        put(&mut vm, obj, "lifeEnd", &(end as i32).to_string());
        put(&mut vm, obj, "lifeFade", &fade.to_string());
        let ask = format!("return schedule({ms}, 0, \"ropeNetExpire\", {obj});");
        let event = vm
            .eval(ask.as_bytes())
            .map_err(|e| ScriptError::new(e.to_string()))?;
        put(&mut vm, obj, "lifeEvent", &String::from_utf8_lossy(&event));
    }
    mark(&vm, obj, MASK_LIFE);
    Ok(())
}

/// deletes a networked rope whose lifetime has run out (the call `ropeNetSetLifetime` schedules)
#[function(name = "ropeNetExpire")]
fn net_expire(mut vm: VmMut, rope: i32) {
    let Ok(obj) = rope_of(&vm, rope) else {
        return;
    };
    let read = |vm: &Vm, field: &str| {
        vm.objects()
            .field(obj, field.as_bytes(), b"")
            .map(|t| atof(&t))
            .unwrap_or(0.0) as i32
    };
    let end = read(&vm, "lifeEnd");
    // The rope lives on if it was given a later end (or none) since.
    if end != 0 && (vm.sim_time().wrapping_sub(end as u32) as i32) >= 0 {
        let _ = vm.delete_object(obj);
    }
}

/// sends a rope that lasts `seconds` (at most 30) between two points to a client, or to every client when `client` is empty or 0,
/// with no object to hold it: a grapple shot, an arc of lightning
///
/// It goes as a remote command (`clientCmdRopeFx` on the client), once, and is never a ghost; a client that runs the rope plugin
/// draws it as a temporary rope of the datablock, curved by `bend` (-4 to 4, 0 straight) when it is not 0, and fading out over its
/// last quarter second. A client holds at most 32 at a time, and is sent at most 32 a second: past that the call drops the rope for
/// that client (the console says so once for it). The function returns the number of clients it was sent to.
// A console function takes the arguments the script gives and what it uses of the engine, each as a parameter.
#[allow(clippy::too_many_arguments)]
#[function(name = "ropeNetFx")]
fn net_fx(
    mut vm: VmMut,
    budget: Res<FxBudget>,
    client: &str,
    start: &str,
    end: &str,
    datablock: &str,
    seconds: f32,
    bend: Option<f32>,
) -> Result<i32, ScriptError> {
    let start = parse_point(start, "the start")?;
    let end = parse_point(end, "the end")?;
    let db = vm
        .objects()
        .find(datablock.as_bytes())
        .ok_or_else(|| ScriptError::new(format!("there is no object {datablock:?}")))?;
    RopeData::read(&vm, db).map_err(|e| ScriptError::new(e.to_string()))?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(ScriptError::new("the seconds must be a positive number"));
    }
    let seconds = seconds.min(MAX_FX_SECONDS);
    let bend = bend
        .filter(|b| b.is_finite())
        .unwrap_or(0.0)
        .clamp(-4.0, 4.0);
    let targets: Vec<ObjId> = match vm.objects().find(client.as_bytes()) {
        Some(one) if !client.trim().is_empty() && client.trim() != "0" => vec![one],
        _ => {
            let objects = vm.objects();
            objects
                .find(b"ClientGroup")
                .and_then(|g| objects.members(g))
                .map(|m| m.as_slice().to_vec())
                .unwrap_or_default()
        }
    };
    let mut sent = 0;
    for target in targets {
        match budget.spend(&vm, target) {
            Spend::Send => {}
            Spend::Drop(first) => {
                if first {
                    vm.warn(format!(
                        "ropeNetFx: more than {FX_PER_SECOND} a second for client {target}; the rest are dropped"
                    ));
                }
                continue;
            }
        }
        let command = format!(
            "commandToClient({target}, 'RopeFx', \"{} {} {}\", \"{} {} {}\", {db}, {seconds}, {bend});",
            start[0], start[1], start[2], end[0], end[1], end[2]
        );
        if vm.eval(command.as_bytes()).is_ok() {
            sent += 1;
        }
    }
    Ok(sent)
}

/// The server's natives, as the plugin registers them.
pub(crate) fn declared() -> Vec<torque_engine::script::NativeDecl> {
    torque_engine::natives![
        net_create,
        net_delete,
        net_pin,
        net_attach,
        net_release,
        net_set_length,
        net_set_color,
        net_set_texture,
        net_set_texture_scroll,
        net_set_emissive,
        net_clear_look,
        net_set_curve,
        net_clear_curve,
        net_set_lifetime,
        net_expire,
        net_fx
    ]
}
