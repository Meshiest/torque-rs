//! The Lua extension the server runs its game mode with: built apart (`just lua`) and loaded from a `plugins/` folder, as a
//! game loads it. This crate never links it.

use std::io;
use std::path::{Path, PathBuf};

use torque::extensions::{
    ApprovalRequest, Approve, NativeExtensions, NativeExtensionsPlugin, TerminalPrompt, sha256_hex,
};

/// The manifest the Lua extension is loaded with (the one the extension's crate ships).
const LUA_MANIFEST: &str = include_str!("../../../crates/torque-plugin-lua/torque_lua.toml");

/// The Lua extension on disk, ready to load: its library and manifest in a `plugins/` folder of its own, and the hashes of
/// the libraries this build put there itself, which it approves without asking.
#[derive(Clone, Debug)]
pub struct Lua {
    plugins: PathBuf,
    user: PathBuf,
    shipped: Vec<String>,
}

/// The file name of the platform's library `name`.
pub fn library_file_name(name: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{name}.dylib")
    } else {
        format!("lib{name}.so")
    }
}

impl Lua {
    /// Puts the built extension `library` (`just lua` makes it) and its manifest in a `plugins` folder under `dir` (emptied
    /// first), and keeps the extension's approvals in a `user` folder beside it.
    pub fn install(library: &Path, dir: &Path) -> io::Result<Lua> {
        let dir = std::path::absolute(dir)?;
        let plugins = dir.join("plugins");
        let user = dir.join("user");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&plugins)?;
        let bytes = std::fs::read(library)?;
        std::fs::write(plugins.join(library_file_name("torque_lua")), &bytes)?;
        std::fs::write(plugins.join("torque_lua.toml"), LUA_MANIFEST)?;
        Ok(Lua {
            plugins,
            user,
            shipped: vec![sha256_hex(&bytes)],
        })
    }

    /// The extension a packaged game ships in the `plugins/` folder of `bundle` (the folder of its executable), its
    /// approvals kept in the per-user folder ([`torque::extensions::user_folder`]). The libraries it approves without
    /// asking are the ones whose hashes the bundler built into this executable ([`SHIPPED_PLUGINS`]): nothing beside it
    /// is read for them, so a library or a file put in the folder later is not among them.
    pub fn bundled(bundle: &Path) -> Result<Lua, String> {
        let user = torque::extensions::user_folder()
            .ok_or("there is no per-user folder to keep the extension's approvals in")?;
        Ok(Lua {
            plugins: bundle.join("plugins"),
            user,
            shipped: shipped_hashes(SHIPPED_PLUGINS),
        })
    }

    /// Whether this build put the library of `request` in its `plugins/` folder itself (by its hash).
    pub fn ships(&self, request: &ApprovalRequest) -> bool {
        self.shipped.contains(&request.sha256)
    }

    /// The loader. A game asks the player before it loads a library the first time; this one approves the libraries it
    /// shipped (by hash) and asks about any other on the terminal (refused when there is none).
    pub(crate) fn plugin(&self) -> NativeExtensions {
        let lua = self.clone();
        NativeExtensions(
            NativeExtensionsPlugin::new(&self.plugins)
                .user_folder(&self.user)
                .approver(move |request: &ApprovalRequest| {
                    lua.ships(request) || TerminalPrompt.approve(request)
                }),
        )
    }
}

/// The hashes `torque-bundle` gave this build (`TORQUE_BUNDLE_PLUGINS`, read when it was compiled): the libraries it put
/// in the bundle's `plugins/`. Empty in a build the bundler did not make.
pub const SHIPPED_PLUGINS: &str = match option_env!("TORQUE_BUNDLE_PLUGINS") {
    Some(hashes) => hashes,
    None => "",
};

/// The hashes in `list` (separated by spaces, as [`SHIPPED_PLUGINS`] holds them), lower-case: only 64 hex digits count.
fn shipped_hashes(list: &str) -> Vec<String> {
    list.split_whitespace()
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
        .collect()
}

/// The built Lua extension's library: the file `TORQUE_LUA_LIBRARY` names, else the one `just lua` leaves in the build
/// folder (`release/`). In a packaged game (feature `bundled`) only the one in its `plugins/` folder.
pub fn lua_library() -> Option<PathBuf> {
    #[cfg(feature = "bundled")]
    return Some(
        crate::bundle_dir()
            .join("plugins")
            .join(library_file_name("torque_lua")),
    )
    .filter(|library| library.is_file());
    #[cfg(not(feature = "bundled"))]
    dev_lua_library()
}

#[cfg(not(feature = "bundled"))]
fn dev_lua_library() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("TORQUE_LUA_LIBRARY") {
        return Some(PathBuf::from(path));
    }
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        PathBuf::from,
    );
    let library = target.join("release").join(library_file_name("torque_lua"));
    library.is_file().then_some(library)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_ships_the_hashes_it_built_in() {
        let (a, b) = ("A".repeat(64), "b".repeat(64));
        assert_eq!(
            shipped_hashes(&format!(" {a}  {b} short {}", "z".repeat(64))),
            ["a".repeat(64), b]
        );
        assert!(shipped_hashes("").is_empty());
        // A build `torque-bundle` did not make has none.
        if option_env!("TORQUE_BUNDLE_PLUGINS").is_none() {
            assert!(shipped_hashes(SHIPPED_PLUGINS).is_empty());
        }
    }

    #[test]
    fn only_a_shipped_library_is_approved_without_asking() {
        let lua = Lua {
            plugins: PathBuf::from("plugins"),
            user: PathBuf::from("user"),
            shipped: vec!["a".repeat(64)],
        };
        let request = |sha256: String| ApprovalRequest {
            id: "torque.lua".to_string(),
            version: "1".to_string(),
            library: PathBuf::from("plugins/x"),
            sha256,
            approved_before: None,
        };
        assert!(lua.ships(&request("a".repeat(64))));
        assert!(!lua.ships(&request("b".repeat(64))));
    }
}
