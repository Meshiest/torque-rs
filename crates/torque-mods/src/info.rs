//! What every layout makes of a mod, whatever its manifest looks like: [`ModInfo`], with [`Version`] and [`Requirement`].

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

/// The longest id, name, stage or category (bytes).
pub const MAX_NAME_LEN: usize = 64;
/// The most entries in each list of a [`ModInfo`] (requirements, scripts of a stage, extra fields).
pub const MAX_LIST: usize = 64;
/// The longest value of an extra field or a description (bytes).
pub const MAX_TEXT_LEN: usize = 512;

/// The most tier names a mod's `client_script` may ask for.
pub const MAX_CLIENT_TIERS: usize = 16;

/// The scripts a mod's server sends to run on its players' clients (the manifest's `client_script`): the entry file and the
/// abilities (tiers) the scripts ask for. The names are the server's request; the player's choices and the engine's review
/// decide what a realm gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientScriptSpec {
    /// The entry script's path within the mod: a `.lua` or `.luau` file the mod's content holds.
    pub entry: String,
    /// The tier names asked for, each once, in the order the manifest lists them (at most [`MAX_CLIENT_TIERS`]).
    pub tiers: Vec<String>,
}

impl ClientScriptSpec {
    /// The spec of `entry` and `tiers`, checked: the entry is a plain `.lua` or `.luau` path, the names are plain lower-case
    /// words, each once and few.
    ///
    /// # Errors
    ///
    /// Why the manifest's request is not one.
    pub fn new(entry: &str, tiers: &[String]) -> Result<ClientScriptSpec, String> {
        let lower = entry.to_ascii_lowercase();
        if !(lower.ends_with(".lua") || lower.ends_with(".luau"))
            || crate::package::check_path(entry, &crate::package::ModLimits::default()).is_err()
        {
            return Err(format!(
                "the client script entry `{}` is not a plain .lua or .luau path",
                clean(entry)
            ));
        }
        let mut names: Vec<String> = Vec::new();
        for tier in tiers {
            let plain = !tier.is_empty()
                && tier.len() <= 32
                && tier
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
            if !plain {
                return Err(format!(
                    "the client script tier `{}` is not a plain lower-case name",
                    clean(tier)
                ));
            }
            if names.contains(tier) {
                continue;
            }
            if names.len() >= MAX_CLIENT_TIERS {
                return Err(format!(
                    "the client script asks for more than {MAX_CLIENT_TIERS} tiers"
                ));
            }
            names.push(tier.clone());
        }
        Ok(ClientScriptSpec {
            entry: entry.to_string(),
            tiers: names,
        })
    }
}

/// A mod's version: one to four dotted whole numbers (`1`, `1.2`, `1.2.3`). Versions compare number by number, a missing
/// number counting as 0 (`1.2` equals `1.2.0`).
///
/// ```
/// use torque_mods::Version;
///
/// let v: Version = "1.10".parse()?;
/// assert!(v > "1.9.5".parse()?);
/// assert_eq!(v, "1.10.0".parse()?);
/// assert!("1.x".parse::<Version>().is_err());
/// # Ok::<(), String>(())
/// ```
#[derive(Clone, Debug, Eq)]
pub struct Version(Vec<u32>);

impl Version {
    /// The version `0`, what a mod with no version has.
    pub fn zero() -> Version {
        Version(vec![0])
    }

    fn part(&self, at: usize) -> u32 {
        self.0.get(at).copied().unwrap_or(0)
    }
}

impl std::str::FromStr for Version {
    type Err = String;

    fn from_str(text: &str) -> Result<Version, String> {
        let text = text.trim();
        let bad = || format!("`{}` is not a version (such as `1.2.3`)", clean(text));
        let parts: Vec<&str> = text.split('.').collect();
        if parts.is_empty() || parts.len() > 4 {
            return Err(bad());
        }
        let mut numbers = Vec::with_capacity(parts.len());
        for part in parts {
            if part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            numbers.push(part.parse::<u32>().map_err(|_| bad())?);
        }
        Ok(Version(numbers))
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Version) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Version) -> Ordering {
        (0..4)
            .map(|at| self.part(at).cmp(&other.part(at)))
            .find(|order| order.is_ne())
            .unwrap_or(Ordering::Equal)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self.0.iter().map(u32::to_string).collect();
        f.write_str(&parts.join("."))
    }
}

