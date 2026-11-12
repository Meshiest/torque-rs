//! The Lua extension the server runs its game mode with: built apart (`just lua`) and loaded from a `plugins/` folder, as a
//! game loads it. This crate never links it.

use std::io;
use std::path::{Path, PathBuf};

use torque::extensions::{
    NativeExtensions, NativeExtensionsPlugin, TerminalPrompt, sha256_hex, shipped_hashes,
};

/// The manifest the Lua extension is loaded with (the one the extension's crate ships).
const LUA_MANIFEST: &str = include_str!("../../../crates/torque-plugin-lua/torque_lua.toml");

/// The Lua extension on disk, ready to load: its library and manifest in a `plugins/` folder of its own. The libraries it
/// approves without asking are listed by hash (`shipped`): the one a build put there itself, or in a packaged game the hashes
/// the bundler compiled into the executable. The bundle's `manifest.toml` is only read to report changed files.
#[derive(Clone, Debug)]
pub struct Lua {
    plugins: PathBuf,
    user: PathBuf,
    shipped: Vec<String>,
    bundle: Option<PathBuf>,
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
            bundle: None,
        })
    }

    /// The extension a packaged game ships in the `plugins/` folder of `bundle` (the folder of its executable), its
    /// approvals kept in the per-user folder ([`torque::extensions::user_folder`]). The libraries it approves without
    /// asking are the ones whose hashes the bundler built into this executable ([`SHIPPED_PLUGINS`]): nothing in the folder
    /// vouches for a library, so one put there later, or a shipped one that changed, is asked about. At start the files the
    /// bundle's `manifest.toml` lists are checked and a changed or missing one is a warning (advisory only).
    pub fn bundled(bundle: &Path) -> Result<Lua, String> {
        let user = torque::extensions::user_folder()
            .ok_or("there is no per-user folder to keep the extension's approvals in")?;
        Ok(Lua {
            plugins: bundle.join("plugins"),
            user,
            shipped: shipped_hashes(SHIPPED_PLUGINS),
            bundle: Some(bundle.to_path_buf()),
        })
    }

    /// The loader. A game asks the player before it loads a library the first time; this one approves the libraries it
    /// shipped (by hash) and asks about any other on the terminal (refused when there is none).
    pub(crate) fn plugin(&self) -> NativeExtensions {
        let plugin = NativeExtensionsPlugin::new(&self.plugins)
            .user_folder(&self.user)
            .shipped(&self.shipped)
            .approver(TerminalPrompt);
        NativeExtensions(match &self.bundle {
            Some(bundle) => plugin.bundle(bundle),
            None => plugin,
        })
    }
}

/// The hashes `torque-bundle` gave this build (`TORQUE_BUNDLE_PLUGINS`, read when it was compiled): the libraries it put
/// in the bundle's `plugins/`. Empty in a build the bundler did not make.
pub const SHIPPED_PLUGINS: &str = match option_env!("TORQUE_BUNDLE_PLUGINS") {
    Some(hashes) => hashes,
    None => "",
};

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
    fn a_build_the_bundler_did_not_make_ships_nothing() {
        if option_env!("TORQUE_BUNDLE_PLUGINS").is_none() {
            assert!(shipped_hashes(SHIPPED_PLUGINS).is_empty());
        }
    }
}
