//! The mods of one script engine: found and mounted when the engine is made ([`ModsPlugin`](crate::ModsPlugin)), their scripts
//! run by stage as TorqueScript packages, and enabled and disabled while it runs.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use std::sync::Arc;

use torque_net::chunk::{Kind, MessageHandle};
use torque_net::handshake::ConnId;
use torque_net::manifest::is_name;
use torque_net::modsync::{
    MAX_REQUIRED, ModsSection, RequiredMod, RequiredMods, is_display_name, is_version,
};
use torque_server::net::channels::ChannelHub;
use torque_ts_vm::sim::ObjId;

use torque_engine::{Assets, Engine, Origin};
use torque_ts_vm::ns::DeclareScope;
use torque_ts_vm::policy::ExecPolicy;
use torque_ts_vm::vm::{Vm, VmError};

use crate::content::{ContentSummary, Hash, content_names, summarize, summarize_files};
use crate::discover::{Budget, Found, Notes, scan};
use crate::files::{ALIAS_FOLDER, ModFiles, Mount, MountKind, MountOrigin, Table, is_script_path};
use crate::info::{ModInfo, check_id};
use crate::layout::{
    Candidate, ModEnableList, ModLayout, ModNames, ModOrder, ModPrereqs, ModStages, PlainNames,
    PrefsEnableList, PrereqPolicy, TopologicalOrder,
};
use crate::offer::{
    FileOffer, MAX_OFFER_BYTES, ModOffer, Request, encode_data, encode_offer, run_bytes,
};
use crate::package::{ModError, ModLimits, ModPackage};
use crate::plan::{Inputs, ModSide, plan};
use crate::share::{ModShare, ModShareDefault, Share};
use crate::transfer::{CHANNEL, Caps, Downloads, LocalSource, NetOptions};

/// Whether a mod is loaded, switched off, or was left out.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ModStatus {
    /// Found and valid, not mounted yet: a mod's files are mounted, and its scripts run, only once the game's entry script has
    /// run and the enable list has been read, so nothing of a mod is visible to the entry script.
    Pending,
    /// Mounted, and its scripts run for the stages that ran.
    Enabled,
    /// Found and valid but switched off: not mounted, its package deactivated.
    Disabled,
    /// Left out, with why (a refused file, a missing prerequisite, a bad manifest).
    Skipped(String),
}

/// One mod as a list shows it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModSummary {
    /// What the layout made of the mod.
    pub info: ModInfo,
    /// Where it was found (a folder or a zip).
    pub source: String,
    /// Whether it is loaded.
    pub status: ModStatus,
    /// How many files it has.
    pub files: usize,
    /// The bytes of its files, unpacked.
    pub bytes: u64,
    /// The name of its TorqueScript package (`Mod_<id>`, see [`package_name`]).
    pub package: String,
}

struct Entry {
    summary: ModSummary,
    mount: Option<Rc<Mount>>,
    /// Its package has been made and activated.
    activated: bool,
    /// The stages whose scripts ran.
    ran: BTreeSet<String>,
}

struct State {
    /// The mods in load order, then the ones left out.
    entries: Vec<Entry>,
    /// The stages that have run, in the order they did.
    loaded: Vec<String>,
    /// The content of each mod, by lower-case id, once computed (a mod's files do not change).
    contents: HashMap<String, Option<ContentSummary>>,
    /// The mods a server requires of clients now (a server's, with sharing on).
    required: Vec<RequiredMod>,
    /// What the server serves for that list.
    served: Option<Rc<Served>>,
}

/// The mods of a script engine. `Mods::of(&vm)` finds them.
pub struct Mods {
    side: ModSide,
    engine: Engine,
    table: Rc<Table>,
    stages: ModStages,
    limits: ModLimits,
    enable_list: Rc<dyn ModEnableList>,
    share: Rc<dyn ModShare>,
    /// The asset cache of the same script engine: what a mount of someone else's files must be untrusted to.
    assets: Rc<Assets>,
    net: Option<NetOptions>,
    /// A client's downloads, when the engine shares mods over the network.
    downloads: RefCell<Option<Rc<Downloads>>>,
    /// The clients a server holds while they fetch what it requires.
    serving: RefCell<HashMap<ObjId, Serving>>,
    state: RefCell<State>,
}

/// What a server holds of the content it serves: the offer and the bytes behind it, built when the required list is made.
struct Served {
    offers: Vec<ModOffer>,
    /// The content files of each mod the server serves (the others have none).
    files: Vec<Vec<(String, Vec<u8>)>>,
    /// The offer message, built once and shared by every connection.
    message: Arc<[u8]>,
    /// The blocks of all the served files: what an honest client asks for at most once each.
    blocks: u64,
}

/// A client being held: what it has asked for.
struct Serving {
    conn: ConnId,
    /// The offer queued for this client: its length is allowed on top of the runs until it is acknowledged.
    offer: Option<MessageHandle>,
    /// How many times each run (mod, file, first block) was asked for: an honest client asks once.
    asked: HashMap<(u32, u32, u32), u8>,
    requests: u64,
    max_requests: u64,
    ready: bool,
    /// The progress units that are not runs: one for the ready word.
    progress: u64,
    /// The bytes of the runs queued for the client.
    served: u64,
}

/// How many times a client may ask for the same run (a repeat is for a run that was lost to a refusal, which does not happen on a
/// reliable channel: a small allowance, not a loop).
const MAX_ASKS: u8 = 2;

/// The bytes of the mods channel a client may have queued and unacknowledged: the runs its window keeps asked for
/// ([`crate::transfer::DOWNLOAD_WINDOW`]) and one more, since a run it has already received can still be unacknowledged when
/// the request that refills the window arrives. Only the mods channel counts: what another channel has queued for the client
/// is not the client's doing and must not refuse it.
const MAX_OUTSTANDING: usize = (crate::transfer::DOWNLOAD_WINDOW + 1)
    * crate::offer::BLOCK_BYTES
    * crate::offer::MAX_REQUEST_BLOCKS as usize;

/// Whether `backlog` bytes of the mods channel queued for a client are more than it may have: the window and one run, and the
/// offer while it is still queued (`offer_pending` bytes; 0 once it is acknowledged).
fn over_allowance(backlog: usize, offer_pending: usize) -> bool {
    backlog > offer_pending.saturating_add(MAX_OUTSTANDING)
}

/// The name of the TorqueScript package of the mod `id`: `Mod_` and the id with every character outside `A-Z`, `a-z`, `0-9` and
/// `_` made `_` (`bigger-bombs` is `Mod_bigger_bombs`), so the name is always a valid identifier, which a script language that
/// declares functions into the package needs. Every place that names a mod's package uses this function.
pub fn package_name(id: &str) -> String {
    let mapped: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("Mod_{mapped}")
}

/// The slots' values, or the defaults.
struct Rules {
    layout: Rc<dyn ModLayout>,
    names: Rc<dyn ModNames>,
    order: Rc<dyn ModOrder>,
    prereqs: Rc<dyn ModPrereqs>,
    enable_list: Rc<dyn ModEnableList>,
    share: Rc<dyn ModShare>,
    stages: ModStages,
}

impl Rules {
    fn of(engine: &Engine) -> Rules {
        let options = engine.resource::<crate::plugin::ModsOptions>();
        let default_layout = options
            .as_ref()
            .map(|o| o.layout.clone())
            .unwrap_or_default();
        Rules {
            layout: engine
                .slot::<dyn ModLayout>()
                .unwrap_or_else(|| Rc::new(default_layout)),
            names: engine
                .slot::<dyn ModNames>()
                .unwrap_or_else(|| Rc::new(PlainNames)),
            order: engine
                .slot::<dyn ModOrder>()
                .unwrap_or_else(|| Rc::new(TopologicalOrder)),
            prereqs: engine
                .slot::<dyn ModPrereqs>()
                .unwrap_or_else(|| Rc::new(PrereqPolicy::Skip)),
            enable_list: engine
                .slot::<dyn ModEnableList>()
                .unwrap_or_else(|| Rc::new(PrefsEnableList)),
            share: engine
                .slot::<dyn ModShare>()
                .unwrap_or_else(|| Rc::new(ModShareDefault)),
            stages: engine
                .slot::<ModStages>()
                .map_or_else(ModStages::default, |s| (*s).clone()),
        }
    }
}

