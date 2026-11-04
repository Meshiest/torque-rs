//! The `ShapeBaseImageData` view the state machine reads: the 31 state records (built once in
//! `onAdd` from the script arrays) and the scalars `updateImageState`, `setImage` and the muzzle chain read.
//!
//! This crate has no dependency on the protocol or datablock crates, so the host fills this: from the datablock's static fields
//! (`stateAllowImageChange`, `stateScript` and the rest, which a wire datablock may not carry), either as [`RawState`]s (the
//! transition names as the script wrote them, resolved here the same way) or directly as [`ImageState`]s.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/shapeImage.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use torque_collide::math::{IDENTITY, Matrix};

use super::{DbId, MAX_STATES};

/// One state record: the fields the server reads. The transitions are state indices, `-1` for none; `[0]`/`[1]` is the
/// condition false/true: `loaded` = [NotLoaded, Loaded], `ammo` = [NoAmmo, Ammo], `target` = [NoTarget, Target], `trigger` =
/// [TriggerUp, TriggerDown], `wet` = [NotWet, Wet].
#[derive(Debug, Clone, PartialEq)]
pub struct ImageState {
    /// `stateName` (what `getImageState` answers).
    pub name: String,
    /// The `loaded` transitions.
    pub loaded: [i32; 2],
    /// The `ammo` transitions.
    pub ammo: [i32; 2],
    /// The `target` transitions.
    pub target: [i32; 2],
    /// The `trigger` transitions.
    pub trigger: [i32; 2],
    /// The `wet` transitions.
    pub wet: [i32; 2],
    /// `stateTransitionOnTimeout`.
    pub timeout: i32,
    /// `stateFire`.
    pub fire: bool,
    /// `stateAllowImageChange`.
    pub allow_image_change: bool,
    /// `stateWaitForTimeout`.
    pub wait_for_timeout: bool,
    /// `stateTimeoutValue` (seconds).
    pub timeout_value: f32,
    /// `stateEnergyDrain` (per second).
    pub energy_drain: f32,
    /// `stateLoaded`: 0 leave `loaded`, 1 set it, anything else clear it.
    pub loaded_flag: i32,
    /// `stateRecoil`.
    pub recoil: i32,
    /// `stateScript` (the function name `executef` calls on the datablock; `None` = the pointer is NULL).
    pub script: Option<String>,
}

impl Default for ImageState {
    /// The array defaults: no transitions, no timeout, nothing set.
    fn default() -> Self {
        ImageState {
            name: String::new(),
            loaded: [-1; 2],
            ammo: [-1; 2],
            target: [-1; 2],
            trigger: [-1; 2],
            wet: [-1; 2],
            timeout: -1,
            fire: false,
            allow_image_change: false,
            wait_for_timeout: false,
            timeout_value: 0.0,
            energy_drain: 0.0,
            loaded_flag: 0,
            recoil: 0,
            script: None,
        }
    }
}

/// A state as the datablock's script fields hold it: transitions by state name (resolved by [`ImageData::from_raw`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RawState {
    /// `stateName[i]` (`None`: the slot is unused).
    pub name: Option<String>,
    pub on_loaded: Option<String>,
    pub on_not_loaded: Option<String>,
    pub on_ammo: Option<String>,
    pub on_no_ammo: Option<String>,
    pub on_target: Option<String>,
    pub on_no_target: Option<String>,
    pub on_wet: Option<String>,
    pub on_not_wet: Option<String>,
    pub on_trigger_up: Option<String>,
    pub on_trigger_down: Option<String>,
    pub on_timeout: Option<String>,
    pub timeout_value: f32,
    pub wait_for_timeout: bool,
    pub fire: bool,
    pub allow_image_change: bool,
    pub energy_drain: f32,
    pub loaded_flag: i32,
    pub recoil: i32,
    pub script: Option<String>,
}

