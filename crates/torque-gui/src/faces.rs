//! TrueType faces a script registers from the game's files: `registerFont(face, path)` reads the `.ttf` at `path` through the file
//! host (so a zip's file counts, and the host's rules apply), checks it ([`torque_fonts::ttf::TtfFace::parse`]) and registers it
//! under `face` with the font source ([`FontSource::register_face`]), which looks a registered face up before the font cache
//! files. The layout's fonts take it at once; each draw cache takes it up before its next frame (`sync`), so every one of them sees
//! it, unless its own fonts (their folder or fallback, which need not be the layout's) have a face of that name: then it keeps its
//! own and one console line says so. A face is registered for the session.
//!
//! What is refused, each with a console line `registerFont: <face> (<path>): <why>` and the answer 0 ([`Refused`]):
//! - a `face` that is empty, longer than [`MAX_FACE_BYTES`], or holds a byte outside `[0-9A-Za-z _-]` (or starts or ends with a
//!   space): the name goes into the cache's file names and the profiles' `fontType`;
//! - a `face` already in use, in any case, so that a mod's file never replaces the game's own font: one of the client script
//!   layer's default faces ([`crate::client_script::DEFAULT_FACES`]), one a `GuiControlProfile`'s `fontType` names (drawn yet or
//!   not), one the layout's fonts found, one a cache file of the font folder names, one the font source's fallback would make
//!   ([`torque_fonts::Fallback::resolves`]: the system's faces, the browser's fonts), or one registered already. A profile made after a mod registered
//!   a face can use it (mods load after the game's own scripts), and a face no profile names yet is free until one does;
//! - a `path` that is not a `.ttf` file below the game's folder: relative, `/` separators, no empty, `.` or `..` part (so no
//!   `//host/share` or leading `/`), no drive, backslash or control byte, no part named as a Windows device (`con`, `nul`,
//!   `com1`, ...), at most [`MAX_PATH_BYTES`];
//! - more than [`torque_fonts::faces::MAX_REGISTERED`] faces;
//! - a missing file, one above [`torque_fonts::ttf::MAX_TTF_BYTES`], or one the check or the parser refuses;
//! - more than [`MAX_ATTEMPTS`] calls (a refused file costs a whole check, so a script cannot loop on one).

use std::fmt;

use torque_fonts::faces::MAX_REGISTERED;
use torque_fonts::ttf::{MAX_TTF_BYTES, SharedFace, TtfFace};
use torque_ts_vm::host::{FileRead, FileStat};
use torque_ts_vm::vm::{Native, Vm};

use crate::Gui;
use crate::ext::FontSource;

/// The longest `path` taken.
pub const MAX_PATH_BYTES: usize = 255;
/// The longest `face` taken.
pub const MAX_FACE_BYTES: usize = 64;
/// The most `registerFont` calls one GUI answers.
pub const MAX_ATTEMPTS: u32 = 64;
/// What one outline point or curve segment the parser walks costs (`Vm::charge_work` units).
const PARSE_WORK: u64 = 8;
/// What one file name of the font folder's listing costs.
const LIST_WORK: u64 = 4;
/// The names Windows opens as devices, with or without an extension.
const DEVICES: [&str; 4] = ["con", "prn", "aux", "nul"];

/// Why a face was not registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The face name is empty, long, or has a character outside the rule.
    FaceName,
    /// The face name is one the game's fonts or a registered face has.
    FaceInUse,
    /// The path is not a `.ttf` file below the game's folder.
    Path,
    /// This many faces are registered already.
    TooManyFaces(usize),
    /// No such file (or the host refused it).
    NoFile,
    /// The file is this long, above the cap.
    TooLarge(u64),
    /// The check or the parser refused the file (what it said).
    NotAFont(String),
    /// The font source takes no registered faces.
    NotTaken,
    /// More than this many calls.
    Calls,
    /// The run's work budget is spent.
    Budget,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::FaceName => f.write_str(
                "the face name is empty, long, or has a character outside 0-9 A-Z a-z space _ -",
            ),
            Refused::FaceInUse => f.write_str("the face name is in use already"),
            Refused::Path => f.write_str("the path is not a .ttf file in the game's folders"),
            Refused::TooManyFaces(n) => write!(f, "{n} faces are registered already"),
            Refused::NoFile => f.write_str("no such file"),
            Refused::TooLarge(len) => {
                write!(f, "the file is {len} bytes (at most {MAX_TTF_BYTES})")
            }
            Refused::NotAFont(why) => f.write_str(why),
            Refused::NotTaken => f.write_str("the GUI's fonts take no registered faces"),
            Refused::Calls => write!(f, "more than {MAX_ATTEMPTS} calls"),
            Refused::Budget => f.write_str("out of budget"),
        }
    }
}