/// A mod found and recognized, before it is planned.
struct Recognized {
    info: ModInfo,
    source: String,
    package: ModPackage,
}

/// A mod that could not be made, for the list.
fn failed(found: &Found, reason: String) -> ModSummary {
    let id = if check_id(&found.name.name).is_ok() {
        found.name.name.clone()
    } else {
        format!(
            "?{}",
            found
                .raw
                .chars()
                .filter(char::is_ascii_graphic)
                .take(40)
                .collect::<String>()
        )
    };
    let mut info = ModInfo::new(id.clone());
    info.name = found
        .raw
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect();
    ModSummary {
        package: package_name(&id),
        info,
        source: found.source.clone(),
        status: ModStatus::Skipped(reason),
        files: 0,
        bytes: 0,
    }
}

impl Mods {
    /// The mods of `vm`, when [`ModsPlugin`](crate::ModsPlugin) is in its engine.
    pub fn of(vm: &Vm) -> Option<Rc<Mods>> {
        vm.extension::<Mods>()
    }

    /// Finds, orders and mounts the mods for a new script engine, and installs them into it. Returns the error of a
    /// prerequisite problem the game's policy made an error.
    pub(crate) fn setup(vm: &mut Vm, engine: &Engine, side: ModSide) -> Result<(), String> {
        let rules = Rules::of(engine);
        let limits = engine
            .resource::<crate::plugin::ModsOptions>()
            .map(|o| o.limits)
            .unwrap_or_default();
        let mut notes = Notes::default();
        let mut budget = Budget::new(&limits);
        let mut found: Vec<Found> = Vec::new();
        for root in rules.layout.roots(engine) {
            found.extend(scan(
                &root,
                vm.file_host(),
                rules.names.as_ref(),
                &limits,
                &mut budget,
                &mut notes,
            ));
        }
        let mut recognized: Vec<Recognized> = Vec::new();
        // The package names taken so far, with the id that took each: two different ids can map to one name.
        let mut packages: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut summaries: Vec<ModSummary> = Vec::new();
        for item in found {
            let package = match &item.package {
                Ok(package) => package,
                Err(error) => {
                    summaries.push(failed(&item, error.to_string()));
                    continue;
                }
            };
            let candidate = Candidate {
                raw: &item.raw,
                kind: item.kind,
                name: &item.name,
                source: &item.source,
                package,
            };
            match rules.layout.recognize(&candidate, &rules.stages) {
                Ok(None) => budget.bytes = budget.bytes.saturating_add(package.held_bytes()),
                Err(why) => {
                    budget.bytes = budget.bytes.saturating_add(package.held_bytes());
                    summaries.push(failed(
                        &item,
                        why.chars().filter(|c| !c.is_control()).take(300).collect(),
                    ));
                }
                Ok(Some(mut info)) => {
                    let source = item.source.clone();
                    let Found { package, .. } = item;
                    let Ok(package) = package else { continue };
                    if let Err(why) = info.check() {
                        summaries.push(ModSummary {
                            package: package_name(&info.id),
                            status: ModStatus::Skipped(why),
                            source,
                            files: package.len(),
                            bytes: package.unpacked_bytes(),
                            info,
                        });
                        continue;
                    }
                    let name = package_name(&info.id).to_ascii_lowercase();
                    let id = info.id.to_ascii_lowercase();
                    if let Some(first) = packages.get(&name).filter(|first| **first != id) {
                        let why = format!(
                            "its package name {} is the one of the mod `{first}`",
                            package_name(&info.id)
                        );
                        notes
                            .warnings
                            .push(format!("the mod `{}` is skipped: {why}", info.id));
                        summaries.push(ModSummary {
                            package: package_name(&info.id),
                            status: ModStatus::Skipped(why),
                            source,
                            files: package.len(),
                            bytes: package.unpacked_bytes(),
                            info,
                        });
                        continue;
                    }
                    packages.entry(name).or_insert(id);
                    tidy_scripts(&mut info, &package, &rules.stages);
                    recognized.push(Recognized {
                        info,
                        source,
                        package,
                    });
                }
            }
        }
        let infos: Vec<ModInfo> = recognized.iter().map(|r| r.info.clone()).collect();
        let has_plugin = |id: &str| engine.is_plugin_added_id(id);
        let planned = plan(
            &infos,
            &Inputs {
                side,
                has_plugin: &has_plugin,
                order: rules.order.as_ref(),
                prereqs: rules.prereqs.as_ref(),
            },
        )?;
        let table = Rc::new(Table::default());
        let mut slots: Vec<Option<Recognized>> = recognized.into_iter().map(Some).collect();
        let mut entries: Vec<Entry> = Vec::new();
        for &at in &planned.order {
            let Some(item) = slots.get_mut(at).and_then(Option::take) else {
                continue;
            };
            let mut hidden: Vec<String> = item
                .info
                .scripts
                .iter()
                .flat_map(|(_, paths)| paths.iter().cloned())
                .collect();
            hidden.extend(item.info.hidden.iter().cloned());
            for name in item.package.ignored() {
                notes.warnings.push(format!(
                    "the mod `{}`: `{}` is compiled script; it is not read from a mod (ship the source)",
                    item.info.id,
                    name.chars().take(80).collect::<String>()
                ));
            }
            let summary = ModSummary {
                package: package_name(&item.info.id),
                files: item.package.len(),
                bytes: item.package.unpacked_bytes(),
                status: ModStatus::Pending,
                source: item.source,
                info: item.info,
            };
            let mount = Rc::new(Mount::new(
                summary.info.id.clone(),
                MountKind::Mod,
                item.package,
                hidden,
            ));
            mount.enabled.set(false);
            table.push(Rc::clone(&mount));
            entries.push(Entry {
                summary,
                mount: Some(mount),
                activated: false,
                ran: BTreeSet::new(),
            });
        }
        for (at, skip) in &planned.skipped {
            let Some(item) = slots.get_mut(*at).and_then(Option::take) else {
                continue;
            };
            if !skip.quiet {
                notes.warnings.push(format!(
                    "the mod `{}` is not loaded: {}",
                    item.info.id, skip.reason
                ));
            }
            entries.push(Entry {
                summary: ModSummary {
                    package: package_name(&item.info.id),
                    files: item.package.len(),
                    bytes: item.package.unpacked_bytes(),
                    status: ModStatus::Skipped(skip.reason.clone()),
                    source: item.source,
                    info: item.info,
                },
                mount: None,
                activated: false,
                ran: BTreeSet::new(),
            });
        }
        for summary in summaries {
            notes.warnings.push(format!(
                "{}: not loaded: {}",
                summary.source,
                match &summary.status {
                    ModStatus::Skipped(why) => why.as_str(),
                    _ => "",
                }
            ));
            entries.push(Entry {
                summary,
                mount: None,
                activated: false,
                ran: BTreeSet::new(),
            });
        }
        notes.warnings.extend(planned.notes);
        for entry in &entries {
            for warning in &entry.summary.info.warnings {
                notes
                    .warnings
                    .push(format!("the mod `{}`: {warning}", entry.summary.info.id));
            }
        }
        for warning in notes.warnings {
            vm.warn(warning);
        }

        let mods = Rc::new(Mods {
            side,
            engine: engine.clone(),
            table: Rc::clone(&table),
            stages: rules.stages,
            limits,
            enable_list: rules.enable_list,
            share: rules.share,
            assets: engine.assets(vm),
            net: engine
                .resource::<crate::plugin::ModsOptions>()
                .and_then(|o| o.net.clone()),
            downloads: RefCell::new(None),
            serving: RefCell::new(HashMap::new()),
            state: RefCell::new(State {
                entries,
                loaded: Vec::new(),
                contents: HashMap::new(),
                required: Vec::new(),
                served: None,
            }),
        });
        let layer = Rc::clone(&table);
        vm.wrap_file_host(move |inner| Box::new(ModFiles::new(inner, layer)));
        let origins = Rc::clone(&table);
        let assets: Rc<Assets> = engine.assets(vm);
        // The mounts speak for their own files only: a path no mount serves is none of this layer's business.
        assets.add_origins(move |path| origins.provides(path).then_some(Origin::Untrusted));
        let previous = vm.exec_policy();
        vm.set_exec_policy(Rc::new(ContentOnly { table, previous }));
        vm.set_extension(mods);
        Ok(())
    }

