//! The `alx` console functions over this crate's [`Audio`] layer, as TGE 1.4 registers them (`audio/audioFunctions.cc`):
//! names, argument counts, usage lines and error lines; the client side of `AudioProfile` (the preload of `onAdd`);
//! the sounds a GUI asks for ([`play_profile`]); and the clock ([`tick`]). The datablock classes they read are
//! [`AUDIO_CLASSES`] (a host whose own table declares them keeps its own).
//!
//! **The device.** `OpenALInitDriver()` always succeeds: the driver is this crate's mixer. Without a sound card
//! ([`AudioHost::device`] is None) the voices move on the simulation clock (the null sink), so `alxIsPlaying` and
//! `alxGetWaveLen` answer as they would with sounds playing. A window client may open a `cpal` device
//! (`AudioHost::open_device`, feature `device`). `alGetString` answers [`AudioHost::strings`] (a host may set its own), or,
//! once the host has an [`AudioReport`] ([`AudioHost::report`]: set when it opens its sound card, or when it has none), what is
//! really there ([`AudioReport::strings`]: the mixer, the system's audio layer, the output device, no extensions).
//!
//! **The clock** ([`tick`], each pass of the host's loop): without a device the voices advance by the simulation time since the
//! last pass; `alxUpdate` runs when due ([`Audio::tick`]).
//!
//! **Arguments** are read as TGE reads them: `atoi` for handles and channels (`0` is "no handle"), `atof` for values, `"x y z"`
//! through `sscanf("%g %g %g")`; an enum name not in the function's mask prints the function's own error line (TGE's words,
//! some of them naming another function).
//!
//! **The host's state.** The natives keep no state of their own: the [`AudioHost`] lives in the host's state `S`, reached through
//! an [`Access`] function, and the game's differences from TGE's registration come in an [`AlxProfile`].
//!
//! Credit: the console functions follow TGE 1.4's `audio/audioFunctions.cc` and the datablocks' fields its
//! `audio/audioDataBlock.cc` (each `Copyright (C) GarageGames.com, Inc.`); Torque3D (MIT) replaced this layer with SFX. The
//! code is this crate's own.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use torque_game::dbvalues::fields::{bool_of, f32_of, i32_of, object_of, text_of, vec3_of};
use ts_vm::classes::{ClassSpec, EngineClass, FieldSpec, install_spec};
use ts_vm::fields::FieldType;
use ts_vm::host::{FileRead, FileStat};
use ts_vm::sim::ObjId;
use ts_vm::vm::{Native, NativeCall, Vm, VmError};

use crate::al;
use crate::{Audio, Buffer, BufferCache, Description, Found, SoundFiles};

/// What `alGetString` answers by default (vendor, version, renderer, extensions): this crate's mixer.
pub const STRINGS: [&str; 4] = ["torque-audio", env!("CARGO_PKG_VERSION"), "Software", ""];
/// The most profiles whose preloaded buffer is kept.
pub const MAX_PRELOADED: usize = 8_192;

/// How the natives reach the [`AudioHost`] inside the host's state `S`.
pub type Access<S> = fn(&mut S) -> &mut AudioHost;

/// What a game changes in the registration: `alGetString`'s strings are [`AudioHost::strings`]; these are the rest.
#[derive(Clone)]
pub struct AlxProfile {
    /// The `AudioProfile` class behaviour the client's preload runs after (a server's checks of `onAdd`), if any.
    pub profile_base: Option<Rc<dyn EngineClass>>,
}

impl AlxProfile {
    /// TGE's registration.
    pub fn tge() -> AlxProfile {
        AlxProfile { profile_base: None }
    }
}

const fn field(name: &'static str, ty: FieldType) -> FieldSpec {
    FieldSpec {
        name,
        ty,
        count: 1,
        offset: None,
        validator: None,
    }
}

