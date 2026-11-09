//! The rules a game sets on the runtime, each installed once when its `Vm` is made: who may declare which
//! functions, what may be allocated, the numeric limits, how some failures are reported, the file-length
//! bookkeeping, the build's version and the extensions a game keeps beside the `Vm`. The defaults are Torque3D's:
//! declare anything, allocate without caps, report real file lengths.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::rc::Rc;

pub use crate::fields::FieldRules;
use crate::sim::{FIRST_DATABLOCK_ID, ObjId};
pub use crate::tags::TagRules;
use crate::vm::{Level, Vm};

/// Whether a script may declare a function (an executor may also refuse a declaration that would replace a
/// native, `Namespaces::shadows_native`).
pub trait DeclarePolicy {
    /// `namespace` is empty for a global function; `trusted` is set for the top level of a compiled file run by
    /// `exec`.
    fn may_declare(&self, namespace: &[u8], name: &[u8], trusted: bool) -> bool;
}

/// What the code running at a moment may allocate: objects and scheduled events. Told when a local frame opens
/// and closes (a policy can keep a stack beside the frames), when an object registers (it may refuse it) and
/// leaves, and when an event is posted, ends or is dropped with its object.
pub trait AllocPolicy {
    fn frame_entered(&self, _vm: &mut Vm) {}
    fn frame_left(&self, _vm: &mut Vm) {}
    /// An object is registering; false refuses it (silently: no `Register object failed` line).
    fn admit(&self, _vm: &mut Vm, _id: ObjId) -> bool {
        true
    }
    /// An object admitted earlier leaves (or its registration was abandoned).
    fn released(&self, _vm: &mut Vm, _id: ObjId) {}
    /// Whether a counted `schedule` is refused now.
    fn refuses_schedule(&self, _vm: &mut Vm) -> bool {
        false
    }
    /// A counted `schedule` posted the event `seq`.
    fn schedule_taken(&self, _vm: &mut Vm, _seq: u32) {}
    /// The event `seq` ran, was cancelled, or was dropped with its object.
    fn event_gone(&self, _vm: &mut Vm, _seq: u32) {}
    /// Events dropped where no object may be deleted (a policy defers what it would delete).
    fn events_dropped(&self, _vm: &mut Vm, _seqs: &[u32]) {}
}

/// The length a file's resource has recorded, where it differs from the file's (the file natives ask; the
/// default reports the real length and records nothing). Paths are as the natives expanded them.
pub trait FileRules {
    /// The length to report for `path`, whose real length is `real`.
    fn size_of(&self, _path: &[u8], real: u64) -> u64 {
        real
    }
    /// Whether the recorded length of `path` is 0 (a file opened for writing and not read since).
    fn is_zero(&self, _path: &[u8]) -> bool {
        false
    }
    /// Whether `FileObject`'s opens expand `./` and `~/` against the running file, as `exec` does (TGE's
    /// `FileObject::openForRead` and `openForWrite` call `Con::expandScriptFilename`); false keeps such a name as
    /// written, so it is refused as outside the default directories.
    fn expand_file_object_names(&self) -> bool {
        true
    }
    /// Whether `FileObject::openForRead` opens an empty file of the disk (false: it answers 0).
    fn opens_empty_files(&self) -> bool {
        true
    }
    /// Whether deleting a `FileObject` that writes leaves its stream open: the file stays locked for the session and
    /// the lines it held are lost (false: it is closed, its lines land).
    fn leaks_deleted_writers(&self) -> bool {
        false
    }
    /// The resource manager's rules the `Vm` applies itself (the write names, a `FileObject`'s open).
    fn resources(&self) -> crate::host::ResourceRules {
        crate::host::ResourceRules::default()
    }
    /// `path` was opened for writing.
    fn zero(&self, _path: &[u8]) {}
    /// `path` was opened for reading (or run).
    fn refresh(&self, _path: &[u8]) {}
    /// `path` was deleted.
    fn forget(&self, _path: &[u8]) {}
    /// `path` was deleted and is listed again from the disk.
    fn relist(&self, _path: &[u8]) {}
    /// `path`, of real length `len`, was listed by a file search.
    fn listed(&self, _path: &[u8], _len: u64) {}
    /// `path` was looked up.
    fn looked_up(&self, _path: &[u8]) {}
    /// A file was copied onto `path`, which existed with that length before (None: it did not).
    fn copied_onto(&self, _path: &[u8], _existed: Option<u64>) {}
}

/// What a game checks of a file `exec` was asked to run, after the name is expanded and has an extension and before
/// the file is looked up: its naming rules, say. The default checks nothing.
pub trait ExecPolicy {
    /// `Ok(false)` refuses the file, after printing why; it may look files up and run script (a refusal's BackTrace).
    fn check(&self, _vm: &mut Vm, _path: &[u8]) -> Result<bool, crate::vm::VmError> {
        Ok(true)
    }
}