/// How a [`Requirement`] compares versions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Compare {
    /// `>=`
    AtLeast,
    /// `>`
    Above,
    /// `<=`
    AtMost,
    /// `<`
    Below,
    /// `=`
    Exactly,
}

/// A mod another mod needs: an id, and optionally a version range, written `core`, `core >= 1.2` or `core < 2`.
///
/// ```
/// use torque_mods::Requirement;
///
/// let need: Requirement = "core >= 1.2".parse()?;
/// assert_eq!(need.id, "core");
/// assert!(need.accepts(&"1.2".parse()?));
/// assert!(!need.accepts(&"1.1.9".parse()?));
/// # Ok::<(), String>(())
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Requirement {
    /// The id of the mod needed.
    pub id: String,
    /// The version it must have, if any.
    pub version: Option<(Compare, Version)>,
}

impl Requirement {
    /// A requirement of any version of `id`.
    pub fn any(id: impl Into<String>) -> Requirement {
        Requirement {
            id: id.into(),
            version: None,
        }
    }

    /// Whether a mod of version `have` meets the requirement.
    pub fn accepts(&self, have: &Version) -> bool {
        match &self.version {
            None => true,
            Some((compare, want)) => match compare {
                Compare::AtLeast => have >= want,
                Compare::Above => have > want,
                Compare::AtMost => have <= want,
                Compare::Below => have < want,
                Compare::Exactly => have == want,
            },
        }
    }
}

impl std::str::FromStr for Requirement {
    type Err = String;

    fn from_str(text: &str) -> Result<Requirement, String> {
        let text = text.trim();
        let split = text
            .find(|c: char| c.is_whitespace() || matches!(c, '>' | '<' | '='))
            .unwrap_or(text.len());
        let (id, rest) = text.split_at(split);
        check_id(id)?;
        let rest = rest.trim();
        if rest.is_empty() {
            return Ok(Requirement::any(id));
        }
        let (compare, number) = [
            (">=", Compare::AtLeast),
            ("<=", Compare::AtMost),
            (">", Compare::Above),
            ("<", Compare::Below),
            ("=", Compare::Exactly),
        ]
        .into_iter()
        .find_map(|(sign, compare)| rest.strip_prefix(sign).map(|n| (compare, n)))
        .ok_or_else(|| format!("`{}` needs a comparison such as `>= 1.2`", clean(text)))?;
        Ok(Requirement {
            id: id.to_string(),
            version: Some((compare, number.parse()?)),
        })
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.version {
            None => f.write_str(&self.id),
            Some((compare, version)) => {
                let sign = match compare {
                    Compare::AtLeast => ">=",
                    Compare::Above => ">",
                    Compare::AtMost => "<=",
                    Compare::Below => "<",
                    Compare::Exactly => "=",
                };
                write!(f, "{} {sign} {version}", self.id)
            }
        }
    }
}

/// Everything the engine knows about a mod, as its layout made it from the mod's files (the default layout reads `mod.toml`).
///
/// The engine checks every layout's result before it uses it ([`ModInfo::check`]): ids and names are short and plain, the lists
/// are bounded, and a script path is a safe path of the mod's own files.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModInfo {
    /// The mod's id: ASCII letters, digits, `_`, `-` and `.`, at most [`MAX_NAME_LEN`] bytes. Ids are unique; script natives
    /// and the enable list name mods by it.
    pub id: String,
    /// The version (`0` when the layout has none).
    pub version: Version,
    /// What to call the mod in a list (the id by default).
    pub name: String,
    /// A line about the mod.
    pub description: String,
    /// Who made it.
    pub author: String,
    /// A grouping a layout may give (a folder name's prefix, say); the engine does nothing with it.
    pub category: Option<String>,
    /// Mods this one needs, loaded before it.
    pub requires: Vec<Requirement>,
    /// Mods that load before this one when they are there (no need).
    pub load_after: Vec<String>,
    /// Mods that load after this one when they are there (no need).
    pub load_before: Vec<String>,
    /// Ids of plugins the engine must have for this mod to load.
    pub requires_plugins: Vec<String>,
    /// A mod for clients only: a dedicated server does not load it, and a server never asks clients to have it.
    pub client_side: bool,
    /// What a server asks of clients for this mod (`None`: the engine's default, required when the mod has content).
    pub share: Option<crate::share::Share>,
    /// The scripts the mod's server sends to run on clients, when its manifest asks for them. The entry is part of the mod's
    /// content (its hash), so a client runs only what the server offered.
    pub client_script: Option<ClientScriptSpec>,
    /// Entry scripts per load stage, in the order they run: `(stage, paths)`. A stage must be one the game declared
    /// ([`ModStages`](crate::ModStages)); a path is relative to the mod's own files.
    pub scripts: Vec<(String, Vec<String>)>,
    /// Files that are not laid over the game's files (a manifest, say): they stay under `mods/<id>/`. Entry scripts never are.
    pub hidden: Vec<String>,
    /// Free-form fields a layout keeps for a resolver or a game of its own (`priority`, say). At most [`MAX_LIST`] fields.
    pub extra: BTreeMap<String, String>,
    /// What the layout found odd and went on from; shown with the engine's warnings.
    pub warnings: Vec<String>,
}