/// The datablock classes the natives read, as TGE 1.4's `audioDataBlock.cc` declares their fields and its constructors
/// set them.
pub static AUDIO_CLASSES: &[ClassSpec] = &[
    ClassSpec {
        name: "AudioDescription",
        parent: Some("SimDataBlock"),
        sim_object: true,
        inherits_fields: true,
        fields: &[
            field("volume", FieldType::F32),
            field("isLooping", FieldType::Bool),
            field("isStreaming", FieldType::Bool),
            field("is3D", FieldType::Bool),
            field("referenceDistance", FieldType::F32),
            field("maxDistance", FieldType::F32),
            field("coneInsideAngle", FieldType::S32),
            field("coneOutsideAngle", FieldType::S32),
            field("coneOutsideVolume", FieldType::F32),
            field("coneVector", FieldType::Point3F),
            field("environmentLevel", FieldType::F32),
            field("loopCount", FieldType::S32),
            field("minLoopGap", FieldType::S32),
            field("maxLoopGap", FieldType::S32),
            field("type", FieldType::S32),
        ],
        methods: &[],
        // `AudioDescription::AudioDescription`'s values.
        defaults: Some(&[
            ("volume", "1"),
            ("referenceDistance", "1"),
            ("maxDistance", "100"),
            ("coneInsideAngle", "360"),
            ("coneOutsideAngle", "360"),
            ("coneOutsideVolume", "1"),
            ("coneVector", "0 0 1"),
            ("loopCount", "-1"),
        ]),
    },
    ClassSpec {
        name: "AudioProfile",
        parent: Some("SimDataBlock"),
        sim_object: true,
        inherits_fields: true,
        fields: &[
            field("filename", FieldType::Filename),
            field("description", FieldType::DataBlock("AudioDescription")),
            field(
                "environment",
                FieldType::DataBlock("AudioSampleEnvironment"),
            ),
            field("preload", FieldType::Bool),
        ],
        methods: &[],
        defaults: None,
    },
];

/// Registers every class of [`AUDIO_CLASSES`] that `vm` does not have yet (by name); returns how many it added.
pub fn install_classes(vm: &mut Vm) -> usize {
    let mut added = 0;
    for spec in AUDIO_CLASSES {
        if vm.objects().classes.find(spec.name.as_bytes()).is_some() {
            continue;
        }
        added += usize::from(install_spec(vm, spec));
    }
    added
}

/// The client's audio: the `alx` layer (shared with a device's callback), the buffers, the profiles' preloaded buffers.
pub struct AudioHost {
    pub core: Arc<Mutex<Audio>>,
    pub cache: BufferCache,
    /// The buffer `onAdd`'s preload found, by profile (at most [`MAX_PRELOADED`]; the buffers are the cache's, inside its memory
    /// budget), and the preloads past the cap (their profiles read as unloaded ones do).
    pub preloaded: HashMap<ObjId, Found>,
    pub preloads_dropped: u64,
    /// The sound card's name when one plays (a window client); None: the null sink.
    pub device: Option<String>,
    /// What `alGetString` reports once the host knows what it plays through (None: [`AudioHost::strings`]).
    pub report: Option<AudioReport>,
    /// `alGetString`'s vendor, version, renderer and extensions without a report ([`STRINGS`] by default).
    pub strings: [String; 4],
    /// The mixer a report names as its vendor (`"<mixer>'s own mixer"`).
    pub mixer: String,
    #[cfg(feature = "device")]
    stream: Option<crate::device::Device>,
    last_ms: Option<u32>,
}

impl Default for AudioHost {
    fn default() -> AudioHost {
        AudioHost::new(Audio::default(), BufferCache::default())
    }
}

impl AudioHost {
    /// The host over `core` (its volume curve and uncull gain the game's) and `cache` (its WAV rules the game's).
    pub fn new(core: Audio, cache: BufferCache) -> AudioHost {
        AudioHost {
            core: Arc::new(Mutex::new(core)),
            cache,
            preloaded: HashMap::new(),
            preloads_dropped: 0,
            device: None,
            report: None,
            strings: STRINGS.map(str::to_string),
            mixer: STRINGS[0].to_string(),
            #[cfg(feature = "device")]
            stream: None,
            last_ms: None,
        }
    }

    /// The `alx` layer, locked.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Audio> {
        self.core.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Opens the default sound card (a window client only): the device's callback moves the voices from now on. Err: why
    /// none could be opened; the host then stays on the null sink ([`AudioHost::no_device`]), so the game plays on, silent.
    #[cfg(feature = "device")]
    pub fn open_device(&mut self) -> Result<String, String> {
        let d = match crate::device::Device::open(self.core.clone()) {
            Ok(d) => d,
            Err(why) => {
                self.no_device();
                return Err(why);
            }
        };
        let name = d.name.clone();
        self.device = Some(name.clone());
        self.report = Some(AudioReport {
            backend: d.backend.clone(),
            device: Some((d.name.clone(), d.rate, d.channels)),
        });
        self.stream = Some(d);
        Ok(name)
    }

    /// The null sink, known: no sound card plays (none was found, or opening it failed). The voices keep moving on the
    /// host's clock, `alxIsPlaying` and the rest answer as with sound, and `alGetString` reports no output device.
    pub fn no_device(&mut self) {
        self.device = None;
        self.report = Some(AudioReport::no_device());
        #[cfg(feature = "device")]
        {
            self.stream = None;
        }
    }
}

