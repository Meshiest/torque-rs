//! A shape as the client loads and poses it, the parts that read no world: a constructor's sequences imported into the shape
//! (`TSShapeConstructor`: each `sequenceN`, "file name"; a text after the file name renames the file's last sequence; the file
//! is looked for beside the base shape, a host's convention), the script threads added to an instance, a Player's threads at
//! rest, and an `.ifl` file's frames.
//!
//! Credit: the import follows TGE 1.4's `ts/tsShapeConstruct.cc` (Copyright (C) GarageGames.com, Inc.) as Torque3D carries it
//! under the MIT licence (`ts/tsShapeConstruct.cpp`, Copyright (c) 2012 GarageGames, LLC); a Player's threads at rest follow
//! TGE 1.4's `game/player.cc` (Copyright (C) GarageGames.com, Inc.), in Torque3D `T3D/player.cpp` (Copyright (c) 2012
//! GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

use ghost_world::set::ClientSet;
use ghost_world::world::ClientWorld;
use shapes::instance::{ShapeInstance, find_sequence};
use shapes::shape::Shape;

use crate::Assets;

/// The largest `.dsq` read.
const MAX_DSQ_BYTES: usize = 1 << 20;
/// The longest `.ifl` read, and the most frames kept of one.
const MAX_IFL_BYTES: usize = 1 << 16;
const MAX_IFL_FRAMES: usize = 1024;

/// Imports the constructor's sequences into `shape` (the file beside the base shape, then the new name of its last sequence).
pub fn import(
    shape: &mut Shape,
    dir: &str,
    seqs: &[Vec<u8>],
    assets: &dyn Assets,
    notes: &mut Vec<String>,
) {
    for value in seqs {
        let cut = value
            .iter()
            .position(|&b| b == b' ' || b == b'\t')
            .unwrap_or(value.len());
        let file = String::from_utf8_lossy(&value[..cut]).replace('\\', "/");
        let rename: Vec<u8> = value[cut..]
            .iter()
            .copied()
            .skip_while(|&b| b == b' ' || b == b'\t')
            .collect();
        let path = if dir.is_empty() {
            file.clone()
        } else {
            format!("{dir}/{file}")
        };
        let Some(bytes) = assets.read(&path).filter(|b| b.len() <= MAX_DSQ_BYTES) else {
            notes.push(format!("sequence {path}: not read"));
            continue;
        };
        let before = shape.sequences.len();
        let mut charge = |_: u64| true;
        let imported = shapes::dsq::parse(&bytes, &mut charge)
            .and_then(|dsq| shape.import_sequences(&dsq, &mut charge));
        if let Err(e) = imported {
            notes.push(format!("sequence {path}: {e:?}"));
            continue;
        }
        if !rename.is_empty() && shape.sequences.len() > before {
            let index = match shape.find_name(&rename) {
                Some(i) => i,
                None => {
                    shape.names.push(rename.clone());
                    (shape.names.len() - 1) as i32
                }
            };
            if let Some(last) = shape.sequences.last_mut() {
                last.name = index;
            }
        }
    }
}

/// A script thread as the renderer poses it: the sequence, its speed and the seconds it has played.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScriptPose {
    pub sequence: u32,
    pub time_scale: f32,
    pub seconds: f32,
}

/// The script threads of ghost `index` at the world's clock, in slot order.
pub fn script_poses<S: ClientSet>(world: &ClientWorld<S>, index: u32) -> Vec<ScriptPose> {
    world.threads.get(&index).map_or_else(Vec::new, |slots| {
        slots
            .iter()
            .flatten()
            .map(|t| ScriptPose {
                sequence: t.sequence,
                time_scale: t.time_scale(),
                seconds: t.seconds(world.clock_ms),
            })
            .collect()
    })
}

/// Adds the script threads to `instance` (each `addThread`, `setSequence(seq, 0)`, its speed, then the time it played: a sequence
/// that does not cycle holds at its end). Whether any was added.
pub fn add_script_threads(
    shape: &Shape,
    instance: &mut ShapeInstance,
    script: &[ScriptPose],
) -> bool {
    let mut added = false;
    for s in script {
        if s.sequence as usize >= shape.sequences.len() {
            continue;
        }
        let Some(t) = instance.create_thread(shape) else {
            continue;
        };
        instance.set_sequence(shape, t, s.sequence as i32, 0.0);
        if let Some(th) = instance.thread_mut(t) {
            th.time_scale = s.time_scale;
        }
        if s.seconds.is_finite() && s.seconds > 0.0 {
            instance.advance_time(shape, t, s.seconds);
        }
        added = true;
    }
    added
}

/// `Player::mArmRange` and `mHeadVRange`: the head pitch the `look` and `head` threads span (-80 to +80 degrees).
const LOOK_MIN: f32 = -80.0 * (std::f32::consts::PI / 180.0);
const LOOK_MAX: f32 = 80.0 * (std::f32::consts::PI / 180.0);

/// A Player's own threads at rest as TGE makes them (`Player::onNewDataBlock`'s order): the arm thread on `look`, the `head` and
/// `headside` threads, each held still, then the action thread on `root` at its start (`setActionThread(RootAnim)`). The look
/// threads at the head pitch `pitch` and side angle `side` (`updateLookAnimation`): `look` and `head` at
/// `(pitch - min) / (max - min)` clamped to 0..1, `headside` at `(side + 1) / 2` clamped (`side` the head's z as a fraction of
/// `maxLookAngle`; 0 when the pose carries no side angle). TGE holds the arm thread at 0.5 while the Player controls another
/// object; the rest pose takes it as controlling none.
pub fn rest_threads(shape: &Shape, instance: &mut ShapeInstance, pitch: f32, side: f32) {
    let at = ((pitch - LOOK_MIN) / (LOOK_MAX - LOOK_MIN)).clamp(0.0, 1.0);
    let side = ((side + 1.0) * 0.5).clamp(0.0, 1.0);
    for (name, pos) in [(&b"look"[..], at), (b"head", at), (b"headside", side)] {
        let Some(seq) = find_sequence(shape, name) else {
            continue;
        };
        if let Some(t) = instance.create_thread(shape) {
            instance.set_sequence(shape, t, seq as i32, pos);
            if let Some(th) = instance.thread_mut(t) {
                th.time_scale = 0.0;
            }
        }
    }
    if let Some(t) = instance.create_thread(shape)
        && let Some(root) = find_sequence(shape, b"root")
    {
        instance.set_sequence(shape, t, root as i32, 0.0);
    }
}

/// The frames of an `.ifl` file: one game path a line (a duration after it is dropped).
pub fn ifl_frames(assets: &dyn Assets, path: &str) -> Option<Vec<String>> {
    let bytes = assets.read(path).filter(|b| b.len() <= MAX_IFL_BYTES)?;
    Some(
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .map(|f| f.replace('\\', "/"))
            .take(MAX_IFL_FRAMES)
            .collect(),
    )
}
