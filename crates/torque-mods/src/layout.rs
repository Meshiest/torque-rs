//! The rules for how mods are found, named, ordered and enabled, each a slot a game or a plugin may fill
//! ([`EngineMods`](crate::EngineMods)): [`ModLayout`], [`ModNames`], [`ModOrder`], [`ModPrereqs`], [`ModEnableList`] and
//! [`ModStages`]. A slot nobody fills has the default (`mod.toml` folders and zips, plain names, topological order, skip with
//! a warning, the `$Pref::Mods::Disabled` list, the stages `server` and `client`).
//!
//! The rules are code, but what they work on is checked first and below them: a layout is handed a mod's files after the caps
//! and path rules were applied ([`ModPackage`]), and what it makes of them ([`ModInfo`]) is checked before it is used.

use std::path::PathBuf;

use torque_engine::Engine;
use torque_ts_vm::vm::Vm;

use crate::info::{ModInfo, Requirement};
use crate::package::ModPackage;

/// What a candidate is: a folder of files, or one zip file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CandidateKind {
    /// A folder of the root.
    Folder,
    /// A `.zip` file of the root.
    Zip,
}

/// Where a root is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RootPlace {
    /// A folder in the game's files (what scripts see: a game folder on disk, a pack, a web build's files).
    Files(String),
    /// A folder on the player's disk, outside the game's files (not on the web). Links inside it are never followed.
    Disk(PathBuf),
}

/// A place mods are searched for: the folders and zips directly in it are the candidates.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModRoot {
    /// Where.
    pub place: RootPlace,
    /// Whether folders are candidates.
    pub folders: bool,
    /// Whether `.zip` files are candidates.
    pub zips: bool,
}

impl ModRoot {
    /// The folder `path` of the game's files, folders and zips both.
    pub fn files(path: impl Into<String>) -> ModRoot {
        ModRoot {
            place: RootPlace::Files(path.into()),
            folders: true,
            zips: true,
        }
    }

    /// The folder `path` on the disk, folders and zips both.
    pub fn disk(path: impl Into<PathBuf>) -> ModRoot {
        ModRoot {
            place: RootPlace::Disk(path.into()),
            folders: true,
            zips: true,
        }
    }

    /// Only folders are candidates.
    pub fn folders_only(mut self) -> ModRoot {
        self.zips = false;
        self
    }

    /// Only zips are candidates.
    pub fn zips_only(mut self) -> ModRoot {
        self.folders = false;
        self
    }
}

/// What [`ModNames`] makes of a folder's or a zip's name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NameInfo {
    /// The name the layout works with: the default layout uses it as the id when the manifest has none.
    pub name: String,
    /// A grouping the name gave (its prefix, say), kept for the layout.
    pub category: Option<String>,
}

impl NameInfo {
    /// A name with no category.
    pub fn plain(name: impl Into<String>) -> NameInfo {
        NameInfo {
            name: name.into(),
            category: None,
        }
    }
}

/// Maps the name of a folder (or of a zip, without `.zip`) to the name its layout works with. The default keeps every name
/// except the ones that start with `.` or `_` (editors' and tools' leftovers), and `None` skips the candidate.
///
/// ```
/// use torque_mods::{CandidateKind, ModNames, NameInfo};
///
/// /// `ui-minimap` is the mod `minimap` of the category `ui`.
/// struct Prefixed;
/// impl ModNames for Prefixed {
///     fn map(&self, raw: &str, _kind: CandidateKind) -> Option<NameInfo> {
///         let (category, name) = raw.split_once('-')?;
///         Some(NameInfo { name: name.to_string(), category: Some(category.to_string()) })
///     }
/// }
/// assert_eq!(Prefixed.map("ui-minimap", CandidateKind::Zip).unwrap().name, "minimap");
/// assert!(Prefixed.map("readme", CandidateKind::Folder).is_none());
/// ```
pub trait ModNames {
    /// The name for `raw`, or `None` to skip this candidate.
    fn map(&self, raw: &str, kind: CandidateKind) -> Option<NameInfo>;
}