/// What a window client really plays through, for `alGetString` and an options dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioReport {
    /// The system's audio layer ("WASAPI", ...); empty when there is no sound card.
    pub backend: String,
    /// The output device: its name, rate and channels; None when there is no sound card (the voices run on the clock, silent).
    pub device: Option<(String, u32, u16)>,
}

/// The cpal version the output is built on (this crate's dependency).
pub const CPAL: &str = "cpal 0.15";

/// The longest device name reported.
const MAX_REPORTED: usize = 120;

impl AudioReport {
    /// A window client without a sound card.
    pub fn no_device() -> AudioReport {
        AudioReport {
            backend: String::new(),
            device: None,
        }
    }

    /// The driver's name an options dialog lists: the audio layer, else "No sound card".
    pub fn driver(&self) -> String {
        if self.backend.is_empty() || self.device.is_none() {
            "No sound card".to_string()
        } else {
            self.backend.clone()
        }
    }

    /// `alGetString`'s vendor, version, renderer and extensions, the vendor naming `mixer`.
    pub fn strings(&self, mixer: &str) -> [String; 4] {
        let clean = |s: &str| -> String {
            s.chars()
                .filter(|c| !c.is_control())
                .take(MAX_REPORTED)
                .collect()
        };
        let renderer = match &self.device {
            Some((name, rate, channels)) => format!(
                "{}, {rate} Hz, {channels} {}",
                clean(name),
                if *channels == 1 {
                    "channel"
                } else {
                    "channels"
                }
            ),
            None => "No output device (silent)".to_string(),
        };
        let version = if self.backend.is_empty() {
            CPAL.to_string()
        } else {
            format!("{} through {CPAL}", clean(&self.backend))
        };
        [
            format!("{mixer}'s own mixer"),
            version,
            renderer,
            "None".to_string(),
        ]
    }
}

/// The host's files through the `Vm`'s file host (the resource manager's view).
struct VmFiles<'a>(&'a Vm);

impl SoundFiles for VmFiles<'_> {
    fn read(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let host = self.0.file_host();
        if let FileStat::Found { len } = host.stat(name)
            && len > crate::MAX_SOUND_FILE_BYTES as u64
        {
            return Some(Vec::new());
        }
        match host.read(name) {
            FileRead::Found(b) => Some(b),
            _ => None,
        }
    }

    /// A byte read or decoded is one unit of the script's step budget (`Vm::charge_work`).
    fn charge(&mut self, units: u64) -> bool {
        self.0.charge_work(units)
    }
}

fn atoi(b: &[u8]) -> u32 {
    ts_text::atoi(b) as u32
}

fn atof(b: &[u8]) -> f32 {
    ts_text::atof(b) as f32
}

fn xyz(text: &[u8]) -> [f32; 3] {
    let mut v = [0f32; 3];
    ts_text::scan_floats(text, &mut v);
    v
}

fn class_is(vm: &Vm, id: ObjId, class: &str) -> bool {
    let objects = vm.objects();
    match (objects.get(id), objects.classes.find(class.as_bytes())) {
        (Some(o), Some(c)) => objects.classes.is_a(o.class(), c),
        _ => false,
    }
}

/// `Sim::findObject(name)` cast to `class`.
fn find_as(vm: &Vm, name: &[u8], class: &str) -> Option<ObjId> {
    vm.objects()
        .find(name)
        .filter(|id| class_is(vm, *id, class))
}

/// An `AudioDescription`'s `Audio::Description` (its fields, as `onAdd` left them).
pub fn description(vm: &Vm, id: ObjId) -> Description {
    Description {
        volume: f32_of(vm, id, "volume", 0),
        is_looping: bool_of(vm, id, "isLooping", 0),
        is_streaming: bool_of(vm, id, "isStreaming", 0),
        is_3d: bool_of(vm, id, "is3D", 0),
        reference_distance: f32_of(vm, id, "ReferenceDistance", 0),
        max_distance: f32_of(vm, id, "maxDistance", 0),
        cone_inside_angle: i32_of(vm, id, "coneInsideAngle", 0),
        cone_outside_angle: i32_of(vm, id, "coneOutsideAngle", 0),
        cone_outside_volume: f32_of(vm, id, "coneOutsideVolume", 0),
        cone_vector: vec3_of(vm, id, "coneVector"),
        environment_level: f32_of(vm, id, "environmentLevel", 0),
        loop_count: i32_of(vm, id, "loopCount", 0),
        min_loop_gap: i32_of(vm, id, "minLoopGap", 0),
        max_loop_gap: i32_of(vm, id, "maxLoopGap", 0),
        kind: i32_of(vm, id, "type", 0),
    }
}

