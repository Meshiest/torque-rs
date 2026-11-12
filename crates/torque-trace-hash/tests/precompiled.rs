//! A script shipped compiled (`torque-bundle --precompile`) runs as its source does: the trace world's server script, put through
//! the bundle's own game-folder pass, runs as a `.dso` with no source and gives the hash of the committed line, and the server
//! really loaded the compiled file.

use std::path::Path;

use torque_bundle::select::{Caps, select};
use torque_bundle::validate::{GameInfo, Limits, Mode, process};
use torque_trace_hash::rig::{Control, Mode as Protocol, Rig, SERVER_SCRIPT, files_with};
use torque_trace_hash::scenarios::{Baseline, figure, figure_with_files};

/// The hash `expected.txt` commits for the figure scenario over Torque's protocol.
fn committed() -> String {
    include_str!("../expected.txt")
        .lines()
        .find_map(|line| line.strip_prefix("figure tge "))
        .expect("a figure line in expected.txt")
        .trim()
        .to_string()
}

/// The game folder `server.cs` is the whole of, as the bundle ships it under `--precompile`: the paths and bytes it would hold.
fn bundled(name: &str) -> Vec<(String, Vec<u8>)> {
    let game = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("precompiled-{name}"));
    let _ = std::fs::remove_dir_all(&game);
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(game.join("server.cs"), SERVER_SCRIPT).unwrap();
    let (selected, _) = select(&game, &[], Caps::default()).unwrap();
    let mode = Mode {
        validate: true,
        precompile: true,
        ..Mode::default()
    };
    let (files, report) = process(&selected, &Limits::default(), mode, &GameInfo::default());
    assert_eq!(report.refused, []);
    assert_eq!(report.precompiled, 1);
    files.into_iter().map(|f| (f.path, f.bytes)).collect()
}

#[test]
fn the_trace_world_hashes_the_same_from_the_bundles_compiled_script_as_from_its_source() {
    let shipped = bundled("hash");
    let paths: Vec<&str> = shipped.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(paths, ["server.cs.dso"]);
    let scripts: Vec<(&str, &[u8])> = shipped
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();

    let source = figure_with_files(files_with(&[("server.cs", SERVER_SCRIPT)])).unwrap();
    let precompiled = figure_with_files(files_with(&scripts)).unwrap();
    assert_eq!(source, precompiled);
    assert_eq!(precompiled, figure(Baseline).unwrap());
    assert_eq!(format!("{:016x}", precompiled.hash), committed());
}

#[test]
fn the_server_loads_the_compiled_file_and_does_not_compile_the_source() {
    let shipped = bundled("log");
    let scripts: Vec<(&str, &[u8])> = shipped
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    let mut rig: Rig = Rig::with_files(
        Protocol::Tge,
        Baseline,
        Control::Figure,
        files_with(&scripts),
        |_| Ok(()),
    )
    .expect("the server starts from the compiled script");
    let log = rig.server.drain_log().join("\n");
    assert!(log.contains("Loading compiled script server.cs."), "{log}");
    assert!(!log.contains("Executing server.cs."), "{log}");
}