/// The default [`ModNames`].
#[derive(Clone, Copy, Debug, Default)]
pub struct PlainNames;

impl ModNames for PlainNames {
    fn map(&self, raw: &str, _kind: CandidateKind) -> Option<NameInfo> {
        (!raw.starts_with(['.', '_'])).then(|| NameInfo::plain(raw))
    }
}

/// A folder or zip found in a root, with its files: what a layout recognizes a mod in.
pub struct Candidate<'a> {
    /// The folder's or zip's own name (a zip's with `.zip`).
    pub raw: &'a str,
    /// Which it is.
    pub kind: CandidateKind,
    /// What [`ModNames`] made of the name.
    pub name: &'a NameInfo,
    /// Where it was found, as a message shows it.
    pub source: &'a str,
    /// Its files, checked against the limits and the path rules.
    pub package: &'a ModPackage,
}

/// How mods are found and what a mod is: which roots are searched, and how a folder or zip is recognized as a mod and read
/// into a [`ModInfo`].
///
/// The default is [`ModTomlLayout`]. A game replaces it with [`EngineMods::mods`](crate::EngineMods::mods)`().layout(..)`.
/// The files a layout sees were checked before it was called (the caps of [`ModLimits`](crate::ModLimits), the path
/// rules), so no layout can load what the caps refuse.
pub trait ModLayout {
    /// The places to search, in the order their mods are found (the order a resolver breaks ties in, by default after
    /// sorting by id).
    fn roots(&self, engine: &Engine) -> Vec<ModRoot>;

    /// Reads a candidate into a mod: `Ok(Some(info))`, `Ok(None)` when it is not a mod (left out with no message), or an
    /// `Err` with why it is not a good one (shown as a warning; the candidate is skipped). `stages` are the stages the game
    /// declared, for a layout that finds entry scripts by name.
    fn recognize(
        &self,
        candidate: &Candidate<'_>,
        stages: &ModStages,
    ) -> Result<Option<ModInfo>, String>;
}

/// A resolver found a loop in what mods ask of each other.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OrderError {
    /// The mods (positions in the list given) that could be placed before the loop was met, in order.
    pub placed: Vec<usize>,
    /// The mods that could not be placed: the loop and what waits on it.
    pub stuck: Vec<usize>,
    /// One loop, as ids in order, the first repeated at the end (`a`, `b`, `a`).
    pub path: Vec<String>,
}

/// Puts mods in load order (the first loads first, a later mount overrides an earlier one). The default is
/// [`TopologicalOrder`]; a game may order by a list, by priority numbers, or by name.
///
/// ```
/// use torque_mods::{ModInfo, ModOrder, OrderError};
///
/// /// Alphabetical by id, ignoring `requires`.
/// struct Alphabetical;
/// impl ModOrder for Alphabetical {
///     fn order(&self, mods: &[&ModInfo]) -> Result<Vec<usize>, OrderError> {
///         let mut at: Vec<usize> = (0..mods.len()).collect();
///         at.sort_by(|&a, &b| mods[a].id.cmp(&mods[b].id));
///         Ok(at)
///     }
/// }
/// let (b, a) = (ModInfo::new("b"), ModInfo::new("a"));
/// assert_eq!(Alphabetical.order(&[&b, &a]), Ok(vec![1, 0]));
/// ```
pub trait ModOrder {
    /// The positions of `mods` in load order. A list that leaves a mod out or repeats one is repaired by the engine (the
    /// missing ones go last, in the order given, with a warning). An `Err` is a loop, which [`ModPrereqs`] then judges.
    fn order(&self, mods: &[&ModInfo]) -> Result<Vec<usize>, OrderError>;
}