/// The datablock: the states and the scalars.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageData {
    /// The `SimObject` id.
    pub id: DbId,
    /// The states, at most [`MAX_STATES`].
    pub states: Vec<ImageState>,
    /// The first state with `fire` set (-1 when none).
    pub fire_state: i32,
    /// `usesEnergy` and `minEnergy`.
    pub uses_energy: bool,
    pub min_energy: f32,
    /// `lightType`.
    pub light_type: u32,
    /// `correctMuzzleVector`.
    pub correct_muzzle_vector: bool,
    /// `useEyeOffset` (the preload's `!isIdentity(eyeOffset)`; not the `firstPerson` field): with the
    /// owner in first person the eye transform is used.
    pub first_person: bool,
    /// `mountPoint`.
    pub mount_point: u32,
    /// `offset` (the mount transform).
    pub mount_transform: Matrix,
    /// `eyeOffset`.
    pub eye_offset: Matrix,
    /// `muzzleNode` (-1 none) and its transform in the image shape (`nodeTransforms[node]` of the instance; `None` when the
    /// shape has no such node).
    pub muzzle_node: i32,
    pub muzzle_node_transform: Option<Matrix>,
}

impl ImageData {
    /// A datablock with no states and the constructor's defaults.
    pub fn new(id: DbId) -> ImageData {
        ImageData {
            id,
            states: Vec::new(),
            fire_state: -1,
            uses_energy: false,
            min_energy: 0.0,
            light_type: 0,
            correct_muzzle_vector: false,
            first_person: false,
            mount_point: 0,
            mount_transform: IDENTITY,
            eye_offset: IDENTITY,
            muzzle_node: -1,
            muzzle_node_transform: None,
        }
    }

    /// A transition name to a state index. `NULL` or empty is -1; else the first of the 31 names that matches without
    /// regard to case (an unused slot never matches); no match is an error and 0.
    fn resolve(
        raw: &[RawState],
        name: &Option<String>,
        errors: &mut Vec<String>,
        image: DbId,
    ) -> i32 {
        let Some(n) = name.as_deref().filter(|n| !n.is_empty()) else {
            return -1;
        };
        for (i, r) in raw.iter().take(MAX_STATES).enumerate() {
            if let Some(s) = &r.name
                && s.eq_ignore_ascii_case(n)
            {
                return i as i32;
            }
        }
        errors.push(format!(
            "ShapeBaseImageData:: Could not resolve state \"{n}\" for image \"{image}\""
        ));
        0
    }

    /// The states of `raw` as `onAdd` builds them, and the error messages it prints. Only the first [`MAX_STATES`] count.
    pub fn from_raw(id: DbId, raw: &[RawState]) -> (ImageData, Vec<String>) {
        let mut errors = Vec::new();
        let mut d = ImageData::new(id);
        for r in raw.iter().take(MAX_STATES) {
            let mut res = |n: &Option<String>| Self::resolve(raw, n, &mut errors, id);
            d.states.push(ImageState {
                name: r.name.clone().unwrap_or_default(),
                loaded: [res(&r.on_not_loaded), res(&r.on_loaded)],
                ammo: [res(&r.on_no_ammo), res(&r.on_ammo)],
                target: [res(&r.on_no_target), res(&r.on_target)],
                trigger: [res(&r.on_trigger_up), res(&r.on_trigger_down)],
                wet: [res(&r.on_not_wet), res(&r.on_wet)],
                timeout: res(&r.on_timeout),
                fire: r.fire,
                allow_image_change: r.allow_image_change,
                wait_for_timeout: r.wait_for_timeout,
                timeout_value: r.timeout_value,
                energy_drain: r.energy_drain,
                loaded_flag: r.loaded_flag,
                recoil: r.recoil,
                script: r.script.clone(),
            });
        }
        d.finish();
        (d, errors)
    }

    /// Makes the table safe to run and computes `fireState`: at most [`MAX_STATES`] states; a transition outside the table is
    /// "none" (only the 31 are resolved, so a sane datablock has none); `fireState` is the first state with `fire` set.
    pub fn finish(&mut self) {
        self.states.truncate(MAX_STATES);
        let n = self.states.len() as i32;
        let fix = |t: &mut i32| {
            if *t < 0 || *t >= n {
                *t = -1;
            }
        };
        for s in &mut self.states {
            for t in s
                .loaded
                .iter_mut()
                .chain(s.ammo.iter_mut())
                .chain(s.target.iter_mut())
                .chain(s.trigger.iter_mut())
                .chain(s.wet.iter_mut())
            {
                fix(t);
            }
            fix(&mut s.timeout);
        }
        self.fire_state = self
            .states
            .iter()
            .position(|s| s.fire)
            .map_or(-1, |i| i as i32);
    }

    pub fn state(&self, index: usize) -> Option<&ImageState> {
        self.states.get(index)
    }
}