/// An `AudioProfile`'s description and file name, when it has a description.
fn profile_parts(vm: &Vm, profile: ObjId) -> Option<(Description, Vec<u8>)> {
    let desc = object_of(vm, profile, "description", 0)
        .filter(|d| class_is(vm, *d, "AudioDescription"))?;
    let file = text_of(vm, profile, "fileName", 0)
        .map(|t| t.to_vec())
        .unwrap_or_default();
    Some((description(vm, desc), file))
}

/// Prints the buffer reads' lines (the `smpl` error) left in the cache.
fn flush_messages(vm: &mut Vm, host: &mut AudioHost) {
    for (error, text) in host.cache.messages.drain(..) {
        if error {
            vm.error(text);
        } else {
            vm.print(text.as_bytes());
        }
    }
}

/// The buffer of `name` (None: no file or no buffer); the step limit when the budget cannot pay for the read.
fn load(vm: &mut Vm, host: &mut AudioHost, name: &[u8]) -> Result<Option<Arc<Buffer>>, VmError> {
    let found = host.cache.find(&mut VmFiles(vm), name);
    flush_messages(vm, host);
    match found {
        Found::Loaded(b) => Ok(Some(b)),
        Found::OverBudget => Err(VmError::StepLimit),
        Found::Missing | Found::Failed(_) => Ok(None),
    }
}

/// `alxCreateSource(desc, file, position)`: the buffer found and the source made.
fn create(
    vm: &mut Vm,
    host: &mut AudioHost,
    desc: &Description,
    file: &[u8],
    position: Option<[f32; 3]>,
) -> Result<u32, VmError> {
    if !host.lock().initialized || file.is_empty() {
        return Ok(0);
    }
    let buffer = load(vm, host, file)?;
    Ok(host.lock().create_source(desc, buffer, position, 0))
}

fn handle(r: Result<u32, VmError>) -> Native {
    match r {
        Ok(h) => Native::Int(h as i32),
        Err(e) => Native::Fail(e),
    }
}

/// `alxPlay(alxCreateSource(profile))`, as a GUI plays a control's sound (a button's, a text edit's denied sound): nothing for
/// a profile without a description. Returns the handle.
pub fn play_profile<S>(
    vm: &mut Vm,
    state: &Rc<RefCell<S>>,
    access: Access<S>,
    profile: ObjId,
) -> u32 {
    if !class_is(vm, profile, "AudioProfile") {
        return 0;
    }
    let Some((desc, file)) = profile_parts(vm, profile) else {
        return 0;
    };
    let Ok(mut s) = state.try_borrow_mut() else {
        return 0;
    };
    let host = access(&mut s);
    // Out of budget: nothing plays (the run that asked ends at its step limit).
    let h = create(vm, host, &desc, &file, None).unwrap_or(0);
    host.lock().play(h)
}

/// `alxPlay(alxCreateSource(profile, transform))` at `position` (None: unpositioned), as the world's sounds play: the handle
/// (0: nothing plays) and whether its description loops. `description_id` is the profile's description as the datablock's
/// record names it: a client datablock's pointer fields print "" (it is unnamed), so the object cannot be found from its field
/// text; TGE's client holds the pointer `unpackData` resolved.
pub fn play_profile_at<S>(
    vm: &mut Vm,
    state: &Rc<RefCell<S>>,
    access: Access<S>,
    profile: ObjId,
    description_id: Option<ObjId>,
    position: Option<[f32; 3]>,
) -> (u32, bool) {
    if !class_is(vm, profile, "AudioProfile") {
        return (0, false);
    }
    let parts = match description_id.filter(|d| class_is(vm, *d, "AudioDescription")) {
        Some(d) => Some((
            description(vm, d),
            text_of(vm, profile, "fileName", 0)
                .map(|t| t.to_vec())
                .unwrap_or_default(),
        )),
        None => profile_parts(vm, profile),
    };
    let Some((desc, file)) = parts else {
        return (0, false);
    };
    let Ok(mut s) = state.try_borrow_mut() else {
        return (0, false);
    };
    let host = access(&mut s);
    let h = create(vm, host, &desc, &file, position).unwrap_or(0);
    (host.lock().play(h), desc.is_looping)
}