    /// Which side's script engine these mods are in.
    pub fn side(&self) -> ModSide {
        self.side
    }

    /// The stages the game declared.
    pub fn stages(&self) -> &ModStages {
        &self.stages
    }

    /// The limits the mods were loaded under.
    pub fn limits(&self) -> &ModLimits {
        &self.limits
    }

    /// Every mod found: the loaded ones in load order, then the ones left out.
    pub fn list(&self) -> Vec<ModSummary> {
        self.state
            .borrow()
            .entries
            .iter()
            .map(|e| e.summary.clone())
            .collect()
    }

    /// The mod with the id `id` (any case).
    pub fn get(&self, id: &str) -> Option<ModSummary> {
        self.state
            .borrow()
            .entries
            .iter()
            .find(|e| e.summary.info.id.eq_ignore_ascii_case(id))
            .map(|e| e.summary.clone())
    }

    /// Runs the entry scripts of `stage` of every enabled mod, in load order, each mod's as its package (activated first, so
    /// its own functions and its overrides are in force while the script runs). A stage runs once for a mod: asking again
    /// runs nothing new. Returns how many scripts ran. Mods enabled later run the stages that already ran.
    ///
    /// A script that is missing or does not compile prints its error, as `exec` does, and the mod stays enabled.
    pub fn load_stage(&self, vm: &mut Vm, stage: &str) -> Result<usize, String> {
        let Some(name) = self
            .stages
            .names()
            .iter()
            .find(|n| n.eq_ignore_ascii_case(stage))
            .cloned()
        else {
            return Err(format!(
                "`{}` is not a mod stage (the stages are: {})",
                stage
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(40)
                    .collect::<String>(),
                self.stages.names().join(", ")
            ));
        };
        {
            let mut state = self.state.borrow_mut();
            if !state.loaded.contains(&name) {
                state.loaded.push(name.clone());
            }
        }
        let mut ran = 0;
        let count = self.state.borrow().entries.len();
        for at in 0..count {
            ran += self.run_stage_of(vm, at, &name);
        }
        Ok(ran)
    }

    /// Runs the scripts of mod `at` for `stage`, if it is enabled and has not. Nothing is borrowed while a script runs.
    fn run_stage_of(&self, vm: &mut Vm, at: usize, stage: &str) -> usize {
        let (id, package, scripts, first) = {
            let mut state = self.state.borrow_mut();
            let Some(entry) = state.entries.get_mut(at) else {
                return 0;
            };
            if entry.summary.status != ModStatus::Enabled || !entry.ran.insert(stage.to_string()) {
                return 0;
            }
            let scripts = entry.summary.info.scripts_of(stage).to_vec();
            if scripts.is_empty() {
                return 0;
            }
            let first = !entry.activated;
            entry.activated = true;
            (
                entry.summary.info.id.clone(),
                entry.summary.package.clone(),
                scripts,
                first,
            )
        };
        if first && !activate(vm, &package) {
            vm.warn(format!(
                "the mod `{id}`: too many packages are active; its scripts did not run"
            ));
            return 0;
        }
        let mut ran = 0;
        // Functions go into the mod's package only in files from the mod's own mount (not in a game script it execs), and
        // whatever scope was set (a mod's script calling `loadMods`) is put back afterwards.
        let table = Rc::clone(&self.table);
        let own = id.clone();
        let scope = DeclareScope::new(package.as_bytes(), move |file| {
            table.find(file).is_some_and(|hit| {
                hit.mount.kind == MountKind::Mod && hit.mount.id.eq_ignore_ascii_case(&own)
            })
        });
        let previous = vm.namespaces_mut().set_declare_package(Some(scope));
        for script in scripts {
            let path = format!("{ALIAS_FOLDER}/{id}/{script}");
            // Another language's file runs only when the game has that language (a Lua extension); TorqueScript would
            // only fail to compile it.
            let lower = path.to_ascii_lowercase();
            let other = crate::files::OTHER_SCRIPT_EXTENSIONS
                .iter()
                .any(|ext| lower.rsplit_once('.').is_some_and(|(_, e)| e == *ext));
            if other && vm.script_language_of(path.as_bytes()).is_none() {
                vm.warn(format!(
                    "the mod `{id}`: `{script}` did not run: this game has no script language for it"
                ));
                continue;
            }
            match vm.exec_file(path.as_bytes(), false) {
                Ok(true) => ran += 1,
                Ok(false) => vm.warn(format!("the mod `{id}`: the script `{script}` did not run")),
                Err(VmError::StepLimit) => {
                    vm.warn(format!(
                        "the mod `{id}`: the script `{script}` ran out of steps"
                    ));
                }
                Err(error) => vm.warn(format!("the mod `{id}`: the script `{script}`: {error}")),
            }
        }
        vm.namespaces_mut().set_declare_package(previous);
        ran
    }

    /// Reads the enable list and mounts the mods it wants on, in load order: called once the game's entry script has run, which
    /// is what loads a game's prefs. A mod the list switches off stays unmounted (disabled), and so does one whose requirement
    /// is off. Nothing is saved (the list is the source).
    fn start_mods(&self, vm: &mut Vm, engine: &Engine) {
        let infos: Vec<ModInfo> = self
            .state
            .borrow()
            .entries
            .iter()
            .map(|e| e.summary.info.clone())
            .collect();
        let refs: Vec<&ModInfo> = infos.iter().collect();
        let off: Vec<String> = self
            .enable_list
            .disabled(vm, engine, &refs)
            .into_iter()
            .map(|id| id.to_ascii_lowercase())
            .collect();
        let mut notes = Vec::new();
        {
            let mut state = self.state.borrow_mut();
            // Which pending mods are wanted: not listed off, and with every requirement wanted too (until that settles).
            let mut wanted: Vec<bool> = state
                .entries
                .iter()
                .map(|e| {
                    e.summary.status == ModStatus::Pending
                        && !off.contains(&e.summary.info.id.to_ascii_lowercase())
                })
                .collect();
            loop {
                let mut dropped = None;
                for (at, entry) in state.entries.iter().enumerate() {
                    if !wanted.get(at).copied().unwrap_or(false) {
                        continue;
                    }
                    let unmet = entry.summary.info.requires.iter().find(|need| {
                        !state.entries.iter().enumerate().any(|(other, e)| {
                            wanted.get(other).copied().unwrap_or(false)
                                && e.summary.info.id.eq_ignore_ascii_case(&need.id)
                                && need.accepts(&e.summary.info.version)
                        })
                    });
                    if let Some(need) = unmet {
                        notes.push(format!(
                            "the mod `{}` is off because `{need}` is not enabled",
                            entry.summary.info.id
                        ));
                        dropped = Some(at);
                        break;
                    }
                }
                match dropped {
                    Some(at) => {
                        if let Some(flag) = wanted.get_mut(at) {
                            *flag = false;
                        }
                    }
                    None => break,
                }
            }
            for (at, entry) in state.entries.iter_mut().enumerate() {
                if entry.summary.status != ModStatus::Pending {
                    continue;
                }
                let on = wanted.get(at).copied().unwrap_or(false);
                entry.summary.status = if on {
                    ModStatus::Enabled
                } else {
                    ModStatus::Disabled
                };
                if let Some(mount) = &entry.mount {
                    mount.enabled.set(on);
                }
            }
        }
        self.table.bump();
        for note in notes {
            vm.warn(note);
        }
        self.refresh_required(vm);
    }