/// The default [`ModOrder`]: a mod loads after the mods it requires and the ones it `load_after`, and before the ones that
/// `load_before` it; among mods that may go in either order, the smaller id goes first (so the order never depends on how
/// the folders were listed). A loop is an [`OrderError`].
#[derive(Clone, Copy, Debug, Default)]
pub struct TopologicalOrder;

impl ModOrder for TopologicalOrder {
    fn order(&self, mods: &[&ModInfo]) -> Result<Vec<usize>, OrderError> {
        let find = |id: &str| mods.iter().position(|m| m.id.eq_ignore_ascii_case(id));
        // `before[i]`: the mods that must come before mod i.
        let mut before: Vec<Vec<usize>> = vec![Vec::new(); mods.len()];
        for (at, info) in mods.iter().enumerate() {
            let needs = info.requires.iter().map(|r: &Requirement| r.id.as_str());
            for id in needs.chain(info.load_after.iter().map(String::as_str)) {
                if let Some(other) = find(id)
                    && let Some(list) = before.get_mut(at)
                {
                    list.push(other);
                }
            }
            for id in &info.load_before {
                if let Some(other) = find(id)
                    && let Some(list) = before.get_mut(other)
                {
                    list.push(at);
                }
            }
        }
        let mut placed: Vec<usize> = Vec::with_capacity(mods.len());
        let mut done = vec![false; mods.len()];
        while placed.len() < mods.len() {
            let next = (0..mods.len())
                .filter(|&i| !done.get(i).copied().unwrap_or(true))
                .filter(|&i| {
                    before.get(i).is_some_and(|list| {
                        list.iter().all(|&p| done.get(p).copied().unwrap_or(true))
                    })
                })
                .min_by(|&a, &b| id_of(mods, a).cmp(id_of(mods, b)));
            match next {
                Some(i) => {
                    placed.push(i);
                    if let Some(flag) = done.get_mut(i) {
                        *flag = true;
                    }
                }
                None => break,
            }
        }
        if placed.len() == mods.len() {
            return Ok(placed);
        }
        let stuck: Vec<usize> = (0..mods.len())
            .filter(|&i| !done.get(i).copied().unwrap_or(true))
            .collect();
        let path = find_loop(mods, &before, &done, &stuck);
        Err(OrderError {
            placed,
            stuck,
            path,
        })
    }
}

fn id_of<'a>(mods: &[&'a ModInfo], at: usize) -> &'a str {
    mods.get(at).map_or("", |m| m.id.as_str())
}

/// One loop among the mods that could not be placed: follow, from the smallest stuck id, a mod it waits on that is stuck too,
/// until one repeats.
fn find_loop(
    mods: &[&ModInfo],
    before: &[Vec<usize>],
    done: &[bool],
    stuck: &[usize],
) -> Vec<String> {
    let Some(&start) = stuck
        .iter()
        .min_by(|&&a, &&b| id_of(mods, a).cmp(id_of(mods, b)))
    else {
        return Vec::new();
    };
    let mut chain = vec![start];
    loop {
        let Some(&last) = chain.last() else {
            return Vec::new();
        };
        let waits = before.get(last).and_then(|list| {
            list.iter()
                .copied()
                .filter(|&p| !done.get(p).copied().unwrap_or(true))
                .min_by(|&a, &b| id_of(mods, a).cmp(id_of(mods, b)))
        });
        let Some(next) = waits else {
            return Vec::new();
        };
        if let Some(at) = chain.iter().position(|&c| c == next) {
            let mut ring: Vec<String> = chain[at..]
                .iter()
                .map(|&i| id_of(mods, i).to_string())
                .collect();
            ring.reverse();
            ring.push(ring.first().cloned().unwrap_or_default());
            return ring;
        }
        chain.push(next);
        if chain.len() > mods.len() + 1 {
            return Vec::new();
        }
    }
}