/// A language other than TorqueScript that `exec` runs: a file whose extension the language was installed for
/// ([`Vm::set_script_language`]) is read through the file host, after the [`ExecPolicy`] has checked it, and handed to
/// [`run`](Self::run) instead of the compiler. `exec` prints `Executing <name>.` first, as for a script, and a missing file
/// warns `Missing file: <name>!`.
pub trait ScriptLanguage {
    /// Runs `source`, the file `path` (as `exec` expanded it), inside a run of `vm`: the run's step budget and call depth
    /// apply to what the language does through the `Vm` ([`Vm::charge_work`], calls into script), as they do to a script.
    /// `Err` ends the `exec` with that error, as a script's would (`StepLimit`, `CallDepth`).
    fn run(&self, vm: &mut Vm, path: &[u8], source: &[u8]) -> Result<(), crate::vm::VmError>;
}

/// The runtime's numeric limits a game may set differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// How many packages can be active at once.
    pub max_active_packages: usize,
    /// How many ids the datablock range holds, from [`FIRST_DATABLOCK_ID`] on (by default [`DATABLOCK_IDS`]: 14 bits'
    /// worth, as Torque3D's `DataBlockObjectIdBitSize`). The last id of the range is never registered: a datablock's
    /// registration fails once the id counter, which has moved past the new datablock's id, has passed the range.
    pub datablock_ids: u32,
    /// How many events a time-sliced pass ([`EventLimits::per_pass_steps`](crate::events::EventLimits::per_pass_steps))
    /// fires between two looks at its clock (every event it takes counts, the first look comes after the first
    /// batch). 1 by default.
    pub pass_batch: usize,
}

impl Limits {
    /// The last id of the datablock range.
    pub fn datablock_id_limit(&self) -> ObjId {
        FIRST_DATABLOCK_ID.saturating_add(self.datablock_ids.saturating_sub(1))
    }

    /// The most datablocks that can be registered at once (one fewer than the range's ids).
    pub fn max_datablocks(&self) -> u32 {
        self.datablock_ids.saturating_sub(1)
    }
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_active_packages: crate::ns::MAX_ACTIVE_PACKAGES,
            datablock_ids: DATABLOCK_IDS,
            pass_batch: 1,
        }
    }
}

/// The ids of the default datablock range: 14 bits' worth, as Torque3D's `DataBlockObjectIdBitSize`.
pub const DATABLOCK_IDS: u32 = 1 << 14;

/// How the runtime reports a few failures that a game may print differently. The defaults are Torque3D's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsoleRules {
    /// [`Vm::invoke`] of a global function that does not exist prints `<name>: Unknown command.` as a warning
    /// (`Con::execute`, which `call()` uses too); false: it is silent. Either way the result is "".
    pub unknown_command: bool,
    /// The level of the lines `new` prints when it cannot make an object: a class that is not a console class,
    /// a `datablock` of a class that is not one, a class that is not a `SimObject`, a parent that is not found.
    /// Errors in TGE.
    pub new_refusals: Level,
    /// The global function `new` calls (as `Con::evaluate` would) when a datablock does not fit in the datablock
    /// range, between the error and the failed registration's warning; None: no call.
    pub datablock_limit_callback: Option<&'static str>,
    /// A loud report after the runtime's own warnings and refusals: a BackTrace after a refused path, a refused
    /// write name, a `writeLine` on a reader and a missing function the datablock callback names; and the
    /// argument-count and missing-object warnings between blank lines with a BackTrace. Off: the line alone.
    pub backtraces: bool,
}

impl Default for ConsoleRules {
    fn default() -> ConsoleRules {
        ConsoleRules {
            unknown_command: true,
            new_refusals: Level::Error,
            datablock_limit_callback: None,
            backtraces: false,
        }
    }
}