    /// Runs the stages this side runs by itself (a server's `server`, a client's `client`, by default).
    pub(crate) fn run_auto(&self, vm: &mut Vm) {
        let engine = self.engine.clone();
        self.start_mods(vm, &engine);
        // Stages the entry script already asked for run for the mods that just came on.
        let loaded = self.state.borrow().loaded.clone();
        for stage in loaded {
            let _ = self.load_stage(vm, &stage);
        }
        let stages = match self.side {
            ModSide::Server => self.stages.auto_server(),
            ModSide::Client => self.stages.auto_client(),
        };
        for stage in stages {
            if let Err(error) = self.load_stage(vm, stage) {
                vm.warn(error);
            }
        }
    }

    /// Switches a mod off: its files are unmounted, its package deactivated (what it overrides is back), and the mods that
    /// require it go off with it. Returns the ids switched off, the mod's first. The enable list is told.
    pub fn disable(&self, vm: &mut Vm, id: &str) -> Result<Vec<String>, String> {
        self.disable_with(vm, id, true)
    }

    fn disable_with(&self, vm: &mut Vm, id: &str, save: bool) -> Result<Vec<String>, String> {
        let off: Vec<String> = {
            let state = self.state.borrow();
            let Some(start) = state
                .entries
                .iter()
                .find(|e| e.summary.info.id.eq_ignore_ascii_case(id))
            else {
                return Err(format!("there is no mod `{}`", clean(id)));
            };
            match &start.summary.status {
                ModStatus::Enabled => {}
                ModStatus::Pending => return Err(pending(&start.summary.info.id)),
                ModStatus::Disabled => {
                    return Err(format!(
                        "the mod `{}` is already disabled",
                        start.summary.info.id
                    ));
                }
                ModStatus::Skipped(why) => {
                    return Err(format!(
                        "the mod `{}` is not loaded: {why}",
                        start.summary.info.id
                    ));
                }
            }
            let mut off = vec![start.summary.info.id.clone()];
            let mut at = 0;
            while let Some(next) = off.get(at).cloned() {
                at += 1;
                for entry in &state.entries {
                    let needs = entry
                        .summary
                        .info
                        .requires
                        .iter()
                        .any(|r| r.id.eq_ignore_ascii_case(&next));
                    if needs
                        && entry.summary.status == ModStatus::Enabled
                        && !off
                            .iter()
                            .any(|o| o.eq_ignore_ascii_case(&entry.summary.info.id))
                    {
                        off.push(entry.summary.info.id.clone());
                    }
                }
            }
            off
        };
        {
            let mut state = self.state.borrow_mut();
            for entry in &mut state.entries {
                if !off
                    .iter()
                    .any(|o| o.eq_ignore_ascii_case(&entry.summary.info.id))
                {
                    continue;
                }
                entry.summary.status = ModStatus::Disabled;
                if let Some(mount) = &entry.mount {
                    mount.enabled.set(false);
                }
                if entry.activated {
                    vm.namespaces_mut()
                        .deactivate(entry.summary.package.as_bytes());
                }
            }
        }
        self.table.bump();
        self.refresh_required(vm);
        if save {
            self.save(vm);
        }
        Ok(off)
    }

    /// Switches a mod on: its files are mounted again and its scripts run for the stages that have run (a mod that was on
    /// before only has its package activated again). The mods it requires must be on. Mods after it in the load order
    /// that were on keep their place above it.
    pub fn enable(&self, vm: &mut Vm, id: &str) -> Result<(), String> {
        self.enable_with(vm, id, true)
    }

    fn enable_with(&self, vm: &mut Vm, id: &str, save: bool) -> Result<(), String> {
        let at = {
            let state = self.state.borrow();
            let Some(at) = state
                .entries
                .iter()
                .position(|e| e.summary.info.id.eq_ignore_ascii_case(id))
            else {
                return Err(format!("there is no mod `{}`", clean(id)));
            };
            let Some(entry) = state.entries.get(at) else {
                return Err(format!("there is no mod `{}`", clean(id)));
            };
            match &entry.summary.status {
                ModStatus::Disabled => {}
                ModStatus::Pending => return Err(pending(&entry.summary.info.id)),
                ModStatus::Enabled => {
                    return Err(format!(
                        "the mod `{}` is already enabled",
                        entry.summary.info.id
                    ));
                }
                ModStatus::Skipped(why) => {
                    return Err(format!(
                        "the mod `{}` is not loaded: {why}",
                        entry.summary.info.id
                    ));
                }
            }
            for need in &entry.summary.info.requires {
                let met = state.entries.iter().any(|e| {
                    e.summary.info.id.eq_ignore_ascii_case(&need.id)
                        && e.summary.status == ModStatus::Enabled
                        && need.accepts(&e.summary.info.version)
                });
                if !met {
                    return Err(format!(
                        "the mod `{}` requires `{need}`, which is not enabled",
                        entry.summary.info.id
                    ));
                }
            }
            at
        };
        let (loaded, reactivate, later): (Vec<String>, Option<String>, Vec<String>) = {
            let mut state = self.state.borrow_mut();
            let loaded = state.loaded.clone();
            let later: Vec<String> = state
                .entries
                .iter()
                .skip(at + 1)
                .filter(|e| e.activated && e.summary.status == ModStatus::Enabled)
                .map(|e| e.summary.package.clone())
                .collect();
            let Some(entry) = state.entries.get_mut(at) else {
                return Ok(());
            };
            entry.summary.status = ModStatus::Enabled;
            if let Some(mount) = &entry.mount {
                mount.enabled.set(true);
            }
            let reactivate = entry.activated.then(|| entry.summary.package.clone());
            (loaded, reactivate, later)
        };
        self.table.bump();
        if let Some(package) = reactivate {
            activate(vm, &package);
        }
        for stage in loaded {
            self.run_stage_of(vm, at, &stage);
        }
        // The mods after this one go back on top of it.
        for package in &later {
            vm.namespaces_mut().deactivate(package.as_bytes());
        }
        for package in &later {
            activate(vm, package);
        }
        self.refresh_required(vm);
        if save {
            self.save(vm);
        }
        Ok(())
    }

    fn save(&self, vm: &mut Vm) {
        let states: Vec<(String, bool)> = self
            .state
            .borrow()
            .entries
            .iter()
            .filter(|e| !matches!(e.summary.status, ModStatus::Skipped(_) | ModStatus::Pending))
            .map(|e| {
                (
                    e.summary.info.id.clone(),
                    e.summary.status == ModStatus::Enabled,
                )
            })
            .collect();
        self.enable_list.save(vm, &self.engine, &states);
    }

    /// Mounts files a server sent, as content: whatever could be script (`.cs`, `.dso`, `.gui`, `.mis`, in any case or
    /// under another extension) is left out and can neither be read nor run. The same caps as a mod's apply (the
    /// [`ModLimits`] the mods were loaded under, and what the mods already hold counts too). `name` identifies the mount
    /// (a server's address); mounting under a name again replaces it. Returns how many files were mounted.
    ///
    /// Everything mounted here is [`Origin::Untrusted`](torque_engine::Origin) to the asset cache.
    pub fn mount_server_files(
        &self,
        name: &str,
        files: Vec<(String, Vec<u8>)>,
    ) -> Result<usize, ModError> {
        let kept: Vec<(String, Vec<u8>)> = files
            .into_iter()
            .filter(|(path, _)| !is_script_path(path))
            .collect();
        // What is mounted already counts: every mod's bytes and every other server mount's bytes and files (a mount under
        // this name is being replaced, so it does not), so many mounts together stay within the limits.
        let paths: Vec<String> = if cfg!(debug_assertions) {
            kept.iter().map(|(path, _)| path.clone()).collect()
        } else {
            Vec::new()
        };
        let count = mount_content(&self.table, &self.limits, name, kept, None)?;
        self.assets
            .debug_assert_untrusted("files a server sent", &paths);
        Ok(count)
    }

    /// Unmounts what [`mount_server_files`](Self::mount_server_files) mounted under `name`.
    pub fn unmount_server_files(&self, name: &str) -> bool {
        self.table.remove_server(name)
    }

    /// Whether `path` is served from files a server sent.
    pub fn is_server_file(&self, path: &str) -> bool {
        self.table.is_server_file(path.as_bytes())
    }

