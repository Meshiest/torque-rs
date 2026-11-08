//! The console functions: `ropeCreate` and kin.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use torque_engine::{Engine, Res, ScriptClass, ScriptError, VmMut, VmRef, function};
use torque_ts_text::atof;
use torque_ts_vm::vm::Vm;

use crate::data::{RopeData, RopeLook, RopeParams, RopeStyle, parse_color as parse_rgba};
use crate::image::SharedImage;
use crate::pass::SharedWorld;
use crate::sim::{Curve, RopeEnd, RopeError, RopeId, RopeWorld, Tangent};

/// The engine resource that holds the ropes every client of the engine shares.
#[derive(Clone, Default)]
pub struct RopeShared(pub SharedWorld);

impl RopeShared {
    /// A new, empty world.
    pub fn new() -> RopeShared {
        RopeShared(Arc::new(Mutex::new(RopeWorld::new())))
    }

    /// The world, for the length of the guard.
    pub fn lock(&self) -> MutexGuard<'_, RopeWorld> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// `"x y z"` as a point. Missing words are an error; a word that is not a number reads as 0, as TorqueScript's vectors do.
fn parse_point(text: &str, what: &str) -> Result<[f32; 3], ScriptError> {
    let mut words = text.split_ascii_whitespace();
    let mut next = || words.next().map(|w| atof(w.as_bytes()) as f32);
    match (next(), next(), next()) {
        (Some(x), Some(y), Some(z)) => Ok([x, y, z]),
        _ => Err(ScriptError::new(format!("{what} must be \"x y z\""))),
    }
}

fn parse_end(text: &str) -> Result<RopeEnd, ScriptError> {
    RopeEnd::parse(text).ok_or_else(|| ScriptError::new("the end must be 0 or start, 1 or end"))
}

fn rope_id(id: i32) -> RopeId {
    RopeId::try_from(id).unwrap_or(0)
}

fn refused(error: RopeError) -> ScriptError {
    ScriptError::new(error.to_string())
}

/// The number of the world's texture of the file `path` (0: none): the one the world holds, else the file loaded through the
/// engine's asset loaders and added. A file that is missing, too big or not an image warns once and is 0 (the rope is drawn plain).
fn texture_for(engine: &Engine, vm: &mut Vm, ropes: &RopeShared, path: &str, nearest: bool) -> u32 {
    let path = path.trim();
    if path.is_empty() {
        return 0;
    }
    let held = ropes.lock().textures().find(path, nearest);
    if held != 0 {
        return held;
    }
    let loaded = engine
        .assets(vm)
        .load::<SharedImage>(vm.file_host(), path)
        .map(|image| Arc::clone(&image.0))
        .map_err(|e| e.to_string())
        .and_then(|image| {
            ropes
                .lock()
                .add_texture(path, nearest, image)
                .map_err(|e| e.to_string())
        });
    match loaded {
        Ok(id) => id,
        Err(why) => {
            if ropes.lock().first_warning(path) {
                vm.warn(format!(
                    "rope texture {path}: {why}; the rope is drawn plain"
                ));
            }
            0
        }
    }
}

/// `"r g b"` or `"r g b a"` as a color, each number from 0 to 255 (a number past that is held to it; alpha is 255 when left out).
fn parse_color(text: &str) -> Result<[u8; 4], ScriptError> {
    parse_rgba(text).ok_or_else(|| ScriptError::new("the color must be \"r g b a\" from 0 to 255"))
}

/// makes a rope of the datablock between two points, both ends pinned, and returns its number
///
/// `start` and `end` are points as `"x y z"`; `datablock` is a `RopeData` (its name or number). The rope hangs in a sag between
/// the points as long as the datablock's `length` (or 1.1 times the distance between them); a datablock whose `style` is `line`
/// makes a straight strip between the points that is not simulated. At most 256 ropes and 4096 segments can be made (a line is
/// one segment); past that the call prints an error and returns nothing.
#[function(name = "ropeCreate")]
fn rope_create(
    mut vm: VmMut,
    engine: &Engine,
    ropes: Res<RopeShared>,
    start: &str,
    end: &str,
    datablock: &str,
) -> Result<i32, ScriptError> {
    let start = parse_point(start, "the start")?;
    let end = parse_point(end, "the end")?;
    let id = vm
        .objects()
        .find(datablock.as_bytes())
        .ok_or_else(|| ScriptError::new(format!("there is no object {datablock:?}")))?;
    let data = RopeData::read(&vm, id).map_err(|e| ScriptError::new(e.to_string()))?;
    if !data.sim.trim().is_empty()
        && !["verlet", "none"].contains(&data.sim.trim().to_ascii_lowercase().as_str())
    {
        vm.warn(format!(
            "ropeCreate: the sim {:?} is not verlet or none; the rope is simulated",
            data.sim
        ));
    }
    if RopeStyle::parse(&data.style).is_none() {
        vm.warn(format!(
            "ropeCreate: the style {:?} is not tube, ribbon or line; the rope is a tube",
            data.style
        ));
    }
    let mut params = RopeParams::new(&data);
    params.texture = texture_for(
        engine,
        &mut vm,
        &ropes,
        &data.texture.0,
        params.texture_nearest,
    );
    let id = ropes.lock().create(params, start, end).map_err(refused)?;
    Ok(id as i32)
}

/// takes a rope away; true if there was one
#[function(name = "ropeDelete")]
fn rope_delete(ropes: Res<RopeShared>, rope: i32) -> bool {
    ropes.lock().remove(rope_id(rope))
}

/// takes every rope away
#[function(name = "ropeClear")]
fn rope_clear(ropes: Res<RopeShared>) {
    ropes.lock().clear();
}

/// how many ropes there are
#[function(name = "ropeCount")]
fn rope_count(ropes: Res<RopeShared>) -> i32 {
    ropes.lock().len() as i32
}

/// pins an end of a rope to a point; `end` is 0 or start, 1 or end
///
/// The end glides to the point over the next tick. True if the rope exists.
#[function(name = "ropePin")]
fn rope_pin(
    ropes: Res<RopeShared>,
    rope: i32,
    end: &str,
    point: &str,
) -> Result<bool, ScriptError> {
    let end = parse_end(end)?;
    let point = parse_point(point, "the point")?;
    Ok(ropes.lock().pin_point(rope_id(rope), end, point).is_ok())
}

/// hangs an end of a rope from an object (its name or number); `end` is 0 or start, 1 or end
///
/// The end follows the object's `position` (or the first three numbers of its `transform`) as the client reads it every tick, and
/// lets go if the object is gone. `node` names a place on the object for a game that supplies its own anchor reader; the
/// default reader ignores it. True if the rope exists.
#[function(name = "ropeAttach")]
fn rope_attach(
    vm: VmRef,
    ropes: Res<RopeShared>,
    rope: i32,
    end: &str,
    object: &str,
    node: Option<&str>,
) -> Result<bool, ScriptError> {
    let end = parse_end(end)?;
    let id = vm
        .objects()
        .find(object.as_bytes())
        .ok_or_else(|| ScriptError::new(format!("there is no object {object:?}")))?;
    Ok(ropes
        .lock()
        .pin_object(rope_id(rope), end, id, node.unwrap_or_default())
        .is_ok())
}

/// lets an end of a rope go, so it falls with the rest; `end` is 0 or start, 1 or end
#[function(name = "ropeRelease")]
fn rope_release(ropes: Res<RopeShared>, rope: i32, end: &str) -> Result<bool, ScriptError> {
    let end = parse_end(end)?;
    Ok(ropes.lock().release(rope_id(rope), end).is_ok())
}

/// gives the free part of a rope a velocity `"x y z"`, in units a second (at most 200)
#[function(name = "ropeImpulse")]
fn rope_impulse(ropes: Res<RopeShared>, rope: i32, velocity: &str) -> Result<bool, ScriptError> {
    let velocity = parse_point(velocity, "the velocity")?;
    Ok(ropes.lock().impulse(rope_id(rope), velocity).is_ok())
}

/// keeps the free points of every rope above the height `z`
#[function(name = "ropeFloor")]
fn rope_floor(ropes: Res<RopeShared>, z: f32) {
    ropes.lock().set_floor(Some(z));
}

/// takes the floor away
#[function(name = "ropeNoFloor")]
fn rope_no_floor(ropes: Res<RopeShared>) {
    ropes.lock().set_floor(None);
}

/// sets the color one rope is drawn with, `"r g b a"` from 0 to 255 (a texture is tinted by it); an empty string puts the datablock's back
///
/// A color that is not three or four numbers is an error. True if the rope exists.
#[function(name = "ropeSetColor")]
fn rope_set_color(ropes: Res<RopeShared>, rope: i32, color: &str) -> Result<bool, ScriptError> {
    let clear = color.trim().is_empty();
    let set = RopeLook {
        color: if clear {
            None
        } else {
            Some(parse_color(color)?)
        },
        ..RopeLook::default()
    };
    Ok(ropes
        .lock()
        .set_look(rope_id(rope), set, [clear, false, false, false])
        .is_ok())
}

/// the color a rope is drawn with now, as `"r g b a"` (empty for no such rope)
#[function(name = "ropeGetColor")]
fn rope_get_color(ropes: Res<RopeShared>, rope: i32) -> String {
    let world = ropes.lock();
    world.rope(rope_id(rope)).map_or(String::new(), |r| {
        let [a, b, c, d] = r.appearance(&world.textures()).color;
        format!("{a} {b} {c} {d}")
    })
}

/// sets the texture one rope is drawn with (a .png or .jpg); an empty string puts the datablock's back
///
/// A file that is missing or does not load warns once and the rope is drawn plain. True if the rope exists.
#[function(name = "ropeSetTexture")]
fn rope_set_texture(
    mut vm: VmMut,
    engine: &Engine,
    ropes: Res<RopeShared>,
    rope: i32,
    file: &str,
) -> bool {
    let id = rope_id(rope);
    let file = file.trim();
    let Some(nearest) = ropes.lock().rope(id).map(|r| r.params().texture_nearest) else {
        return false;
    };
    if file.is_empty() {
        return ropes
            .lock()
            .set_look(id, RopeLook::default(), [false, true, false, false])
            .is_ok();
    }
    // The file is loaded (and the failure warned about) now; the rope names it either way.
    texture_for(engine, &mut vm, &ropes, file, nearest);
    let set = RopeLook {
        texture: Some(file.to_owned()),
        ..RopeLook::default()
    };
    ropes
        .lock()
        .set_look(id, set, [false, false, false, false])
        .is_ok()
}

/// the file of the texture a rope is drawn with now (empty for none, or no such rope)
#[function(name = "ropeGetTexture")]
fn rope_get_texture(ropes: Res<RopeShared>, rope: i32) -> String {
    let world = ropes.lock();
    let Some(rope) = world.rope(rope_id(rope)) else {
        return String::new();
    };
    match &rope.look().texture {
        Some(path) => path.clone(),
        None => world
            .textures()
            .get(rope.params().texture)
            .map_or(String::new(), |e| e.path.clone()),
    }
}

/// sets how fast one rope's texture runs along it, in repeats a second (negative: toward the start)
///
/// A speed that is not a number is 0. True if the rope exists.
#[function(name = "ropeSetTextureScroll")]
fn rope_set_texture_scroll(ropes: Res<RopeShared>, rope: i32, speed: f32) -> bool {
    let speed = if speed.is_finite() {
        speed.clamp(-1000.0, 1000.0)
    } else {
        0.0
    };
    let set = RopeLook {
        scroll: Some(speed),
        ..RopeLook::default()
    };
    ropes
        .lock()
        .set_look(rope_id(rope), set, [false; 4])
        .is_ok()
}

/// how fast a rope's texture runs now, in repeats a second (0 for no such rope)
#[function(name = "ropeGetTextureScroll")]
fn rope_get_texture_scroll(ropes: Res<RopeShared>, rope: i32) -> f32 {
    let world = ropes.lock();
    world
        .rope(rope_id(rope))
        .map_or(0.0, |r| r.appearance(&world.textures()).scroll)
}

/// sets whether one rope, drawn as a ribbon or a line, glows (drawn with no light or shade)
///
/// True if the rope exists.
#[function(name = "ropeSetEmissive")]
fn rope_set_emissive(ropes: Res<RopeShared>, rope: i32, on: bool) -> bool {
    let set = RopeLook {
        emissive: Some(on),
        ..RopeLook::default()
    };
    ropes
        .lock()
        .set_look(rope_id(rope), set, [false; 4])
        .is_ok()
}

/// whether a rope glows now
#[function(name = "ropeGetEmissive")]
fn rope_get_emissive(ropes: Res<RopeShared>, rope: i32) -> bool {
    let world = ropes.lock();
    world
        .rope(rope_id(rope))
        .is_some_and(|r| r.appearance(&world.textures()).emissive)
}

/// puts every value a script set on one rope (its color, texture, texture speed and glow) back to the datablock's
///
/// True if the rope exists.
#[function(name = "ropeClearLook")]
fn rope_clear_look(ropes: Res<RopeShared>, rope: i32) -> bool {
    ropes
        .lock()
        .set_look(rope_id(rope), RopeLook::default(), [true; 4])
        .is_ok()
}

/// `""` (toward the other end), `"mount"` (along the end's mount) or `"x y z"` (a direction) as the way a curve leaves an end.
fn parse_tangent(text: &str) -> Result<Tangent, ScriptError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Tangent::Auto);
    }
    if text.eq_ignore_ascii_case("mount") {
        return Ok(Tangent::Mount);
    }
    let bad = || ScriptError::new("a direction must be \"x y z\", \"mount\" or empty");
    let [x, y, z] = parse_point(text, "a direction").map_err(|_| bad())?;
    if !(x.is_finite() && y.is_finite() && z.is_finite()) || (x == 0.0 && y == 0.0 && z == 0.0) {
        return Err(bad());
    }
    Ok(Tangent::Fixed([x, y, z]))
}

