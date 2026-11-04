use super::*;

#[test]
fn a_path_that_could_leave_the_install_is_refused() {
    assert!(path_ok(b"data/shapes/crate.png"));
    assert!(path_ok(b"mods/pack/plate.png"));
    for bad in [
        &b""[..],
        b"/etc/passwd",
        b"../x.png",
        b"data/../../x",
        b"C:/x.png",
        b"data\\x.png",
        b"a//b",
        b"a/./b",
        b"a\0b",
    ] {
        assert!(!path_ok(bad), "{bad:?}");
    }
    assert!(!path_ok("a/".repeat(600).as_bytes()));
}