    /// Whether `path` is untrusted: it came from a server (the same files [`Origin::Untrusted`](torque_engine::Origin) marks for
    /// the asset cache). Whatever runs files, `exec` or a loader of another scripting language, must refuse a path for which
    /// this is true; only a sandboxed realm made for such files may opt in. `.lua` files a server sent are kept as data and
    /// answer true here.
    pub fn is_untrusted(&self, path: &str) -> bool {
        self.is_server_file(path)
    }
}

/// A required mod whose manifest has `client_script`: what the server tells clients so they can ask the player.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ClientScriptOffer {
    /// The mod's id.
    pub mod_id: String,
    /// The content hash of the mod, which the client's copy must have.
    pub hash: Hash,
    /// The entry script's path within the mod.
    pub entry: String,
    /// The tier names the scripts ask for.
    pub tiers: Vec<String>,
}

impl Mods {
    /// The required mods whose manifests ask for client scripts, each with the content hash the clients verify. Empty on a
    /// client and on a server that requires nothing. The entry is part of the content the hash covers.
    pub fn client_script_offers(&self) -> Vec<ClientScriptOffer> {
        let state = self.state.borrow();
        state
            .required
            .iter()
            .filter_map(|required| {
                let entry = state
                    .entries
                    .iter()
                    .find(|e| e.summary.info.id.eq_ignore_ascii_case(&required.id))?;
                let spec = entry.summary.info.client_script.as_ref()?;
                Some(ClientScriptOffer {
                    mod_id: required.id.clone(),
                    hash: required.hash,
                    entry: spec.entry.clone(),
                    tiers: spec.tiers.clone(),
                })
            })
            .collect()
    }

    /// Whether the files of mod `mod_id` that `server` offered with content hash `hash` are mounted for this connection and
    /// hold `entry`, by the rules of [`client_scripts`](Self::client_scripts). Nothing is read: a client asks this before it asks
    /// the player, and reads the files only once the player has allowed them.
    pub fn client_ready(
        &self,
        server: crate::cache::CacheOwner,
        mod_id: &str,
        hash: &Hash,
        entry: &str,
    ) -> bool {
        self.table.client_ready(mod_id, server, hash, entry)
    }

    /// The script files of mod `mod_id` as the server `server` sent them on this connection, each as `(mods/<id>/<path>, bytes)`:
    /// the one opt-in reader of server-sent script files, for a client-script realm. Only `.lua` and `.luau` files are read, at
    /// most [`MAX_CLIENT_SCRIPT_BYTES`](crate::MAX_CLIENT_SCRIPT_BYTES) of them; the realm's images are read one at a time when
    /// it shows them ([`client_image`](Self::client_image)), and no other file of the mod is read.
    ///
    /// Only the mount that was offered by `server`, verified against `hash`, and reached the client by that server's own
    /// download or its own cache entry is read. A mod the player has installed, content another server sent, an entry found
    /// through cache sharing, a mount made by [`mount_server_files`](Self::mount_server_files) and a hash that differs give
    /// `None`, and so does a mod with more than [`MAX_CLIENT_FILES`](crate::MAX_CLIENT_FILES) files. `exec`, `read_script` and every other path keep
    /// refusing these files ([`is_untrusted`](Self::is_untrusted)).
    pub fn client_scripts(
        &self,
        server: crate::cache::CacheOwner,
        mod_id: &str,
        hash: &Hash,
    ) -> Option<Vec<(String, Arc<[u8]>)>> {
        self.table.client_scripts(mod_id, server, hash)
    }

    /// One script file of that mount ([`client_scripts`](Self::client_scripts)'s rules), by its path within the mod. Only a
    /// `.lua` or `.luau` file.
    pub fn client_script(
        &self,
        server: crate::cache::CacheOwner,
        mod_id: &str,
        hash: &Hash,
        path: &str,
    ) -> Option<Arc<[u8]>> {
        self.table.client_script(mod_id, server, hash, path)
    }

    /// One image file of that mount ([`client_scripts`](Self::client_scripts)'s rules), by its path within the mod: only a
    /// `.png`, `.jpg` or `.jpeg` file of at most [`MAX_CLIENT_IMAGE_BYTES`](crate::MAX_CLIENT_IMAGE_BYTES).
    pub fn client_image(
        &self,
        server: crate::cache::CacheOwner,
        mod_id: &str,
        hash: &Hash,
        path: &str,
    ) -> Option<Arc<[u8]>> {
        self.table.client_image(mod_id, server, hash, path)
    }
}

/// A mod a server requires, or one a client was told of, as a script lists it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RequiredInfo {
    /// The mod's id.
    pub id: String,
    /// The name shown for it (the id when it has no other).
    pub name: String,
    /// Its version.
    pub version: String,
    /// The hash of its content, as 64 hexadecimal digits.
    pub hash: String,
    /// The bytes of its content.
    pub bytes: u64,
    /// How many files it has.
    pub files: u32,
    /// Whether the server sends it to a client that lacks it.
    pub download: bool,
}

impl Mods {
    /// The content of mod `id` (the files that are not scripts: see [`crate::content`]): its hash, file count and bytes. `None`
    /// when the mod is not loaded or a file of it does not read.
    pub fn content_of(&self, id: &str) -> Option<ContentSummary> {
        let key = id.to_ascii_lowercase();
        if let Some(known) = self.state.borrow().contents.get(&key) {
            return *known;
        }
        let computed = {
            let state = self.state.borrow();
            let entry = state
                .entries
                .iter()
                .find(|e| e.summary.info.id.eq_ignore_ascii_case(id))?;
            let mount = entry.mount.as_ref()?;
            summarize(&mount.package, &hidden_of(&entry.summary.info))
        };
        self.state.borrow_mut().contents.insert(key, computed);
        computed
    }

    /// The mods a server requires of clients now: the enabled mods its [`ModShare`] rule asks for, each with its content hash.
    /// Empty on a client, and on a server whose plugin does not share mods
    /// ([`ModsPlugin::required_mods`](crate::ModsPlugin::required_mods)).
    pub fn required(&self) -> Vec<RequiredMod> {
        self.state.borrow().required.clone()
    }