impl ModInfo {
    /// A mod called `id` with nothing else set.
    pub fn new(id: impl Into<String>) -> ModInfo {
        let id = id.into();
        ModInfo {
            name: id.clone(),
            id,
            version: Version::zero(),
            description: String::new(),
            author: String::new(),
            category: None,
            requires: Vec::new(),
            load_after: Vec::new(),
            load_before: Vec::new(),
            requires_plugins: Vec::new(),
            client_side: false,
            share: None,
            client_script: None,
            scripts: Vec::new(),
            hidden: Vec::new(),
            extra: BTreeMap::new(),
            warnings: Vec::new(),
        }
    }

    /// The entry scripts of `stage`, if the mod has any.
    pub fn scripts_of(&self, stage: &str) -> &[String] {
        self.scripts
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(stage))
            .map_or(&[], |(_, paths)| paths.as_slice())
    }

    /// Checks what an untrusted manifest may get wrong: the id, the lengths, the list sizes. The paths of scripts are checked
    /// against the mod's files by the loader, which has them.
    pub fn check(&self) -> Result<(), String> {
        check_id(&self.id)?;
        for text in [&self.name, &self.author] {
            if text.len() > MAX_NAME_LEN * 2 || text.chars().any(char::is_control) {
                return Err("a name or an author is too long or has a control character".into());
            }
        }
        if self.description.len() > MAX_TEXT_LEN || self.description.chars().any(char::is_control) {
            return Err("the description is too long or has a control character".into());
        }
        if let Some(category) = &self.category {
            check_label("the category", category)?;
        }
        let lists = [
            self.requires.len(),
            self.load_after.len(),
            self.load_before.len(),
            self.requires_plugins.len(),
            self.scripts.len(),
            self.hidden.len(),
            self.extra.len(),
        ];
        if lists.iter().any(|&n| n > MAX_LIST)
            || self.scripts.iter().any(|(_, s)| s.len() > MAX_LIST)
        {
            return Err(format!("a list has more than {MAX_LIST} entries"));
        }
        for id in self
            .requires
            .iter()
            .map(|r| &r.id)
            .chain(&self.load_after)
            .chain(&self.load_before)
        {
            check_id(id)?;
        }
        for id in &self.requires_plugins {
            check_label("a plugin id", id)?;
        }
        for (stage, _) in &self.scripts {
            check_label("a stage", stage)?;
        }
        for (key, value) in &self.extra {
            check_label("an extra field's name", key)?;
            if value.len() > MAX_TEXT_LEN || value.chars().any(char::is_control) {
                return Err(format!(
                    "the extra field `{}` is too long or has a control character",
                    clean(key)
                ));
            }
        }
        Ok(())
    }
}

/// A mod id: not empty, at most [`MAX_NAME_LEN`] bytes, ASCII letters, digits, `_`, `-` and `.`, and not starting with a dot.
pub fn check_id(id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id.len() <= MAX_NAME_LEN
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "`{}` is not a mod id (up to {MAX_NAME_LEN} letters, digits, `_`, `-` and `.`)",
            clean(id)
        ))
    }
}

fn check_label(what: &str, text: &str) -> Result<(), String> {
    let ok = !text.is_empty()
        && text.len() <= MAX_NAME_LEN
        && text.chars().all(|c| c.is_ascii_graphic());
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{what} `{}` is empty, too long or not plain text",
            clean(text)
        ))
    }
}

/// `text` as it is shown in a message: cut short, with no control characters.
pub(crate) fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME_LEN)
        .collect()
}