/// One pass of the host loop: the voices moved by the simulation time since the last pass (no device), then `alxUpdate` when due.
pub fn tick<S>(vm: &Vm, state: &Rc<RefCell<S>>, access: Access<S>) {
    let now = vm.sim_time();
    let mut s = state.borrow_mut();
    let host = access(&mut s);
    let last = host.last_ms.replace(now).unwrap_or(now);
    let device = host.device.is_some();
    let mut a = host.lock();
    if !device {
        a.advance(f64::from(now.wrapping_sub(last)));
    }
    a.tick(now);
}

fn float(x: f32) -> Native {
    Native::Float(f64::from(x))
}

/// TGE's usage lines.
mod usage {
    pub const INIT: &str =
        "Initializes the OpenAL driver.\n\n@note You must call this before any sounds will work!";
    pub const GET_STRING: &str = "(string item)\n\nThis wraps alGetString().";
    pub const WAVE_LEN: &str = "(string filename)\n\n@param filename File to determine length of.\n@returns Length in milliseconds.";
    pub const CREATE: &str = "(profile) or (profile, x,y,z) or (description, filename) or (description, filename, x,y,z)";
    pub const SET: &str = "(handle, ALenum, value)";
    pub const SET3: &str = "(handle, ALenum, x, y, z)\n\n@note You can replace the last three parameters with a string, \"x y z\"";
    pub const GET: &str = "(handle, ALenum)";
    pub const PLAY: &str = "alxPlay(handle) or alxPlay(profile) or alxPlay(profile, x,y,z)";
    pub const IS_PLAYING: &str = "alxIsPlaying(handle)";
    pub const LISTENER: &str = "alxListener(ALenum, value)";
    pub const LISTENER3: &str = "alListener3f(ALenum, \"x y z\") or alListener3f(ALenum, x, y, z)";
    pub const GET_CHANNEL: &str = "(int channel_id)\n\n@param  channel_id  ID of channel to fetch volume from.\n@return Volume of channel.";
    pub const SET_CHANNEL: &str = "(int channel_id, float volume)\n\n@param channel_id  ID of channel to set volume on.\n@param volume      New volume of channel, from 0.0f-1.0f";
}

/// A native's argument `i` (empty past the last).
fn arg(c: &NativeCall, i: usize) -> &[u8] {
    c.argv.get(i).map_or(&[][..], Vec::as_slice)
}