/// The faces registered so far, in order, and how many calls were made.
#[derive(Debug, Default)]
pub struct Registered {
    faces: Vec<(String, SharedFace)>,
    attempts: u32,
    /// The faces (lower case) a draw cache's own fonts have, which it was not given, and the console lines about them not yet
    /// printed ([`Gui::frame`] prints them).
    withheld: Vec<String>,
    notes: Vec<String>,
}

impl Registered {
    /// The console lines about faces withheld from a draw cache since the last call.
    pub(crate) fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }
}

impl Registered {
    /// How many faces are registered.
    pub fn len(&self) -> usize {
        self.faces.len()
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    fn has(&self, face: &str) -> bool {
        self.faces.iter().any(|(f, _)| f.eq_ignore_ascii_case(face))
    }
}

/// Whether `face` is a name a face may be registered under.
pub fn face_ok(face: &str) -> bool {
    !face.is_empty()
        && face.len() <= MAX_FACE_BYTES
        && face
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'_' | b'-'))
        && !face.starts_with(' ')
        && !face.ends_with(' ')
}

/// Whether a part of a path is a name Windows opens as a device (`con`, `NUL.txt`, `com1`, `lpt9.ttf`).
fn is_device(part: &[u8]) -> bool {
    let stem = part.split(|b| *b == b'.').next().unwrap_or_default();
    let stem = String::from_utf8_lossy(stem)
        .trim_end()
        .to_ascii_lowercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix)
            .is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']))
    };
    DEVICES.contains(&stem.as_str()) || numbered("com") || numbered("lpt")
}

/// Whether `path` names a `.ttf` file below the game's folder (see the module's notes).
pub fn path_ok(path: &[u8]) -> bool {
    if path.is_empty() || path.len() > MAX_PATH_BYTES {
        return false;
    }
    let plain = |b: &u8| *b >= 0x20 && *b != 0x7f && *b != b'\\' && *b != b':';
    path.iter().all(plain)
        && path.to_ascii_lowercase().ends_with(b".ttf")
        && path
            .split(|b| *b == b'/')
            .all(|part| !part.is_empty() && part != b"." && part != b".." && !is_device(part))
}

/// The face a font cache file's name gives (`<face> <size> (<charset>).uft`, any case), if it is one.
fn cache_file_face(file: &[u8], dir: &str) -> Option<String> {
    let lower = String::from_utf8_lossy(file).to_ascii_lowercase();
    let rest = lower.strip_prefix(&format!("{}/", dir.to_ascii_lowercase()))?;
    let name = rest.strip_suffix(".uft")?.strip_suffix(')')?;
    let (head, _charset) = name.rsplit_once(" (")?;
    let (face, size) = head.rsplit_once(' ')?;
    size.bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| face.to_string())
}

