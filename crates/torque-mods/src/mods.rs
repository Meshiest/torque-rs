//! The mods of one script engine: found and mounted when the engine is made ([`ModsPlugin`](crate::ModsPlugin)), their scripts
//! run by stage as TorqueScript packages, and enabled and disabled while it runs.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use torque_engine::{Assets, Engine, Origin};
use torque_ts_vm::ns::DeclareScope;
use torque_ts_vm::policy::ExecPolicy;
use torque_ts_vm::vm::{Vm, VmError};

use crate::discover::{Budget, Found, Notes, scan};
use crate::files::{ALIAS_FOLDER, ModFiles, Mount, MountKind, Table, is_script_path};
use crate::info::{ModInfo, check_id};
use crate::layout::{
    Candidate, ModEnableList, ModLayout, ModNames, ModOrder, ModPrereqs, ModStages, PlainNames,
    PrefsEnableList, PrereqPolicy, TopologicalOrder,
};
use crate::package::{ModError, ModLimits, ModPackage};
use crate::plan::{Inputs, ModSide, plan};

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
    /// The name of its TorqueScript package (`Mod_<id>`).
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
}

/// The mods of a script engine. `Mods::of(&vm)` finds them.
pub struct Mods {
    side: ModSide,
    engine: Engine,
    table: Rc<Table>,
    stages: ModStages,
    limits: ModLimits,
    enable_list: Rc<dyn ModEnableList>,
    state: RefCell<State>,
}

fn package_name(id: &str) -> String {
    format!("Mod_{id}")
}

/// The slots' values, or the defaults.
struct Rules {
    layout: Rc<dyn ModLayout>,
    names: Rc<dyn ModNames>,
    order: Rc<dyn ModOrder>,
    prereqs: Rc<dyn ModPrereqs>,
    enable_list: Rc<dyn ModEnableList>,
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
            state: RefCell::new(State {
                entries,
                loaded: Vec::new(),
            }),
        });
        let layer = Rc::clone(&table);
        vm.wrap_file_host(move |inner| Box::new(ModFiles::new(inner, layer)));
        let origins = Rc::clone(&table);
        let assets: Rc<Assets> = engine.assets(vm);
        assets.set_origins(move |path| {
            if origins.provides(path) {
                Origin::Untrusted
            } else {
                Origin::Trusted
            }
        });
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
        let (held, files) = self.table.held_except(name);
        let mut limits = self.limits;
        limits.total_bytes = limits.total_bytes.saturating_sub(held);
        limits.files = limits.files.saturating_sub(files);
        let package = ModPackage::from_files(kept, &limits)?;
        let count = package.len();
        self.table.remove_server(name);
        self.table.push(Rc::new(Mount::new(
            name.to_string(),
            MountKind::Server,
            package,
            [],
        )));
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