/// gives one rope a driven shape: a curve from its start to its far end, not simulated (a beam from a muzzle)
///
/// The curve leaves the start along `startDir` and the far end along `endDir`, each `""` (toward the other end), `"mount"` (the way
/// the end's mount points as the frame draws it, for an end attached to a ghost's muzzle) or a direction `"x y z"`; the handles are
/// `bend` times the distance between the ends long (0 is straight; held to -4 to 4). Both ends and the mounts' directions are
/// followed each frame. A direction that is none of those, or a bend that is not a number, is an error. True if the rope exists.
#[function(name = "ropeSetCurve")]
fn rope_set_curve(
    ropes: Res<RopeShared>,
    rope: i32,
    start_dir: &str,
    end_dir: &str,
    bend: f32,
) -> Result<bool, ScriptError> {
    let (start, end) = (parse_tangent(start_dir)?, parse_tangent(end_dir)?);
    if !bend.is_finite() {
        return Err(ScriptError::new("the bend must be a number"));
    }
    let curve = Curve {
        start,
        end,
        bend: bend.clamp(-4.0, 4.0),
    };
    Ok(ropes.lock().set_curve(rope_id(rope), Some(curve)).is_ok())
}

/// takes a rope's driven curve away: it is simulated again if its datablock says so, else straight between its ends
///
/// True if the rope exists.
#[function(name = "ropeClearCurve")]
fn rope_clear_curve(ropes: Res<RopeShared>, rope: i32) -> bool {
    ropes.lock().set_curve(rope_id(rope), None).is_ok()
}

/// the bend of a rope's driven curve (0 for none, or no such rope)
#[function(name = "ropeGetCurveBend")]
fn rope_get_curve_bend(ropes: Res<RopeShared>, rope: i32) -> f32 {
    ropes
        .lock()
        .rope(rope_id(rope))
        .and_then(|r| r.curve())
        .map_or(0.0, |c| c.bend)
}

/// The natives, as the plugin registers them.
pub(crate) fn declared() -> Vec<torque_engine::script::NativeDecl> {
    torque_engine::natives![
        rope_create,
        rope_delete,
        rope_clear,
        rope_count,
        rope_pin,
        rope_attach,
        rope_release,
        rope_impulse,
        rope_floor,
        rope_no_floor,
        rope_set_color,
        rope_get_color,
        rope_set_texture,
        rope_get_texture,
        rope_set_texture_scroll,
        rope_get_texture_scroll,
        rope_set_emissive,
        rope_get_emissive,
        rope_clear_look,
        rope_set_curve,
        rope_clear_curve,
        rope_get_curve_bend
    ]
}
