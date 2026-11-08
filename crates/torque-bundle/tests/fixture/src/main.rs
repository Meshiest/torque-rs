//! Prints the game folder's greeting, read from beside the executable, and which features it was built with.

fn main() {
    let exe = std::env::current_exe().expect("the executable's path");
    let dir = exe.parent().expect("the executable's folder");
    let hello = std::fs::read_to_string(dir.join("game").join("hello.txt"))
        .expect("game/hello.txt beside the executable");
    println!("game says {}", hello.trim());
    if cfg!(feature = "gizmos") {
        println!("gizmos on");
    }
    if cfg!(feature = "dev") {
        println!("dev build");
    }
    #[cfg(feature = "leak")]
    println!("built in {}", env!("CARGO_MANIFEST_DIR"));
}
