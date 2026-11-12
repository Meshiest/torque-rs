//! The command line: what `--twice` may remove.

use std::path::{Path, PathBuf};
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("main-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_twice_run_leaves_a_build_folder_it_did_not_make_alone() {
    let dir = tmp("twice");
    let target = dir.join("target");
    // The user's own folders, named as the run's build folders are.
    for name in ["twice-one", "twice-two"] {
        std::fs::create_dir_all(target.join(name)).unwrap();
        std::fs::write(target.join(name).join("mine.txt"), "keep me").unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_torque-bundle"))
        .arg(dir.join("no-such-crate"))
        .arg("--twice")
        .arg("--target-dir")
        .arg(&target)
        .arg("--out")
        .arg(dir.join("out"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("not a folder this tool made"), "{error}");
    for name in ["twice-one", "twice-two"] {
        assert_eq!(
            std::fs::read_to_string(target.join(name).join("mine.txt")).unwrap(),
            "keep me"
        );
    }
}

#[test]
fn only_a_marked_folder_is_removed() {
    let dir = tmp("remove");
    let theirs = dir.join("theirs");
    std::fs::create_dir_all(&theirs).unwrap();
    std::fs::write(theirs.join("x.txt"), "keep me").unwrap();
    assert!(torque_bundle::remove_owned_folder(&theirs).is_err());
    assert!(theirs.join("x.txt").is_file());
    let ours = dir.join("ours");
    torque_bundle::own_folder(&ours).unwrap();
    std::fs::write(ours.join("built.txt"), "x").unwrap();
    torque_bundle::remove_owned_folder(&ours).unwrap();
    assert!(!ours.exists());
}

/// `--verify` reads a bundle from elsewhere: a link in it is reported, not followed.
#[cfg(unix)]
#[test]
fn verify_does_not_follow_a_link_out_of_the_bundle() {
    use torque_bundle::manifest::{Entry, Header, write};
    let dir = tmp("verify-link");
    let outside = dir.join("outside.txt");
    std::fs::write(&outside, "secret").unwrap();
    let bundle = dir.join("bundle");
    std::fs::create_dir_all(&bundle).unwrap();
    std::os::unix::fs::symlink(&outside, bundle.join("a.txt")).unwrap();
    let header = Header {
        name: "x".into(),
        version: "1".into(),
        target: "t".into(),
        mode: "compiled".into(),
        features: Vec::new(),
        rustc: "r".into(),
    };
    std::fs::write(
        bundle.join(torque_bundle::manifest::FILE),
        write(&header, &[Entry::of("a.txt", b"secret")]),
    )
    .unwrap();
    assert_eq!(
        torque_bundle::manifest::verify(&bundle).unwrap(),
        ["a.txt: is not a plain file"]
    );
}

/// `--verify` reads through every part of a path without following a link, and through the no-follow reader.
#[test]
fn a_file_below_a_linked_folder_is_not_read() {
    let dir = tmp("read-below");
    let outside = dir.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "secret").unwrap();
    let bundle = dir.join("bundle");
    std::fs::create_dir_all(bundle.join("real")).unwrap();
    std::fs::write(bundle.join("real/a.txt"), "fine").unwrap();
    let read = |path: &str| torque_bundle::manifest::read_below(&bundle, path, 100);
    assert_eq!(read("real/a.txt").unwrap(), b"fine");
    assert!(matches!(
        read("real/missing.txt"),
        Err(torque_resource::plain::PlainError::Io(_))
    ));
    assert!(matches!(
        read("real/../real/a.txt"),
        Err(torque_resource::plain::PlainError::NotPlain)
    ));
    assert!(matches!(
        torque_bundle::manifest::read_below(&bundle, "real/a.txt", 2),
        Err(torque_resource::plain::PlainError::TooLarge { .. })
    ));
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, bundle.join("link"));
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(&outside, bundle.join("link"));
    #[cfg(not(any(unix, windows)))]
    let linked: std::io::Result<()> = Err(std::io::ErrorKind::Unsupported.into());
    if linked.is_ok() {
        assert!(matches!(
            read("link/secret.txt"),
            Err(torque_resource::plain::PlainError::NotPlain)
        ));
    } else {
        eprintln!("no symbolic links here: the linked folder case is skipped");
    }
}

/// Whatever the program prints is made printable: a folder named with an escape sequence (the command line takes any name, and
/// a game folder or a bundle can hold one) writes none of it to the terminal, in standard output or standard error.
#[test]
fn nothing_printed_holds_a_control_character() {
    let dir = tmp("escape");
    let hostile = dir.join("x\u{1b}]0;title\u{7}\u{1b}[2J");
    for args in [
        vec![hostile.as_os_str().to_owned()],
        vec!["--validate".into(), hostile.as_os_str().to_owned()],
        vec!["--verify".into(), hostile.as_os_str().to_owned()],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_torque-bundle"))
            .args(&args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        for (stream, bytes) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
            let text = String::from_utf8_lossy(bytes);
            assert!(
                !text
                    .chars()
                    .any(|c| c.is_control() && c != '\n' && c != '\r'),
                "{stream}: {text:?}"
            );
        }
        assert!(String::from_utf8_lossy(&output.stderr).contains("title"));
    }
}

/// The same through the walk of a game folder that holds a file and a link named with an escape (Unix only: Windows file names
/// hold no control character).
#[cfg(unix)]
#[test]
fn a_file_and_a_link_named_with_an_escape_are_reported_without_it() {
    let dir = tmp("escape-game");
    let game = dir.join("game");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(
        dir.join("bundle.toml"),
        "[bundle]\nname = \"x\"\npackage = \"x\"\nbins = [\"x\"]\n",
    )
    .unwrap();
    std::fs::write(game.join("f\u{1b}[31m.txt"), "x").unwrap();
    std::os::unix::fs::symlink(&dir, game.join("l\u{1b}]0;title\u{7}")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_torque-bundle"))
        .arg("--validate")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("l?]0;title?: a link"), "{error:?}");
    assert!(
        !error
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\r'),
        "{error:?}"
    );
}
