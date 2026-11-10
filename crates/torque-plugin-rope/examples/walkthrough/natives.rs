// ANCHOR: natives
use torque_engine::{Res, ScriptClass, ScriptError, VmRef, function};
use torque_ts_text::atof;

use crate::data::RopeData;
use crate::sim::{MAX_ROPES, Rope, Ropes};

/// `"x y z"` as a point.
fn point(text: &str) -> Result<[f32; 3], ScriptError> {
    let mut words = text
        .split_ascii_whitespace()
        .map(|w| atof(w.as_bytes()) as f32);
    match (words.next(), words.next(), words.next()) {
        (Some(x), Some(y), Some(z)) => Ok([x, y, z]),
        _ => Err(ScriptError::new(format!("{text:?} is not \"x y z\""))),
    }
}

/// makes a rope of the datablock between two points, and returns how many ropes there are
#[function(name = "ropeCreate")]
pub fn rope_create(
    vm: VmRef,
    ropes: Res<Ropes>,
    start: &str,
    end: &str,
    datablock: &str,
) -> Result<i32, ScriptError> {
    let (start, end) = (point(start)?, point(end)?);
    let id = vm
        .objects()
        .find(datablock.as_bytes())
        .ok_or_else(|| ScriptError::new(format!("there is no object {datablock:?}")))?;
    let data = RopeData::read(&vm, id).map_err(|e| ScriptError::new(e.to_string()))?;
    let mut ropes = ropes.lock();
    if ropes.len() >= MAX_ROPES {
        return Err(ScriptError::new(format!(
            "there are already {MAX_ROPES} ropes"
        )));
    }
    ropes.push(Rope::new(&data, start, end));
    Ok(ropes.len() as i32)
}

/// how many ropes there are
#[function(name = "ropeCount")]
pub fn rope_count(ropes: Res<Ropes>) -> i32 {
    ropes.lock().len() as i32
}
// ANCHOR_END: natives
