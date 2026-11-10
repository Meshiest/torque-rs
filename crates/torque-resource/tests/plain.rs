//! Plain files: opened without following a link, read within a cap.

use std::fs;
use std::path::{Path, PathBuf};

use torque_resource::plain::{PlainError, open_plain, read_plain};

fn folder(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("plain-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A symbolic link to `target` at `link`, or false where this machine cannot make one (Windows needs a privilege).
fn link(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(target, link);
    #[cfg(not(any(unix, windows)))]
    let made: std::io::Result<()> = Err(std::io::ErrorKind::Unsupported.into());
    made.is_ok()
}

#[test]
fn a_file_reads_whole_within_its_cap_and_its_listed_length() {
    let dir = folder("cap");
    let path = dir.join("a.bin");
    fs::write(&path, vec![1u8; 1000]).unwrap();
    assert_eq!(read_plain(&path, 1000, None).unwrap().len(), 1000);
    assert!(matches!(
        read_plain(&path, 999, None),
        Err(PlainError::TooLarge {
            len: 1000,
            cap: 999
        })
    ));
    // A listing that said 10 bytes: the file grew since.
    assert!(matches!(
        read_plain(&path, 1000, Some(10)),
        Err(PlainError::TooLarge { cap: 10, .. })
    ));
    assert!(matches!(
        read_plain(&dir.join("missing"), 10, None),
        Err(PlainError::Io(_))
    ));
}

#[test]
fn a_folder_and_a_link_are_not_plain_files() {
    let dir = folder("kinds");
    fs::create_dir(dir.join("d")).unwrap();
    // A folder is refused (Windows cannot open one as a file at all).
    assert!(open_plain(&dir.join("d")).is_err());
    let outside = folder("kinds-outside");
    fs::write(outside.join("secret.txt"), b"secret").unwrap();
    if link(&outside.join("secret.txt"), &dir.join("l.txt")) {
        assert!(matches!(
            open_plain(&dir.join("l.txt")),
            Err(PlainError::NotPlain)
        ));
        assert!(read_plain(&dir.join("l.txt"), 100, None).is_err());
    } else {
        eprintln!("no symbolic links here: the link case is skipped");
    }
}

/// A FIFO is refused without waiting for a writer to open it.
#[cfg(unix)]
#[test]
fn a_fifo_is_refused_and_does_not_block() {
    let dir = folder("fifo");
    let fifo = dir.join("pipe");
    if !std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .is_ok_and(|s| s.success())
    {
        eprintln!("no mkfifo here: the FIFO case is skipped");
        return;
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(open_plain(&fifo).is_err());
    });
    assert_eq!(
        receiver.recv_timeout(std::time::Duration::from_secs(10)),
        Ok(true),
        "open_plain waited on the FIFO or took it"
    );
}

#[test]
fn a_plain_error_converts_to_an_io_error_of_its_kind() {
    use std::io::ErrorKind;
    let kind = |e: PlainError| std::io::Error::from(e).kind();
    assert_eq!(kind(PlainError::NotPlain), ErrorKind::InvalidInput);
    assert_eq!(
        kind(PlainError::TooLarge { len: 2, cap: 1 }),
        ErrorKind::InvalidData
    );
    assert_eq!(
        kind(PlainError::Io(ErrorKind::NotFound.into())),
        ErrorKind::NotFound
    );
}

/// A file that passes the check is an ordinary blocking handle: `O_NONBLOCK` was only for the open.
#[cfg(unix)]
#[test]
fn an_opened_file_is_not_nonblocking() {
    use rustix::fs::{OFlags, fcntl_getfl};
    let dir = folder("blocking");
    let path = dir.join("a.bin");
    fs::write(&path, b"abc").unwrap();
    let (file, len) = open_plain(&path).unwrap();
    assert_eq!(len, 3);
    assert!(!fcntl_getfl(&file).unwrap().contains(OFlags::NONBLOCK));
}

#[test]
fn at_path_names_the_file_and_keeps_the_kind() {
    let e =
        torque_resource::plain::at_path(Path::new("a/b.txt"), std::io::ErrorKind::NotFound.into());
    assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
    assert!(e.to_string().starts_with("a/b.txt: "), "{e}");
}