/// Registers the `alx` functions and the client side of `AudioProfile` over the [`AudioHost`] in `state` (reached through
/// `access`), with `profile`'s differences from TGE's registration.
pub fn register<S: 'static>(
    vm: &mut Vm,
    state: &Rc<RefCell<S>>,
    access: Access<S>,
    profile: AlxProfile,
) {
    let s = Rc::clone(state);
    let reg = |vm: &mut Vm,
               name: &str,
               min: i32,
               max: i32,
               usage: &str,
               f: fn(&mut Vm, &NativeCall, &mut AudioHost) -> Native| {
        let s = Rc::clone(&s);
        vm.register(name, min, max, usage, move |vm, c| {
            let mut st = s.borrow_mut();
            f(vm, c, access(&mut st))
        });
    };
    reg(vm, "OpenALInitDriver", 1, 1, usage::INIT, |_, _, h| {
        Native::Bool(h.lock().init())
    });
    reg(
        vm,
        "OpenALShutdownDriver",
        1,
        1,
        "OpenALShutdownDriver()",
        |_, _, h| {
            h.lock().shutdown();
            Native::Void
        },
    );
    reg(
        vm,
        "OpenALRegisterExtensions",
        1,
        1,
        "OpenALRegisterExtensions()",
        |_, _, _| Native::Void,
    );
    reg(vm, "alGetString", 2, 2, usage::GET_STRING, |vm, c, h| {
        let Some(e) = al::lookup(arg(c, 1), al::MASK_GET_STRING) else {
            vm.error(format!(
                "alGetString: invalid enum name '{}'",
                String::from_utf8_lossy(arg(c, 1))
            ));
            return Native::Str(Vec::new());
        };
        if !h.lock().initialized {
            return Native::Str(Vec::new());
        }
        if let Some(r) = &h.report {
            let [vendor, version, renderer, extensions] = r.strings(&h.mixer);
            let text = match e {
                al::AL_VENDOR => vendor,
                al::AL_VERSION => version,
                al::AL_RENDERER => renderer,
                al::AL_EXTENSIONS => extensions,
                _ => String::new(),
            };
            return Native::Str(text.into_bytes());
        }
        let text = match e {
            al::AL_VENDOR => h.strings[0].as_str(),
            al::AL_VERSION => h.strings[1].as_str(),
            al::AL_RENDERER => h.strings[2].as_str(),
            al::AL_EXTENSIONS => h.strings[3].as_str(),
            _ => "",
        };
        Native::Str(text.as_bytes().to_vec())
    });
    reg(vm, "alxGetWaveLen", 2, 2, usage::WAVE_LEN, |vm, c, h| {
        let found = h.cache.find(&mut VmFiles(vm), arg(c, 1));
        flush_messages(vm, h);
        match found {
            Found::Missing => Native::Int(0),
            Found::Failed(_) => {
                vm.error("alxGetWaveLen: invalid buffer".to_string());
                Native::Int(0)
            }
            Found::Loaded(b) => Native::Int(b.wave_len_ms()),
            Found::OverBudget => Native::Fail(VmError::StepLimit),
        }
    });
    reg(vm, "alxCreateSource", 2, 6, usage::CREATE, |vm, c, h| {
        let name = arg(c, 1);
        if let Some(p) = find_as(vm, name, "AudioProfile") {
            let Some((desc, file)) = profile_parts(vm, p) else {
                return Native::Int(0);
            };
            let position =
                (c.argv.len() != 2).then(|| [atof(arg(c, 2)), atof(arg(c, 3)), atof(arg(c, 4))]);
            return handle(create(vm, h, &desc, &file, position));
        }
        let Some(d) = find_as(vm, name, "AudioDescription") else {
            vm.print(
                format!(
                    "Unable to locate audio profile/description '{}'",
                    String::from_utf8_lossy(name)
                )
                .as_bytes(),
            );
            return Native::Int(0);
        };
        let desc = description(vm, d);
        let file = arg(c, 2).to_vec();
        let position =
            (c.argv.len() != 3).then(|| [atof(arg(c, 3)), atof(arg(c, 4)), atof(arg(c, 5))]);
        handle(create(vm, h, &desc, &file, position))
    });
    reg(vm, "alxPlay", 2, 5, usage::PLAY, |vm, c, h| {
        if c.argv.len() == 2 {
            let handle = atoi(arg(c, 1));
            if handle != 0 {
                return Native::Int(h.lock().play(handle) as i32);
            }
        }
        let Some(p) = find_as(vm, arg(c, 1), "AudioProfile") else {
            vm.print(
                format!(
                    "Unable to locate audio profile '{}'",
                    String::from_utf8_lossy(arg(c, 1))
                )
                .as_bytes(),
            );
            return Native::Int(0);
        };
        let position = match c.argv.len() {
            3 => Some(xyz(arg(c, 2))),
            5 => Some([atof(arg(c, 2)), atof(arg(c, 3)), atof(arg(c, 4))]),
            // TGE passes a transform in every form, the bare profile's and four arguments' at (0, 0, 0): the source is positioned.
            _ => Some([0.0; 3]),
        };
        let Some((desc, file)) = profile_parts(vm, p) else {
            return Native::Int(0);
        };
        let hnd = match create(vm, h, &desc, &file, position) {
            Ok(hnd) => hnd,
            Err(e) => return Native::Fail(e),
        };
        if hnd == 0 {
            return Native::Int(0);
        }
        Native::Int(h.lock().play(hnd) as i32)
    });
    reg(vm, "alxStop", 2, 2, "(int handle)", |_, c, h| {
        let handle = atoi(arg(c, 1));
        if handle != 0 {
            h.lock().stop(handle);
        }
        Native::Void
    });
    reg(vm, "alxStopAll", 1, 1, "()", |_, _, h| {
        h.lock().stop_all();
        Native::Void
    });
    reg(
        vm,
        "alxGetStreamPosition",
        2,
        2,
        "alxGetStreamPosition(handle)",
        |_, c, h| {
            let handle = atoi(arg(c, 1));
            float(if handle == 0 {
                -1.0
            } else {
                h.lock().stream_times(handle).map_or(-1.0, |t| t.0)
            })
        },
    );
    reg(
        vm,
        "alxGetStreamDuration",
        2,
        2,
        "alxGetStreamDuration(handle)",
        |_, c, h| {
            let handle = atoi(arg(c, 1));
            float(if handle == 0 {
                -1.0
            } else {
                h.lock().stream_times(handle).map_or(-1.0, |t| t.1)
            })
        },
    );
    reg(vm, "alxIsPlaying", 2, 5, usage::IS_PLAYING, |_, c, h| {
        let handle = atoi(arg(c, 1));
        Native::Bool(handle != 0 && h.lock().is_playing(handle))
    });
    reg(vm, "alxListenerf", 3, 3, usage::LISTENER, |vm, c, h| {
        let Some(e) = al::lookup(arg(c, 1), al::MASK_LISTENER_F) else {
            vm.error(format!(
                "alxListenerf: invalid enum name '{}'",
                String::from_utf8_lossy(arg(c, 1))
            ));
            return Native::Void;
        };
        h.lock().listener_f(e, atof(arg(c, 2)));
        Native::Void
    });
    reg(
        vm,
        "alxGetListenerf",
        2,
        2,
        "alxGetListenerf(Alenum)",
        |vm, c, h| {
            let Some(e) = al::lookup(arg(c, 1), al::MASK_GET_LISTENER_F) else {
                vm.error(format!(
                    "alxGetListenerf: invalid enum name '{}'",
                    String::from_utf8_lossy(arg(c, 1))
                ));
                return float(0.0);
            };
            float(h.lock().get_listener_f(e))
        },
    );
    reg(vm, "alListener3f", 3, 5, usage::LISTENER3, |vm, c, h| {
        let Some(e) = al::lookup(arg(c, 1), al::MASK_LISTENER_3F) else {
            vm.error(format!(
                "alListener3f: invalid enum name '{}'",
                String::from_utf8_lossy(arg(c, 1))
            ));
            return Native::Void;
        };
        let v = match c.argv.len() {
            3 => xyz(arg(c, 2)),
            5 => [atof(arg(c, 2)), atof(arg(c, 3)), atof(arg(c, 4))],
            _ => {
                vm.error("alListener3f: wrong number of arguments".to_string());
                return Native::Void;
            }
        };
        h.lock().listener_3f(e, v);
        Native::Void
    });
    reg(
        vm,
        "alGetListener3f",
        2,
        2,
        "alGetListener3f(Alenum)",
        |vm, c, _| {
            if al::lookup(arg(c, 1), al::MASK_GET_LISTENER_3F).is_none() {
                vm.error(format!(
                    "alGetListener3f: invalid enum name '{}'",
                    String::from_utf8_lossy(arg(c, 1))
                ));
                return Native::Str(b"0 0 0".to_vec());
            }
            // Only scalar names pass the mask: OpenAL refuses them and the three values stay as they were. Here: zeros, in TGE's
            // format "%7.3f %7.3 %7.3" (two conversions without a type).
            Native::Str(format!("{:7.3}  ", 0.0f32).into_bytes())
        },
    );
    reg(
        vm,
        "alGetListeneri",
        2,
        2,
        "alGetListeneri(Alenum)",
        |vm, c, _| {
            if al::lookup(arg(c, 1), al::MASK_GET_LISTENER_I).is_none() {
                vm.error(format!(
                    "alGetListeneri: invalid enum name '{}'",
                    String::from_utf8_lossy(arg(c, 1))
                ));
            }
            Native::Int(0)
        },
    );
    reg(vm, "alxSourcef", 4, 4, usage::SET, |vm, c, h| {
        let Some(e) = al::lookup(arg(c, 2), al::MASK_SOURCE_F) else {
            vm.error(format!(
                "cAudio_alxSourcef: invalid enum name '{}'",
                String::from_utf8_lossy(arg(c, 2))
            ));
            return Native::Void;
        };
        h.lock().source_f(atoi(arg(c, 1)), e, atof(arg(c, 3)));
        Native::Void
    });
    reg(vm, "alxSource3f", 3, 6, usage::SET3, |vm, c, h| {
        let Some(e) = al::lookup(arg(c, 2), al::MASK_SOURCE_3F) else {
            vm.error(format!(
                "cAudio_alxSource3f: invalid enum name '{}'",
                String::from_utf8_lossy(arg(c, 2))
            ));
            return Native::Void;
        };
        let v = match c.argv.len() {
            4 => xyz(arg(c, 3)),
            6 => [atof(arg(c, 3)), atof(arg(c, 4)), atof(arg(c, 5))],
            _ => {
                vm.error("cAudio_alxSource3f: wrong number of args".to_string());
                return Native::Void;
            }
        };
        h.lock().source_3f(atoi(arg(c, 1)), e, v);
        Native::Void
    });
    reg(vm, "alxSourcei", 4, 4, usage::SET, |vm, c, h| {
        let Some(e) = al::lookup(arg(c, 2), al::MASK_SOURCE_I) else {
            vm.error(format!(
                "cAudio_alxSourcei: invalid enum name '{}'",
                String::from_utf8_lossy(arg(c, 2))
            ));
            return Native::Void;
        };
        h.lock()
            .source_i(atoi(arg(c, 1)), e, atoi(arg(c, 3)) as i32);
        Native::Void
    });
    reg(vm, "alxGetSourcef", 3, 3, usage::GET, |vm, c, h| {
        let Some(e) = al::lookup(arg(c, 2), al::MASK_GET_SOURCE_F) else {
            vm.error(format!(
                "cAudio_alxGetSourcef: invalid enum name '{}'",
                String::from_utf8_lossy(arg(c, 2))
            ));
            return float(0.0);
        };
        // An unknown handle: 0.
        float(h.lock().get_source_f(atoi(arg(c, 1)), e).unwrap_or(0.0))
    });
    reg(vm, "alxGetSource3f", 3, 3, usage::GET, |vm, c, _| {
        if al::lookup(arg(c, 2), al::MASK_GET_SOURCE_3F).is_none() {
            vm.error(format!(
                "cAudio_alxGetSource3f: invalid enum name '{}'",
                String::from_utf8_lossy(arg(c, 2))
            ));
            return Native::Str(b"0 0 0".to_vec());
        }
        Native::Str(format!("{:7.3}  ", 0.0f32).into_bytes())
    });
    reg(vm, "alxGetSourcei", 3, 3, usage::GET, |vm, c, h| {
        let Some(e) = al::lookup(arg(c, 2), al::MASK_GET_SOURCE_I) else {
            vm.error(format!(
                "cAudio_alxGetSourcei: invalid enum name '{}'",
                String::from_utf8_lossy(arg(c, 2))
            ));
            return Native::Int(0);
        };
        Native::Int(h.lock().get_source_i(atoi(arg(c, 1)), e).unwrap_or(0))
    });
    reg(
        vm,
        "alxGetChannelVolume",
        2,
        2,
        usage::GET_CHANNEL,
        |vm, c, h| {
            let ch = atoi(arg(c, 1));
            if ch > 31 {
                vm.error(format!(
                    "alxGetChannelVolume: invalid channel '{}'",
                    ch as i32
                ));
                return float(0.0);
            }
            float(h.lock().type_volume[ch as usize])
        },
    );
    reg(
        vm,
        "alxSetChannelVolume",
        3,
        3,
        usage::SET_CHANNEL,
        |vm, c, h| {
            let ch = atoi(arg(c, 1));
            if ch > 31 {
                vm.error(format!(
                    "alxSetChannelVolume: channel '{}' out of range [0, {}]",
                    ch as i32,
                    crate::system::NUM_TYPES
                ));
                return Native::Bool(false);
            }
            Native::Bool(h.lock().set_channel_volume(ch, atof(arg(c, 2))))
        },
    );
    vm.set_class_behaviour(
        "AudioProfile",
        Rc::new(ClientAudioProfile {
            state: Rc::clone(state),
            access,
            base: profile.profile_base,
        }),
    );
}

