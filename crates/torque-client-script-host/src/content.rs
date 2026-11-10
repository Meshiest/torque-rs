//! A server's content as the client holds it: the files it downloaded from that server and verified (by hash), keyed by
//! their content paths (`mods/<mod id>/...`). Every script a realm runs is looked up here (the engine reads its images
//! from the same verified files when a control first shows one); nothing of the realm's opens a file on disk by a name it
//! was given.

use std::collections::BTreeMap;
use std::sync::Arc;

use torque_client_script_format::abi::{self, MAX_SCRIPT_BYTES, MAX_SCRIPTS, MAX_SCRIPTS_BYTES};

/// One server's verified files.
#[derive(Clone, Debug, Default)]
pub struct ServerContent {
    files: Arc<BTreeMap<String, Arc<[u8]>>>,
}

impl ServerContent {
    /// The content of `files`: content paths and their verified bytes.
    pub fn new(files: impl IntoIterator<Item = (String, Arc<[u8]>)>) -> ServerContent {
        ServerContent {
            files: Arc::new(files.into_iter().collect()),
        }
    }

    /// The bytes of `path`, when the server's list has it.
    pub fn get(&self, path: &str) -> Option<&Arc<[u8]>> {
        self.files.get(path)
    }

    /// Whether the server's list has `path`: the check a content path of the realm's (an image) must pass.
    pub fn contains(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    /// The files under `mods/<mod_id>/`, by their path within the mod.
    fn files_of<'a>(&'a self, mod_id: &str) -> impl Iterator<Item = (&'a str, &'a Arc<[u8]>)> + 'a {
        let prefix = format!("mods/{mod_id}/");
        self.files
            .range::<str, _>((
                std::ops::Bound::Included(prefix.as_str()),
                std::ops::Bound::Unbounded,
            ))
            .map_while(move |(path, bytes)| Some((path.strip_prefix(prefix.as_str())?, bytes)))
    }
}

/// A mod of the server's with client scripts: its id and the entry file within it (its manifest's
/// `client_script.entry`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientMod {
    /// The mod's id.
    pub id: String,
    /// The entry file's path within the mod.
    pub entry: String,
}

/// A script file a realm keeps.
#[derive(Clone, Debug)]
pub(crate) struct Script {
    pub(crate) mod_id: Arc<str>,
    pub(crate) path: Arc<str>,
    pub(crate) source: Arc<[u8]>,
}

/// Why a file or a mod's scripts were left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The mod's id is not a plain name.
    ModId(String),
    /// The file's path, or its bytes (compiled code), are not a script file's; or it is past a cap.
    File {
        /// The mod's id.
        mod_id: String,
        /// The file's path within the mod.
        path: String,
    },
    /// The mod's entry file is not among its kept scripts.
    Entry {
        /// The mod's id.
        mod_id: String,
        /// The entry's path.
        path: String,
    },
}

/// The script files of `mods` in `content`, within the format's caps, and what was left out. A file counts only when
/// it is a `.lua` or `.luau` file with a plain path and text, not compiled code.
pub(crate) fn scripts(content: &ServerContent, mods: &[ClientMod]) -> (Vec<Script>, Vec<Refused>) {
    let mut kept = Vec::new();
    let mut refused = Vec::new();
    let mut total = 0usize;
    for client_mod in mods {
        if abi::check_mod_id(&client_mod.id).is_err() {
            refused.push(Refused::ModId(client_mod.id.clone()));
            continue;
        }
        let mut has_entry = false;
        let mod_id: Arc<str> = client_mod.id.as_str().into();
        for (path, source) in content.files_of(&client_mod.id) {
            if !(path.ends_with(".lua") || path.ends_with(".luau")) {
                continue;
            }
            let fits = kept.len() < MAX_SCRIPTS
                && source.len() <= MAX_SCRIPT_BYTES
                && total + source.len() <= MAX_SCRIPTS_BYTES;
            if !fits || abi::check_script_path(path).is_err() || abi::is_compiled(source) {
                refused.push(Refused::File {
                    mod_id: client_mod.id.clone(),
                    path: path.to_string(),
                });
                continue;
            }
            total += source.len();
            has_entry |= path == client_mod.entry;
            kept.push(Script {
                mod_id: Arc::clone(&mod_id),
                path: path.into(),
                source: Arc::clone(source),
            });
        }
        if !has_entry {
            refused.push(Refused::Entry {
                mod_id: client_mod.id.clone(),
                path: client_mod.entry.clone(),
            });
        }
    }
    (kept, refused)
}