/// What is wrong with a mod's prerequisites.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Problem {
    /// A mod it requires is not there (or was left out for a problem of its own).
    MissingMod {
        /// The id required.
        id: String,
    },
    /// The mod it requires is there, at a version outside what is asked.
    WrongVersion {
        /// What is required.
        needs: Requirement,
        /// The version found.
        have: String,
    },
    /// A plugin it requires is not in the engine (not added, or turned off).
    MissingPlugin {
        /// The plugin id.
        id: String,
    },
    /// The mod is in a loop of requirements, or waits on one.
    Cycle {
        /// One loop, as ids; empty when the loop could not be named.
        path: Vec<String>,
    },
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Problem::MissingMod { id } => {
                write!(f, "it requires the mod `{id}`, which is not loaded")
            }
            Problem::WrongVersion { needs, have } => {
                write!(f, "it requires `{needs}`, but that mod is version {have}")
            }
            Problem::MissingPlugin { id } => write!(
                f,
                "it requires the plugin `{id}`, which the engine does not have"
            ),
            Problem::Cycle { path } if path.is_empty() => {
                f.write_str("it is in a loop of requirements")
            }
            Problem::Cycle { path } => {
                write!(f, "it is in a loop of requirements ({})", path.join(" -> "))
            }
        }
    }
}

/// What to do about a [`Problem`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Leave the mod out, with a warning (the mods that need it go too).
    Skip,
    /// Fail: the server or client does not start, and the error says why.
    Error,
    /// Carry on as if the requirement were met (for a cycle: load the mods in the order they were found).
    Ignore,
}

/// What happens when a mod's prerequisites are not met. The default is [`PrereqPolicy::Skip`]; [`PrereqPolicy`] is also a
/// policy that errors or ignores everything, and a game may decide per problem.
pub trait ModPrereqs {
    /// The action for `problem` of the mod `id`.
    fn decide(&self, id: &str, problem: &Problem) -> Action;
}

/// A [`ModPrereqs`] that answers every problem alike.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PrereqPolicy {
    /// [`Action::Skip`] (the default).
    #[default]
    Skip,
    /// [`Action::Error`].
    Error,
    /// [`Action::Ignore`].
    Ignore,
}

impl ModPrereqs for PrereqPolicy {
    fn decide(&self, _id: &str, _problem: &Problem) -> Action {
        match self {
            PrereqPolicy::Skip => Action::Skip,
            PrereqPolicy::Error => Action::Error,
            PrereqPolicy::Ignore => Action::Ignore,
        }
    }
}

/// Where the enabled and disabled state of mods comes from and is kept. The default ([`PrefsEnableList`]) is game.toml's
/// `[mods] disable` plus the `$Pref::Mods::Disabled` pref, which the game's prefs file keeps; a game may keep its own file.
pub trait ModEnableList {
    /// The ids of the mods that start disabled. Every mod not named is enabled. Asked once, after the game's entry script has run
    /// (so a prefs file it loaded is read); until then no mod is mounted.
    fn disabled(&self, vm: &mut Vm, engine: &Engine, mods: &[&ModInfo]) -> Vec<String>;

    /// Called after every enable or disable with each mod's id and whether it is enabled, to keep them.
    fn save(&self, vm: &mut Vm, engine: &Engine, states: &[(String, bool)]);
}

/// The default [`ModEnableList`]: the ids in game.toml's `[mods] disable` and in the pref `$Pref::Mods::Disabled` (ids
/// separated by spaces) start disabled; every enable and disable writes the pref, and the game saves its prefs as it always
/// does.
#[derive(Clone, Copy, Debug, Default)]
pub struct PrefsEnableList;

/// The pref [`PrefsEnableList`] keeps the disabled mods' ids in.
pub const DISABLED_PREF: &str = "$Pref::Mods::Disabled";