/// Whether a `GuiControlProfile` that exists names `face` (any case) as its `fontType`.
fn named_by_a_profile(vm: &Vm, face: &str) -> Result<bool, Refused> {
    let objects = vm.objects();
    let Some(class) = objects.classes.find(b"GuiControlProfile") else {
        return Ok(false);
    };
    for (id, object) in objects.iter() {
        if !objects.classes.is_a(object.class(), class) {
            continue;
        }
        if !vm.charge_work(LIST_WORK) {
            return Err(Refused::Budget);
        }
        let named = objects.field(id, b"fontType", b"").unwrap_or_default();
        if named.eq_ignore_ascii_case(face.as_bytes()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether the game's own fonts have `face` (any case): a default face of the client script layer, one a profile names, one the
/// layout's fonts found, one the fallback would make, or one a cache file of the font folder names.
fn in_use(vm: &Vm, gui: &Gui, face: &str) -> Result<bool, Refused> {
    if crate::client_script::DEFAULT_FACES
        .iter()
        .any(|f| f.eq_ignore_ascii_case(face))
    {
        return Ok(true);
    }
    if named_by_a_profile(vm, face)? {
        return Ok(true);
    }
    let fonts = gui.0.borrow().fonts.0.clone();
    let fonts = fonts.borrow();
    source_has(vm, &**fonts, face)
}

/// Whether the font source `fonts` has `face` (any case) of itself: its fallback would make it, it found it, or a cache file of
/// its folder names it. A face registered over such a source would shadow what its text draws with.
fn source_has(vm: &Vm, fonts: &dyn FontSource, face: &str) -> Result<bool, Refused> {
    let lower = face.to_ascii_lowercase();
    let cache = fonts.cache();
    if cache.fallback_resolves(face) || cache.found().iter().any(|(f, _, _)| *f == lower) {
        return Ok(true);
    }
    for file in vm.file_host().files().iter() {
        if !vm.charge_work(LIST_WORK) {
            return Err(Refused::Budget);
        }
        if cache_file_face(file, &cache.dir).is_some_and(|f| f == lower) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Reads and registers the face.
fn register_face(vm: &Vm, gui: &Gui, face: &[u8], path: &[u8]) -> Result<(), Refused> {
    let name = std::str::from_utf8(face)
        .ok()
        .filter(|f| face_ok(f))
        .ok_or(Refused::FaceName)?;
    if !path_ok(path) {
        return Err(Refused::Path);
    }
    {
        let state = gui.0.borrow();
        if state.faces.has(name) {
            return Err(Refused::FaceInUse);
        }
        if state.faces.len() >= MAX_REGISTERED {
            return Err(Refused::TooManyFaces(MAX_REGISTERED));
        }
    }
    if in_use(vm, gui, name)? {
        return Err(Refused::FaceInUse);
    }
    let len = match vm.file_host().stat(path) {
        FileStat::Found { len } if len <= MAX_TTF_BYTES as u64 => len,
        FileStat::Found { len } => return Err(Refused::TooLarge(len)),
        FileStat::Missing | FileStat::Refused(_) => return Err(Refused::NoFile),
    };
    // The bytes are paid for before they are read and parsed.
    if !vm.charge_work(len) {
        return Err(Refused::Budget);
    }
    let bytes = match vm.file_host().read(path) {
        FileRead::Found(bytes) if bytes.len() <= MAX_TTF_BYTES => bytes,
        FileRead::Found(bytes) => return Err(Refused::TooLarge(bytes.len() as u64)),
        FileRead::Missing | FileRead::Refused(_) => return Err(Refused::NoFile),
    };
    let ttf =
        SharedFace::new(TtfFace::parse(&bytes).map_err(|e| Refused::NotAFont(e.to_string()))?);
    // What the check and the parser walked: every outline point and curve segment.
    let walked = ttf.metrics.points.saturating_add(ttf.metrics.lines);
    if !vm.charge_work(walked.saturating_mul(PARSE_WORK)) {
        return Err(Refused::Budget);
    }
    let fonts = gui.0.borrow().fonts.0.clone();
    if !fonts
        .borrow_mut()
        .register_face(name, SharedFace::clone(&ttf))
    {
        return Err(Refused::NotTaken);
    }
    gui.0.borrow_mut().faces.faces.push((name.to_string(), ttf));
    Ok(())
}

/// Gives `fonts` the faces registered since it took `taken` of them (a draw cache calls it each frame): `taken` is how many it has.
/// A face the source has of itself ([`source_has`]; its folder and fallback need not be the layout's, which the registration
/// checked) is not given to it, so the registered file never replaces what its text draws with: one console line says so.
pub(crate) fn sync(vm: &Vm, gui: &Gui, fonts: &mut dyn FontSource, taken: &mut usize) {
    let pending: Vec<(String, SharedFace)> = {
        let state = gui.0.borrow();
        let Some(rest) = state.faces.faces.get(*taken..).filter(|r| !r.is_empty()) else {
            return;
        };
        rest.to_vec()
    };
    *taken += pending.len();
    for (face, ttf) in pending {
        // A source that cannot be asked (the work budget is spent) is left without the face.
        if source_has(vm, fonts, &face).unwrap_or(true) {
            let mut state = gui.0.borrow_mut();
            let lower = face.to_ascii_lowercase();
            if !state.faces.withheld.contains(&lower) {
                state.faces.notes.push(format!(
                    "registerFont: {face}: a font with this name already exists, so it is kept"
                ));
                state.faces.withheld.push(lower);
            }
            continue;
        }
        fonts.register_face(&face, ttf);
    }
}

pub(crate) fn register(vm: &mut Vm, gui: &Gui) {
    let gui = gui.clone();
    vm.register(
        "registerFont",
        3,
        3,
        "(string face, string path) - registers the TrueType file at path (a .ttf below the game's folder) as the font face, for \
         profiles' fontType; 1 when it was, else 0 with a console line saying why (a face name the game's fonts have is refused)",
        move |vm, c| {
            let (face, path) = (c.arg(1), c.arg(2));
            let calls = {
                let mut state = gui.0.borrow_mut();
                state.faces.attempts = state.faces.attempts.saturating_add(1);
                state.faces.attempts
            };
            let result = if calls > MAX_ATTEMPTS {
                Err(Refused::Calls)
            } else {
                register_face(vm, &gui, face, path)
            };
            let Err(why) = result else {
                return Native::Bool(true);
            };
            let shown = |text: &[u8], max: usize| {
                String::from_utf8_lossy(&text[..text.len().min(max)]).into_owned()
            };
            vm.print(
                format!(
                    "registerFont: {} ({}): {why}",
                    shown(face, MAX_FACE_BYTES),
                    shown(path, MAX_PATH_BYTES)
                )
                .as_bytes(),
            );
            Native::Bool(false)
        },
    );
}

#[cfg(test)]
#[path = "faces_tests.rs"]
mod tests;