    /// What scripts see of the required mods: a server's own list, or on a client the list its server announced.
    pub(crate) fn required_info(&self) -> Vec<RequiredInfo> {
        let info = |m: &RequiredMod| RequiredInfo {
            id: m.id.clone(),
            name: m.name.clone(),
            version: m.version.clone(),
            hash: crate::content::hex(&m.hash),
            bytes: m.bytes,
            files: m.files,
            download: m.download,
        };
        if self.side == ModSide::Server {
            return self.state.borrow().required.iter().map(info).collect();
        }
        self.downloads
            .borrow()
            .as_ref()
            .map(|d| {
                d.offer()
                    .iter()
                    .map(|o| RequiredInfo {
                        id: o.id.clone(),
                        name: o.name.clone(),
                        version: o.version.clone(),
                        hash: crate::content::hex(&o.hash),
                        bytes: o.bytes(),
                        files: u32::try_from(o.files.len()).unwrap_or(u32::MAX),
                        download: o.download,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Works out the list a server requires from the enabled mods and gives it to the gate. Called when the mods start and when
    /// one is switched on or off; clients already connected keep the list they joined under.
    pub(crate) fn refresh_required(&self, vm: &Vm) {
        let Some(net) = &self.net else { return };
        if self.side != ModSide::Server {
            return;
        }
        let candidates: Vec<ModInfo> = self
            .state
            .borrow()
            .entries
            .iter()
            .filter(|e| e.summary.status == ModStatus::Enabled)
            .map(|e| e.summary.info.clone())
            .collect();
        let limits = net.limits;
        let mut list: Vec<RequiredMod> = Vec::new();
        let mut sendable: u64 = 0;
        for info in candidates {
            let Some(content) = self.content_of(&info.id) else {
                continue;
            };
            let share = self.share.share(&info, &content);
            let version = info.version.to_string();
            if share == Share::None
                || !is_name(&info.id)
                || !is_version(&version)
                || list.len() >= MAX_REQUIRED
            {
                continue;
            }
            let largest = self.largest_file(&info.id);
            let fits = content.files > 0
                && content.bytes <= limits.mod_bytes as u64
                && content.files as usize <= limits.files
                && largest <= limits.file_bytes
                && sendable.saturating_add(content.bytes) <= limits.total_bytes;
            let download = share == Share::Download && net.allow_downloads && fits;
            if download {
                sendable = sendable.saturating_add(content.bytes);
            }
            // A name a client could not show is the id.
            let name = if is_display_name(&info.name) {
                info.name.clone()
            } else {
                info.id.clone()
            };
            list.push(RequiredMod {
                id: info.id.clone(),
                name,
                version,
                hash: content.hash,
                bytes: content.bytes,
                files: content.files,
                download,
            });
        }
        let served = self.serve(&mut list);
        let mut state = self.state.borrow_mut();
        state.required = list.clone();
        state.served = served.map(Rc::new);
        drop(state);
        if let Some(handle) = vm.extension::<RequiredMods>() {
            handle.set(list);
        }
    }

    /// The offer and the content of `list`'s served mods. A mod whose offer would take the message past its cap is no longer
    /// served (largest first) and the list says so.
    fn serve(&self, list: &mut [RequiredMod]) -> Option<Served> {
        let mut offers: Vec<ModOffer> = Vec::with_capacity(list.len());
        let mut files: Vec<Vec<(String, Vec<u8>)>> = Vec::with_capacity(list.len());
        for need in list.iter() {
            let mut offer = ModOffer {
                id: need.id.clone(),
                name: need.name.clone(),
                version: need.version.clone(),
                hash: need.hash,
                download: false,
                files: Vec::new(),
            };
            let mut data = Vec::new();
            if need.download
                && let Some(read) = self.read_content(&need.id)
            {
                let offered: Option<Vec<FileOffer>> =
                    read.iter().map(|(p, d)| FileOffer::of(p, d)).collect();
                if let Some(offered) = offered
                    && summarize_files(&read).hash == need.hash
                {
                    offer.download = true;
                    offer.files = offered;
                    data = read;
                }
            }
            offers.push(offer);
            files.push(data);
        }
        loop {
            let message = encode_offer(&offers);
            if message.len() <= MAX_OFFER_BYTES {
                let blocks = offers
                    .iter()
                    .flat_map(|o| &o.files)
                    .map(|f| f.blocks.len() as u64)
                    .sum();
                for (need, offer) in list.iter_mut().zip(&offers) {
                    need.download = offer.download;
                }
                return Some(Served {
                    offers,
                    files,
                    message: Arc::from(message),
                    blocks,
                });
            }
            // Over the cap: stop serving the biggest served mod and try again.
            let biggest = offers
                .iter()
                .enumerate()
                .filter(|(_, o)| o.download)
                .max_by_key(|(_, o)| o.files.len())
                .map(|(at, _)| at)?;
            offers[biggest].download = false;
            offers[biggest].files.clear();
            files[biggest].clear();
        }
    }

    /// The content files of the enabled mod `id`, sorted as the package hash takes them.
    fn read_content(&self, id: &str) -> Option<Vec<(String, Vec<u8>)>> {
        let state = self.state.borrow();
        let entry = state.entries.iter().find(|e| {
            e.summary.status == ModStatus::Enabled && e.summary.info.id.eq_ignore_ascii_case(id)
        })?;
        let mount = entry.mount.as_ref()?;
        let mut files = Vec::new();
        for name in content_names(&mount.package, &hidden_of(&entry.summary.info)) {
            let data = mount.package.read(&name)?;
            files.push((name, data));
        }
        Some(files)
    }

    /// A client was accepted and can take content: holds it and queues the offer, shared with every other connection.
    pub(crate) fn begin_serving(
        &self,
        hub: &ChannelHub,
        obj: ObjId,
        conn: ConnId,
        _section: &ModsSection,
    ) -> Result<bool, String> {
        let served = self.state.borrow().served.clone();
        let (message, blocks): (Arc<[u8]>, u64) = match &served {
            Some(s) => (Arc::clone(&s.message), s.blocks),
            None => (Arc::from(encode_offer(&[])), 0),
        };
        let offer = hub
            .send_shared_with_handle(conn, CHANNEL, Kind::Snapshot, message)
            .map_err(|e| format!("The server could not queue its mods for you ({e:?})."))?;
        // With nothing required the client has its (empty) list and nothing holds its join.
        if self.state.borrow().required.is_empty() {
            return Ok(false);
        }
        self.serving.borrow_mut().insert(
            obj,
            Serving {
                conn,
                offer: Some(offer),
                requests: 0,
                asked: HashMap::new(),
                max_requests: blocks.saturating_add(8),
                ready: false,
                progress: 0,
                served: 0,
            },
        );
        Ok(true)
    }

    /// A held client asks for a run of blocks. Only what the offer lists is served, a bounded number of times, and never faster
    /// than the client takes it. An `Err` is the reason the client is dropped.
    pub(crate) fn serve_request(
        &self,
        hub: &ChannelHub,
        obj: ObjId,
        request: Request,
    ) -> Result<(), &'static str> {
        let (conn, offer_handle) = {
            let mut serving = self.serving.borrow_mut();
            let held = serving.get_mut(&obj).ok_or("Unexpected mod request.")?;
            held.requests += 1;
            if held.requests > held.max_requests {
                return Err("Too many mod requests.");
            }
            let again = held
                .asked
                .entry((request.mod_index, request.file, request.start))
                .or_insert(0);
            *again = again.saturating_add(1);
            if *again > MAX_ASKS {
                return Err("Too many mod requests.");
            }
            (held.conn, held.offer)
        };
        let served = self
            .state
            .borrow()
            .served
            .clone()
            .ok_or("Unexpected mod request.")?;
        let offer = served
            .offers
            .get(request.mod_index as usize)
            .filter(|o| o.download)
            .ok_or("Mod request out of range.")?;
        let file = offer
            .files
            .get(request.file as usize)
            .ok_or("Mod request out of range.")?;
        let end = request
            .start
            .checked_add(request.count)
            .ok_or("Mod request out of range.")?;
        if request.count == 0
            || request.count > crate::offer::MAX_REQUEST_BLOCKS
            || end as usize > file.blocks.len()
        {
            return Err("Mod request out of range.");
        }
        // What it may have unacknowledged: the offer while it is still unacknowledged (it goes first and may be large), and a few
        // runs.
        let offer_len = offer_handle.map_or(0, |h| hub.queued_message_len(conn, h));
        if over_allowance(hub.backlog(conn, CHANNEL), offer_len) {
            return Err("Mod requests too fast.");
        }
        let (from, to) = run_bytes(
            u64::from(file.size),
            u64::from(request.start),
            u64::from(request.count),
        );
        let data = served
            .files
            .get(request.mod_index as usize)
            .and_then(|f| f.get(request.file as usize))
            .and_then(|(_, d)| d.get(from as usize..to as usize))
            .ok_or("Mod request out of range.")?;
        hub.send_to(conn, CHANNEL, Kind::Snapshot, encode_data(&request, data))
            .map_err(|_| "Mod requests too fast.")?;
        if let Some(held) = self.serving.borrow_mut().get_mut(&obj) {
            held.served = held
                .served
                .saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
        }
        Ok(())
    }

    /// A held client says it has what the server requires.
    pub(crate) fn mark_ready(&self, obj: ObjId) {
        if let Some(held) = self.serving.borrow_mut().get_mut(&obj) {
            held.ready = true;
            held.progress += 1;
        }
    }

    pub(crate) fn serving_ready(&self, obj: ObjId) -> bool {
        self.serving.borrow().get(&obj).is_none_or(|s| s.ready)
    }

    /// What the client has taken of what it asked for: the bytes of its runs the link has acknowledged (those queued less what
    /// the mods channel still holds besides the offer), and one for the ready word. A client with several runs asked for at once
    /// is measured by what reaches it, not by what it asked, so a slow link is not stalled by the time its last runs take.
    pub(crate) fn serving_progress(&self, hub: Option<&ChannelHub>, obj: ObjId) -> u64 {
        let serving = self.serving.borrow();
        let Some(held) = serving.get(&obj) else {
            return 0;
        };
        let unacked = hub.map_or(0, |hub| {
            let offer = held
                .offer
                .map_or(0, |h| hub.queued_message_len(held.conn, h));
            hub.backlog(held.conn, CHANNEL).saturating_sub(offer)
        });
        let unacked = u64::try_from(unacked).unwrap_or(u64::MAX);
        held.served
            .saturating_sub(unacked)
            .saturating_add(held.progress)
    }

    pub(crate) fn end_serving(&self, obj: ObjId) {
        self.serving.borrow_mut().remove(&obj);
    }

    /// The size of the largest content file of mod `id`.
    fn largest_file(&self, id: &str) -> u64 {
        let state = self.state.borrow();
        let Some(entry) = state
            .entries
            .iter()
            .find(|e| e.summary.info.id.eq_ignore_ascii_case(id))
        else {
            return 0;
        };
        let Some(mount) = &entry.mount else { return 0 };
        content_names(&mount.package, &hidden_of(&entry.summary.info))
            .iter()
            .filter_map(|name| mount.package.size_of(name))
            .max()
            .unwrap_or(0)
    }

    /// The caps this client holds itself to now: the game's, held to the player's prefs.
    fn caps(&self, vm: &Vm) -> Option<Caps> {
        use torque_engine::VmPrefs;
        let net = self.net.as_ref()?;
        let prefs = vm.prefs::<crate::plugin::ModPrefs>();
        let mut limits = net.limits;
        if prefs.download_megabytes > 0 {
            limits.total_bytes = limits
                .total_bytes
                .min(u64::from(prefs.download_megabytes.unsigned_abs()) << 20);
        }
        if prefs.cache_megabytes > 0 {
            limits.cache_bytes = limits
                .cache_bytes
                .min(u64::from(prefs.cache_megabytes.unsigned_abs()) << 20);
        }
        Some(Caps {
            downloads: prefs.download,
            limits,
        })
    }

    /// What a client's connect block says: that it takes content, and the most. Nothing about the mods it has.
    pub(crate) fn connect_section(&self, vm: &Vm) -> Option<ModsSection> {
        let downloads = self.downloads.borrow().clone()?;
        let caps = self.caps(vm)?;
        let share = {
            use torque_engine::VmPrefs;
            vm.prefs::<crate::plugin::ModPrefs>().share_cache
        };
        let name = self.net.as_ref().and_then(|net| net.server.as_deref());
        let target = vm
            .extension::<torque_engine::net::ConnectTarget>()
            .map(|target| target.0);
        let owner = crate::cache::CacheOwner::of_connection(name, target);
        Some(downloads.begin_connect_to(caps, crate::cache::CacheScope { owner, share }))
    }

    /// Makes a client's downloads and hands them to the mods channel and the connect block. A no-op for an engine that does not
    /// share mods.
    pub(crate) fn attach_client(self: &Rc<Self>, vm: &mut Vm) {
        use torque_server::net::channels::ClientChannels;
        let Some(net) = &self.net else { return };
        let Some(channels) = vm.extension::<ClientChannels>() else {
            return;
        };
        let caps = Caps {
            downloads: true,
            limits: net.limits,
        };
        let cache = net.cache.open(net.limits.cache_bytes, net.server.is_some());
        let downloads = Rc::new(Downloads::new(
            Rc::clone(&self.table),
            self.limits,
            caps,
            cache,
        ));
        let local: Rc<dyn LocalSource> = Rc::clone(self) as Rc<dyn LocalSource>;
        downloads.set_local(Rc::downgrade(&local));
        let bound = channels
            .with_channel::<crate::transfer::ModsChannel, _>(crate::transfer::CHANNEL, |channel| {
                channel.bind(Rc::clone(&downloads))
            });
        if bound.is_none() {
            return;
        }
        *self.downloads.borrow_mut() = Some(downloads);
        vm.set_extension(Rc::new(torque_engine::net::ConnectExtras(Rc::new(
            |vm: &Vm| Mods::of(vm).and_then(|mods| mods.connect_section(vm)),
        ))));
    }

    /// Makes the server's half: the content server the host asks when it accepts a client that takes content.
    pub(crate) fn attach_server(&self, vm: &mut Vm) {
        use torque_server::net::content::ContentServerSlot;
        let Some(net) = &self.net else { return };
        vm.set_extension(Rc::new(ContentServerSlot(Rc::new(
            crate::transfer::ModsContentServer {
                limits: net.limits.content(),
            },
        ))));
    }

    /// The state of this client's download as a word, for scripts.
    pub(crate) fn download_state(&self) -> &'static str {
        use crate::transfer::Phase;
        match self.downloads.borrow().as_ref().map(|d| d.phase()) {
            None | Some(Phase::Idle) => "idle",
            Some(Phase::Waiting) => "waiting",
            Some(Phase::Downloading) => "downloading",
            Some(Phase::Done) => "done",
            Some(Phase::Failed) => "failed",
        }
    }

    /// `(bytes received, bytes in all, mods mounted, mods sent)` of this client's download.
    pub(crate) fn download_progress(&self) -> (u64, u64, usize, usize) {
        self.downloads
            .borrow()
            .as_ref()
            .map_or((0, 0, 0, 0), |d| d.progress())
    }

    /// Deletes every entry of this client's download cache; the number of bytes it held.
    pub fn clear_download_cache(&self) -> u64 {
        self.downloads
            .borrow()
            .as_ref()
            .map_or(0, |d| d.clear_cache())
    }

    /// A client's per-tick work: the download's time limit, what scripts hear of it (`onModDownloadStarted`,
    /// `onModDownloadProgress`, `onModDownloadDone`, `onModDownloadFailed`), and the next request or the ready word to the server. A
    /// download that fails ends the connection.
    pub(crate) fn client_tick(vm: &mut Vm) {
        use crate::transfer::{Command, Event, Phase};
        use torque_server::net::channels::ClientChannels;
        let Some(mods) = Mods::of(vm) else { return };
        let Some(downloads) = mods.downloads.borrow().clone() else {
            return;
        };
        let accepted = vm
            .extension::<ClientChannels>()
            .is_some_and(|c| c.is_active());
        let phase = downloads.phase();
        if phase == Phase::Downloading || (phase == Phase::Waiting && accepted) {
            let now = vm.sim_time();
            match downloads.started() {
                None => downloads.set_started(now),
                Some(at) if now.wrapping_sub(at) > downloads.seconds().saturating_mul(1_000) => {
                    downloads.fail(if phase == Phase::Waiting {
                        "The server never said which mods it needs.".to_string()
                    } else {
                        "The mod download took too long.".to_string()
                    });
                }
                Some(_) => {}
            }
        }
        for event in downloads.take_events() {
            match event {
                Event::Started { mods, bytes } => {
                    hook(
                        vm,
                        "onModDownloadStarted",
                        &[mods.to_string(), bytes.to_string()],
                    );
                }
                Event::Mounted { id } => {
                    let (done, total, index, count) = downloads.progress();
                    hook(
                        vm,
                        "onModDownloadProgress",
                        &[
                            id,
                            done.to_string(),
                            total.to_string(),
                            index.to_string(),
                            count.to_string(),
                        ],
                    );
                }
                Event::Done => hook(vm, "onModDownloadDone", &[]),
                Event::Failed(reason) => {
                    hook(vm, "onModDownloadFailed", std::slice::from_ref(&reason));
                    vm.set_global("$Mods::DownloadError", &reason);
                    let _ = vm.eval(
                        b"if (isObject(ServerConnection)) ServerConnection.delete($Mods::DownloadError);",
                    );
                }
            }
        }
        if downloads.phase() == Phase::Downloading
            && let Some(id) = downloads.current()
        {
            let (done, total, index, count) = downloads.progress();
            if downloads.note_reported(done) {
                hook(
                    vm,
                    "onModDownloadProgress",
                    &[
                        id,
                        done.to_string(),
                        total.to_string(),
                        index.to_string(),
                        count.to_string(),
                    ],
                );
            }
        }
        // What the server is waiting for: the next run of blocks, or word that everything is mounted. The numbers are the
        // downloads' own, so the script is fixed text. In the engine's protocol the run is asked for with the RPC, once the
        // connection's RPCs take calls.
        let rpc = crate::transfer::get_rpc_ready(vm);
        if rpc == Some(false) {
            return;
        }
        // Every run the window has room for goes out now, so a link's round trip is paid once for all of them.
        while let Some(command) = downloads.next_command() {
            let Command::Get(r) = command else {
                let _ = vm.eval(b"commandToServer('TorqueModsReady');");
                break;
            };
            let call = if rpc.is_some() {
                format!(
                    "rpcToServer(\"{}\", {}, {}, {}, {});",
                    crate::transfer::GET_RPC,
                    r.mod_index,
                    r.file,
                    r.start,
                    r.count
                )
            } else {
                format!(
                    "commandToServer('TorqueModsGet', {}, {}, {}, {});",
                    r.mod_index, r.file, r.start, r.count
                )
            };
            if let Err(e) = vm.eval(call.as_bytes()) {
                vm.warn(format!("mods: the request for blocks did not go: {e}"));
            }
        }
    }
}

/// Calls the script function `name` with `args` when the game defines it.
fn hook(vm: &mut Vm, name: &str, args: &[String]) {
    if !vm.has_own_function("", name) {
        return;
    }
    let argv: Vec<Vec<u8>> = args.iter().map(|a| a.as_bytes().to_vec()).collect();
    let _ = vm.invoke(None, name.as_bytes(), &argv);
}

/// Mounts `files` as a server's files under `name`, replacing a mount of that name. What is mounted already counts against
/// `limits` (every mod's bytes, and the bytes and files of the other server mounts).
pub(crate) fn mount_content(
    table: &Table,
    limits: &ModLimits,
    name: &str,
    files: Vec<(String, Vec<u8>)>,
    origin: Option<MountOrigin>,
) -> Result<usize, ModError> {
    let (held, count) = table.held_except(name);
    let mut limits = *limits;
    limits.total_bytes = limits.total_bytes.saturating_sub(held);
    limits.files = limits.files.saturating_sub(count);
    let package = ModPackage::from_files(files, &limits)?;
    let count = package.len();
    table.remove_server(name);
    table.push(Rc::new(
        Mount::new(name.to_string(), MountKind::Server, package, []).with_origin(origin),
    ));
    Ok(count)
}

/// The files a layout keeps out of the game's files for `info`: its entry scripts and what it hid (the manifest).
fn hidden_of(info: &ModInfo) -> Vec<String> {
    let mut hidden: Vec<String> = info
        .scripts
        .iter()
        .flat_map(|(_, paths)| paths.iter().cloned())
        .collect();
    hidden.extend(info.hidden.iter().cloned());
    hidden
}

/// Activates the package; false when the engine's limit of active packages is reached.
fn activate(vm: &mut Vm, package: &str) -> bool {
    use torque_ts_vm::ns::Activation;
    vm.namespaces_mut().touch(b"", Some(package.as_bytes()));
    matches!(
        vm.namespaces_mut().activate(package.as_bytes()),
        Activation::Activated | Activation::AlreadyActive
    )
}

/// Drops the entry scripts and files a layout named that the mod does not have, and the stages nobody declared.
fn tidy_scripts(info: &mut ModInfo, package: &ModPackage, stages: &ModStages) {
    let id = info.id.clone();
    let mut warnings = Vec::new();
    let mut kept = Vec::new();
    for (stage, paths) in std::mem::take(&mut info.scripts) {
        if !stages.is_declared(&stage) {
            warnings.push(format!(
                "the stage `{stage}` is not one the game declared; its scripts are skipped"
            ));
            continue;
        }
        let stage = stages
            .names()
            .iter()
            .find(|n| n.eq_ignore_ascii_case(&stage))
            .cloned()
            .unwrap_or(stage);
        let mut have = Vec::new();
        for path in paths {
            match package.spelled(&path) {
                Some(spelled) => have.push(spelled.to_string()),
                None => warnings.push(format!("the script `{}` is not in the mod", clean(&path))),
            }
        }
        match kept
            .iter_mut()
            .find(|(s, _): &&mut (String, Vec<String>)| *s == stage)
        {
            Some((_, list)) => list.extend(have),
            None => kept.push((stage, have)),
        }
    }
    info.scripts = kept;
    info.hidden = std::mem::take(&mut info.hidden)
        .into_iter()
        .filter_map(|p| package.spelled(&p).map(str::to_string))
        .collect();
    info.warnings.extend(warnings);
    let _ = id;
}

fn pending(id: &str) -> String {
    format!("the mod `{id}` is not mounted yet: mods start once the game's entry script has run")
}

fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(64).collect()
}

/// Refuses to `exec` a file a server sent, whatever its name: that is content.
struct ContentOnly {
    table: Rc<Table>,
    previous: Option<Rc<dyn ExecPolicy>>,
}

impl ExecPolicy for ContentOnly {
    fn check(&self, vm: &mut Vm, path: &[u8]) -> Result<bool, VmError> {
        if self.table.is_server_file(path) {
            vm.error(format!(
                "exec: {} came from a server; a server's files are content, never script.",
                String::from_utf8_lossy(path)
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(120)
                    .collect::<String>()
            ));
            return Ok(false);
        }
        match &self.previous {
            Some(previous) => previous.check(vm, path),
            None => Ok(true),
        }
    }
}

impl LocalSource for Mods {
    fn hash_of(&self, id: &str) -> Option<Hash> {
        let enabled = self.state.borrow().entries.iter().any(|e| {
            e.summary.status == ModStatus::Enabled && e.summary.info.id.eq_ignore_ascii_case(id)
        });
        if !enabled {
            return None;
        }
        self.content_of(id).map(|c| c.hash)
    }

    fn bytes_of(&self, id: &str) -> Option<u64> {
        self.content_of(id).map(|c| c.bytes)
    }

    fn files_of(&self, id: &str) -> Option<Vec<(String, Vec<u8>)>> {
        self.read_content(id)
    }
}

#[cfg(test)]
mod allowance_tests {
    use super::*;

    #[test]
    fn the_offer_is_in_the_allowance_only_while_it_is_unacknowledged() {
        let offer = crate::offer::MAX_OFFER_BYTES;
        // Offer still queued: it and a few runs are allowed.
        assert!(!over_allowance(offer + MAX_OUTSTANDING, offer));
        assert!(over_allowance(offer + MAX_OUTSTANDING + 1, offer));
        // Acknowledged: only the runs count, however large the offer was.
        assert!(!over_allowance(MAX_OUTSTANDING, 0));
        assert!(over_allowance(MAX_OUTSTANDING + 1, 0));
        assert!(over_allowance(offer + MAX_OUTSTANDING, 0));
    }

    #[test]
    fn an_honest_client_with_a_large_offer_stays_within_it() {
        // It asks for a run only after it has the whole offer; each run is at most 4 blocks and it holds at most
        // the window and one run of them unacknowledged, with the offer still unacknowledged at worst.
        let offer = 650_000;
        let run = crate::offer::BLOCK_BYTES * crate::offer::MAX_REQUEST_BLOCKS as usize;
        let mut backlog = offer;
        for _ in 0..=crate::transfer::DOWNLOAD_WINDOW {
            assert!(!over_allowance(backlog, offer), "backlog {backlog}");
            backlog += run;
        }
        // The offer acknowledged, a full window and the run it is refilled for are still within bounds.
        assert!(!over_allowance(
            (crate::transfer::DOWNLOAD_WINDOW + 1) * run,
            0
        ));
        assert!(over_allowance(
            (crate::transfer::DOWNLOAD_WINDOW + 2) * run,
            0
        ));
        assert_eq!(
            (crate::transfer::DOWNLOAD_WINDOW + 1) * run,
            MAX_OUTSTANDING
        );
    }
}
