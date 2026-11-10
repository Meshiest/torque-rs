//! The example game's scripts: a script engine over the game folder, with TGE's game classes declared, that has run the game's
//! `main.cs` and loaded its mission.

use std::fmt;
use std::path::Path;

use torque::game::specs;
use torque::resource::files::{MountPolicy, ResourceFiles};
use torque::script::vm::host::ResourceRules;
use torque::script::vm::vm::{Vm, VmError};

/// The folder of the game folder the resource manager scans, and so the one the scripts' files come from.
pub const GAME_FOLDER: &str = "example";
/// The script the server execs at start-up.
pub const MAIN_SCRIPT: &str = "example/main.cs";
/// The mission the server loads.
pub const MISSION: &str = "example/missions/tiny.mis";

/// The example's own game folder (`examples/minimal/game`), which holds [`GAME_FOLDER`]. In a packaged game (feature `bundled`)
/// the `game/` beside the executable; run by cargo it falls back to this crate's folder, from the variable cargo sets.
#[cfg(not(feature = "bundled"))]
pub fn bundled_root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/game"))
}

#[cfg(feature = "bundled")]
pub fn bundled_root() -> &'static Path {
    static ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("game")))
            .unwrap_or_default();
        match std::env::var_os("CARGO_MANIFEST_DIR") {
            Some(dir) if !beside.is_dir() => Path::new(&dir).join("game"),
            _ => beside,
        }
    })
}

/// Why the game's scripts did not load.
#[derive(Debug)]
pub enum LoadError {
    /// The main script is not in the game folder.
    MissingMain,
    /// A script stopped with an error.
    Script(VmError),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::MissingMain => write!(f, "{MAIN_SCRIPT} is not in the game folder"),
            LoadError::Script(e) => write!(f, "a script failed: {e:?}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<VmError> for LoadError {
    fn from(e: VmError) -> LoadError {
        LoadError::Script(e)
    }
}

/// A script engine over the game folder `root`: its files are what the resource manager finds in [`GAME_FOLDER`], its classes
/// are TGE's game classes (torque-game's declarations), and it has run [`MAIN_SCRIPT`] and `loadExampleMission(MISSION)`.
pub fn load(root: &Path) -> Result<Vm, LoadError> {
    let mut vm = Vm::new();
    let policy = MountPolicy {
        zip_folder: None,
        rules: ResourceRules {
            scanned: &[GAME_FOLDER],
            ..ResourceRules::default()
        },
        ..MountPolicy::default()
    };
    vm.set_file_host(ResourceFiles::open(root, policy));
    specs::install(&mut vm);
    if !vm.exec_file(MAIN_SCRIPT.as_bytes(), false)? {
        return Err(LoadError::MissingMain);
    }
    vm.invoke(None, b"loadExampleMission", &[MISSION.as_bytes().to_vec()])?;
    Ok(vm)
}