/// The texts of the runtime's own console lines, where a game prints them in its words. Each method returns the
/// line (None: nothing is printed); the level is the caller's.
pub trait Messages {
    /// `activatePackage` announcing the package (normal level).
    fn activating_package(&self, _name: &str) -> Option<String> {
        None
    }
    /// `activatePackage` of a package that is already active (normal level).
    fn package_already_active(&self, _name: &str) -> Option<String> {
        None
    }
    /// `activatePackage` of a package nothing declared (error level).
    fn package_not_found(&self, name: &str) -> Option<String> {
        Some(format!("activatePackage: no package named {name}"))
    }
    /// `deactivatePackage` announcing the package (normal level).
    fn deactivating_package(&self, _name: &str) -> Option<String> {
        None
    }
    /// A `FileObject::writeLine` on an object that is reading (error level).
    fn write_line_while_reading(&self) -> String {
        "FileObject::writeLine: the file is open for reading".to_string()
    }
    /// A write refused for its name (error level).
    fn invalid_write_name(&self, path: &str) -> String {
        format!("cannot write \"{path}\": not a writable file name")
    }
    /// A write the host or another writer refused (error level).
    fn write_open_failed(&self, path: &str) -> String {
        format!("cannot open \"{path}\" for writing")
    }
    /// `cancel` of an event the runtime posted, not a script (error level).
    fn cancel_not_scheduled(&self, seq: i32) -> String {
        format!("cancel: event {seq} is not a scheduled call")
    }
    /// The dynamic id counter wrapped back to its start (normal level).
    fn ids_wrapped(&self) -> Option<String> {
        None
    }
    /// `SimSet::isMember` of a name that is no object (normal level).
    fn is_member_no_object(&self, text: &str) -> String {
        format!("SimSet::isMember: no object {text}")
    }
}

/// The default texts.
struct OwnWords;

impl Messages for OwnWords {}

/// What the version natives answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    /// `getBuildString()`: "Debug" or "Release".
    pub build_string: &'static str,
    /// `getCompileTimeString()`.
    pub compile_time: &'static str,
    /// `getVersionNumber()`.
    pub version_number: i32,
    /// `getVersionString()`.
    pub version_string: &'static str,
}

impl Default for BuildInfo {
    fn default() -> BuildInfo {
        BuildInfo {
            build_string: "Release",
            compile_time: "",
            version_number: 0,
            version_string: "0",
        }
    }
}

/// State a game keeps beside a `Vm`, one value of each type (see [`Vm::extension`]).
#[derive(Default)]
pub struct Extensions(HashMap<TypeId, Rc<dyn Any>>);

impl Extensions {
    pub(crate) fn get<T: Any>(&self) -> Option<Rc<T>> {
        let any = Rc::clone(self.0.get(&TypeId::of::<T>())?);
        any.downcast::<T>().ok()
    }

    pub(crate) fn insert<T: Any>(&mut self, value: Rc<T>) {
        self.0.insert(TypeId::of::<T>(), value);
    }
}

/// Declares anything.
struct AllowAll;

impl DeclarePolicy for AllowAll {
    fn may_declare(&self, _namespace: &[u8], _name: &[u8], _trusted: bool) -> bool {
        true
    }
}

/// Real lengths, nothing refused.
struct RealLengths;

impl FileRules for RealLengths {}

/// The file extensions TorqueScript's own files have (sources, compiled scripts, GUIs, missions and editor files), which
/// no other language may take ([`Vm::set_script_language`]).
pub const RESERVED_EXTENSIONS: &[&str] = &["cs", "dso", "gui", "mis", "ed"];

/// Whether `extension` (without the dot) is one of [`RESERVED_EXTENSIONS`], without case.
pub fn is_reserved_extension(extension: &[u8]) -> bool {
    RESERVED_EXTENSIONS
        .iter()
        .any(|r| r.as_bytes().eq_ignore_ascii_case(extension))
}

/// The installed rules (boxed in the `Vm`).
pub(crate) struct Policies {
    pub(crate) declare: Rc<dyn DeclarePolicy>,
    pub(crate) alloc: Option<Rc<dyn AllocPolicy>>,
    pub(crate) files: Rc<dyn FileRules>,
    pub(crate) exec: Option<Rc<dyn ExecPolicy>>,
    pub(crate) limits: Limits,
    pub(crate) console: ConsoleRules,
    pub(crate) messages: Rc<dyn Messages>,
    pub(crate) build: BuildInfo,
    pub(crate) extensions: Extensions,
    /// The other languages `exec` runs, by their extension (folded, without the dot).
    pub(crate) languages: Vec<(Vec<u8>, Rc<dyn ScriptLanguage>)>,
    /// What the natives declared they do ([`crate::effects`]).
    pub(crate) effects: crate::effects::EffectTable,
}

impl Default for Policies {
    fn default() -> Policies {
        Policies {
            declare: Rc::new(AllowAll),
            alloc: None,
            files: Rc::new(RealLengths),
            exec: None,
            limits: Limits::default(),
            console: ConsoleRules::default(),
            messages: Rc::new(OwnWords),
            build: BuildInfo::default(),
            extensions: Extensions::default(),
            languages: Vec::new(),
            effects: Default::default(),
        }
    }
}

impl Vm {
    /// Installs the declare policy.
    pub fn set_declare_policy(&mut self, policy: Rc<dyn DeclarePolicy>) {
        self.policies_mut().declare = policy;
    }

    /// Installs the allocation policy.
    pub fn set_alloc_policy(&mut self, policy: Rc<dyn AllocPolicy>) {
        self.policies_mut().alloc = Some(policy);
    }