/// `AudioProfile::onAdd` on the client: the base's checks (a server's), then the preload: with a driver, a profile with
/// `preload` and a file name finds its file's resource ([`AudioHost::preloaded`]: the resource exists for a file that is there
/// even when it gives no buffer). No message here.
struct ClientAudioProfile<S> {
    state: Rc<RefCell<S>>,
    access: Access<S>,
    base: Option<Rc<dyn EngineClass>>,
}

impl<S: 'static> EngineClass for ClientAudioProfile<S> {
    fn on_add(&self, vm: &mut Vm, id: ObjId) -> Result<bool, VmError> {
        if let Some(base) = &self.base
            && !base.on_add(vm, id)?
        {
            return Ok(false);
        }
        let file = text_of(vm, id, "fileName", 0)
            .map(|t| t.to_vec())
            .unwrap_or_default();
        if bool_of(vm, id, "preload", 0) && !file.is_empty() {
            let mut s = self.state.borrow_mut();
            let host = (self.access)(&mut s);
            if host.lock().initialized {
                let b = host.cache.find(&mut VmFiles(vm), &file);
                flush_messages(vm, host);
                if matches!(b, Found::OverBudget) {
                    return Err(VmError::StepLimit);
                }
                if host.preloaded.len() < MAX_PRELOADED {
                    host.preloaded.insert(id, b);
                } else {
                    host.preloads_dropped += 1;
                }
            }
        }
        Ok(true)
    }

    fn on_remove(&self, vm: &mut Vm, id: ObjId) -> Result<(), VmError> {
        (self.access)(&mut self.state.borrow_mut())
            .preloaded
            .remove(&id);
        match &self.base {
            Some(base) => base.on_remove(vm, id),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
#[path = "natives_tests.rs"]
mod tests;
