use std::fs;
use std::time::Duration;

use super::*;

fn scratch(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary's path");
    let dir = exe
        .parent()
        .expect("a folder")
        .join(format!("{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_runs_staging_is_capped_and_has_a_folder_only_when_it_is_given_one() {
    let config = staging_config(Some(PathBuf::from("/data/run")));
    assert_eq!(config.folder, Some(PathBuf::from("/data/run")));
    assert_eq!(config.disk_total, STAGING_DISK_CAP);
    let config = staging_config(None);
    assert_eq!(config.folder, None);
    assert_eq!(config.memory_total, StagingConfig::default().memory_total);
    assert_eq!(
        staging_parent(Path::new("/data")),
        Path::new("/data").join("torque-rs/uploads/.staging")
    );
}

#[test]
fn run_names_differ() {
    let first = run_name();
    std::thread::sleep(Duration::from_millis(2));
    assert_ne!(first, run_name());
}

#[test]
fn a_live_runs_folder_is_never_removed_and_a_dead_ones_is() {
    let parent = scratch("runs");
    let alive = claim_run_folder(&parent, "alive").unwrap();
    fs::write(alive.path().join("upload-0000000000000001.part"), b"in use").unwrap();
    let dead = claim_run_folder(&parent, "dead").unwrap();
    let dead_path = dead.path().to_path_buf();
    fs::write(dead_path.join("upload-0000000000000002.part"), b"left over").unwrap();
    // A folder with no lock file is not a run's: it stays, and so does a file.
    fs::create_dir_all(parent.join("not-a-run")).unwrap();
    fs::write(parent.join("a-file"), b"x").unwrap();
    // The run that ended releases its lock (here, by dropping it); the other is alive however old its folder is.
    drop(dead);
    remove_dead_runs(&parent);
    assert!(alive.path().join("upload-0000000000000001.part").exists());
    assert!(!dead_path.exists());
    assert!(parent.join("not-a-run").exists() && parent.join("a-file").exists());
    // A second claim of a folder that is held is refused.
    assert!(claim_run_folder(&parent, "alive").is_err());
    drop(alive);
    remove_dead_runs(&parent);
    assert!(!parent.join("alive").exists());
    let _ = fs::remove_dir_all(&parent);
}

#[test]
fn a_missing_parent_is_fine() {
    remove_dead_runs(&scratch("none"));
}

mod started {
    use std::net::{Ipv4Addr, SocketAddr};

    use torque_engine::upload::{ServerUploads, UntrustedBytes, UploadCx, UploadSink, UploadSpec};
    use torque_engine::{EngineBuilder, Plugin, PluginId};
    use torque_ts_vm::host::MemFiles;

    use super::*;
    use crate::plugins::MinimalPlugins;
    use crate::server::{GameServer, Listen, ServerStart};

    struct Nothing;

    impl UploadSink for Nothing {
        fn finish(
            &mut self,
            _cx: &mut UploadCx<'_>,
            _meta: &[u8],
            _bytes: UntrustedBytes<'_>,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    #[derive(PluginId)]
    #[plugin(id = "example.uploads")]
    struct Uploads(Option<StagingConfig>);

    impl Plugin for Uploads {
        fn build(&self, engine: &mut EngineBuilder) {
            engine.net().upload("files", UploadSpec::default(), Nothing);
            if let Some(config) = &self.0 {
                engine.net().upload_staging(config.clone());
            }
        }
    }

    fn start(plugin: Uploads, port: u16) -> GameServer {
        let engine = EngineBuilder::new()
            .add_plugins((MinimalPlugins, plugin))
            .build()
            .expect("the engine builds");
        let files = MemFiles::new();
        files.insert("server.cs", b"function startNothing() {}");
        GameServer::start_engine(
            &engine,
            files,
            &ServerStart {
                listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, port))),
                globals: &[],
                start: "startNothing",
                args: &[],
            },
        )
        .expect("the server starts")
    }

    #[test]
    fn a_server_with_an_upload_kind_stages_in_a_locked_folder_of_its_own_under_the_data_folder() {
        let Some(data) = data_dir() else {
            return;
        };
        let server = start(Uploads(None), 28_911);
        let uploads = server
            .vm()
            .extension::<ServerUploads>()
            .expect("the server has uploads");
        let staging = uploads.staging().expect("the state is free");
        let folder = staging.folder.clone().expect("a staging folder");
        assert!(folder.starts_with(staging_parent(&data)), "{folder:?}");
        assert!(folder.is_dir(), "the folder is made when the server starts");
        assert!(folder.join(LOCK_FILE).is_file());
        assert_eq!(staging.disk_total, STAGING_DISK_CAP);
        assert_eq!(uploads.staging_warning(), None);
        // While the server lives, a sweep by another one leaves its folder alone.
        remove_dead_runs(&staging_parent(&data));
        assert!(folder.is_dir());
        drop(uploads);
        drop(server);
        remove_dead_runs(&staging_parent(&data));
        assert!(!folder.exists(), "the folder of a run that ended is swept");
    }

    #[test]
    fn a_games_own_staging_is_left_alone() {
        let own = StagingConfig {
            disk_total: 1 << 20,
            folder: None,
            ..StagingConfig::default()
        };
        let server = start(Uploads(Some(own.clone())), 28_913);
        let uploads = server.vm().extension::<ServerUploads>().unwrap();
        assert_eq!(uploads.staging(), Some(own));
    }
}