    /// The checks `exec` makes before it looks a file up, if a host installed some.
    pub fn exec_policy(&self) -> Option<Rc<dyn ExecPolicy>> {
        self.policies().exec.clone()
    }

    /// Installs the checks `exec` makes before it looks a file up.
    pub fn set_exec_policy(&mut self, policy: Rc<dyn ExecPolicy>) {
        self.policies_mut().exec = Some(policy);
    }

    /// Installs the file rules.
    pub fn set_file_rules(&mut self, rules: Rc<dyn FileRules>) {
        self.policies_mut().files = rules;
    }

    /// Sets the limits.
    pub fn set_limits(&mut self, limits: Limits) {
        self.policies_mut().limits = limits;
        self.namespaces_mut()
            .set_max_active(limits.max_active_packages);
        self.objects_mut()
            .set_datablock_id_limit(limits.datablock_id_limit());
    }

    /// Sets how the runtime reports the failures [`ConsoleRules`] names.
    pub fn set_console_rules(&mut self, rules: ConsoleRules) {
        self.policies_mut().console = rules;
    }

    /// Installs the texts of the runtime's own console lines.
    pub fn set_messages(&mut self, messages: Rc<dyn Messages>) {
        self.policies_mut().messages = messages;
    }

    /// The texts of the runtime's own console lines.
    pub fn messages(&self) -> Rc<dyn Messages> {
        Rc::clone(&self.policies().messages)
    }

    /// How the runtime reports the failures [`ConsoleRules`] names.
    pub fn console_rules(&self) -> ConsoleRules {
        self.policies().console
    }

    /// Sets how the console types read back ([`FieldRules`]).
    pub fn set_field_rules(&mut self, rules: FieldRules) {
        self.objects_mut().classes.field_rules = rules;
    }

    /// Sets the tagged string table's rules ([`TagRules`]).
    pub fn set_tag_rules(&mut self, rules: TagRules) {
        self.tags_mut().set_rules(rules);
    }

    /// The limits.
    pub fn limits(&self) -> Limits {
        self.policies().limits
    }

    /// Sets what the version natives answer.
    pub fn set_build_info(&mut self, build: BuildInfo) {
        self.policies_mut().build = build;
    }

    /// What the version natives answer.
    pub fn build_info(&self) -> &BuildInfo {
        &self.policies().build
    }

    /// Installs `language` for the files whose extension is `extension` (without the dot, any case): `exec` runs them
    /// with it instead of compiling them as TorqueScript ([`ScriptLanguage`]). False, and nothing changes, for TorqueScript's
    /// own extensions ([`RESERVED_EXTENSIONS`]) and for an extension another language already has: the first keeps its
    /// files, so a second extension declaring them cannot take them over.
    pub fn set_script_language(
        &mut self,
        extension: &str,
        language: Rc<dyn ScriptLanguage>,
    ) -> bool {
        let key = crate::text::fold(extension.trim_start_matches('.').as_bytes());
        if key.is_empty() || is_reserved_extension(&key) {
            return false;
        }
        let languages = &mut self.policies_mut().languages;
        if languages.iter().any(|(k, _)| *k == key) {
            return false;
        }
        languages.push((key, language));
        true
    }

    /// The language installed for files with `extension` (without the dot, any case).
    pub fn script_language(&self, extension: &str) -> Option<Rc<dyn ScriptLanguage>> {
        let key = crate::text::fold(extension.trim_start_matches('.').as_bytes());
        self.language_by_key(&key)
    }

    /// The language installed for the file `path` (by the extension after its last dot).
    pub fn script_language_of(&self, path: &[u8]) -> Option<Rc<dyn ScriptLanguage>> {
        let languages = &self.policies().languages;
        if languages.is_empty() {
            return None;
        }
        let name = path.rsplit(|&b| b == b'/').next().unwrap_or(path);
        let dot = name.iter().rposition(|&b| b == b'.')?;
        self.language_by_key(&crate::text::fold(&name[dot + 1..]))
    }

    fn language_by_key(&self, key: &[u8]) -> Option<Rc<dyn ScriptLanguage>> {
        self.policies()
            .languages
            .iter()
            .find(|(k, _)| k.as_slice() == key)
            .map(|(_, language)| Rc::clone(language))
    }

    /// The extension of type `T` a game attached (see [`set_extension`](Self::set_extension)).
    pub fn extension<T: Any>(&self) -> Option<Rc<T>> {
        self.policies().extensions.get::<T>()
    }

    /// Attaches `value` as this `Vm`'s extension of type `T` (state a game keeps beside the `Vm`, with interior
    /// mutability; one per type), replacing an earlier one.
    pub fn set_extension<T: Any>(&mut self, value: Rc<T>) {
        self.policies_mut().extensions.insert(value);
    }
}
