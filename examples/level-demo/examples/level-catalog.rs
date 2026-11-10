//! The office's prop catalog, headless: a server that requires the catalog mod of its clients, two clients that download it, allow
//! its menu and spawn props from it (see `src/catalog.rs`).
//!
//! `cargo run -p torque-example-level-demo --release --example level-catalog` plays it and prints what each client shows;
//! `-- --shots <dir>` draws the prompt, the catalog and a spawned chair from the first client instead (offscreen, no window).
//! It needs the realm guest: `just lua-client-web` builds it, `TORQUE_CLIENT_GUEST` names one built already.

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), String> {
    use std::path::PathBuf;

    use torque_example_level_demo::catalog::{Catalog, guest_path, load_guest, write_shots};
    use torque_plugin_client_script::RealmState;

    let mut args = std::env::args().skip(1);
    let mut shots = None;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--shots" => shots = Some(PathBuf::from(args.next().ok_or("--shots needs a folder")?)),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let guest = load_guest(&guest_path())?;
    let work = PathBuf::from("target/level-catalog");
    if let Some(dir) = shots {
        for path in write_shots(&guest, &work, &dir)? {
            println!("picture: {}", path.display());
        }
        return Ok(());
    }
    let mut catalog = Catalog::new(&guest, &work)?;
    catalog.start_office()?;
    let asked = |c: &mut Catalog| (0..2).all(|n| c.realms[n].state() == RealmState::Asking);
    if !catalog.run_until(6_000, asked) {
        return Err("the clients were not asked".into());
    }
    for n in 0..2 {
        catalog.allow(n)?;
        println!("client {n}: the player allowed the server's scripts");
    }
    catalog.run_until(600, |c| !c.cards(1).is_empty());
    println!("the catalog: {:?}", catalog.cards(1));
    catalog
        .server_eval("ClientGroup.getObject(1).player.setTransform(\"-6 2 0.2 0 0 1 1.5708\");")?;
    catalog.run(12);
    catalog.spawn(1, "Chair", "Bob's chair")?;
    catalog.run(60);
    println!("client 1: {}", catalog.status(1));
    for n in 0..2 {
        println!("client {n}'s recent list: {:?}", catalog.recent(n));
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn main() {}