impl ModEnableList for PrefsEnableList {
    fn disabled(&self, vm: &mut Vm, engine: &Engine, _mods: &[&ModInfo]) -> Vec<String> {
        let mut ids: Vec<String> = engine
            .resource::<crate::ModsConfig>()
            .map(|config| config.disable.clone())
            .unwrap_or_default();
        if let Some(text) = vm.global(DISABLED_PREF) {
            let text = String::from_utf8_lossy(&text).into_owned();
            ids.extend(
                text.split_whitespace()
                    .take(crate::info::MAX_LIST * 8)
                    .map(str::to_string),
            );
        }
        ids
    }

    fn save(&self, vm: &mut Vm, _engine: &Engine, states: &[(String, bool)]) {
        let off: Vec<&str> = states
            .iter()
            .filter(|(_, on)| !on)
            .map(|(id, _)| id.as_str())
            .collect();
        vm.set_global(DISABLED_PREF, &off.join(" "));
    }
}

/// The load stages a game declares, and which of them run by themselves. A stage is a name; each mod lists entry scripts per
/// stage, and `loadMods("stage")` (or [`Mods::load_stage`](crate::Mods::load_stage)) runs every enabled mod's scripts for
/// it, in load order. The stages in `server` run on their own when a server's entry script has run, those in `client` when
/// a client's has.
///
/// The default declares `server` and `client`, each running on its side.
///
/// ```
/// use torque_mods::ModStages;
///
/// let stages = ModStages::new(["boot", "gui", "game"]).server(["boot", "game"]).client(["boot", "gui"]);
/// assert!(stages.is_declared("GUI"));
/// assert_eq!(stages.auto_server(), ["boot", "game"]);
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModStages {
    names: Vec<String>,
    server: Vec<String>,
    client: Vec<String>,
}

impl Default for ModStages {
    fn default() -> ModStages {
        ModStages::new(["server", "client"])
            .server(["server"])
            .client(["client"])
    }
}

impl ModStages {
    /// The stages `names`, none of them running by itself. Names are plain (ASCII, no spaces); others are left out.
    pub fn new<S: Into<String>>(names: impl IntoIterator<Item = S>) -> ModStages {
        let mut kept: Vec<String> = Vec::new();
        for name in names {
            let name = name.into();
            let plain = !name.is_empty()
                && name.len() <= crate::info::MAX_NAME_LEN
                && name.chars().all(|c| c.is_ascii_graphic());
            if plain && !kept.iter().any(|k| k.eq_ignore_ascii_case(&name)) {
                kept.push(name);
            }
        }
        ModStages {
            names: kept,
            server: Vec::new(),
            client: Vec::new(),
        }
    }

    /// The stages a server runs by itself once its entry script has run (those that are declared, in the order given).
    pub fn server<S: Into<String>>(mut self, stages: impl IntoIterator<Item = S>) -> ModStages {
        self.server = self.declared(stages);
        self
    }

    /// The stages a client runs by itself once its entry script has run.
    pub fn client<S: Into<String>>(mut self, stages: impl IntoIterator<Item = S>) -> ModStages {
        self.client = self.declared(stages);
        self
    }

    fn declared<S: Into<String>>(&self, stages: impl IntoIterator<Item = S>) -> Vec<String> {
        stages
            .into_iter()
            .map(Into::into)
            .filter_map(|s| {
                self.names
                    .iter()
                    .find(|n| n.eq_ignore_ascii_case(&s))
                    .cloned()
            })
            .collect()
    }

    /// The declared stages.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Whether `stage` is declared (any case).
    pub fn is_declared(&self, stage: &str) -> bool {
        self.names.iter().any(|n| n.eq_ignore_ascii_case(stage))
    }

    /// The stages a server runs by itself.
    pub fn auto_server(&self) -> &[String] {
        &self.server
    }

    /// The stages a client runs by itself.
    pub fn auto_client(&self) -> &[String] {
        &self.client
    }
}

pub use crate::manifest::ModTomlLayout;
