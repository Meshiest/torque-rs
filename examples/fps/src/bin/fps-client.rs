//! The arena's client.
//!
//! `fps-client` opens the window on the main menu: Host starts a server in this process and joins it (for the local network
//! only, or public: every interface, so players outside the network can join when UDP 28000 is forwarded to this computer),
//! Join lists the games on the local network and takes an address to connect to by hand. `--bind <address>` hosts a LAN game on
//! that address instead of this computer's LAN address, `--name <name>` sets the player's name, `--shots <dir>` draws the review
//! pictures offscreen into `dir` (two clients over the loopback address, no window), and `--motion-shots <dir>` the pictures of
//! the figures' and the pistol's motion (side steps, the walk backward, the look, the falls, a shot, the reload, the pause
//! menu), `--menu-shots <dir>` the menus at 1280x720, 1920x1080 and 2560x1080; `--polish-shots <dir>` the pictures of a bolt's
//! flight frame by frame, the bolts' holes, a hit from both sides and aiming down the sights; `--polish2-shots <dir>` a shot's
//! flares and smoke frame by frame, a bolt's ribbon from the side, bolt holes at a wall's middle and edge and in the floor, and
//! the sights aimed (also at 1920 x 1080); `--hitbox-shots <dir>` the other figure's hitboxes drawn over it in its poses;
//! `--lit-particle-shots <dir>` the muzzle's smoke unlit and lit, in the sun and in shade. `--web-files <dir>` writes the files
//! a build for the web fetches into `dir`: the pack (`files::web_pack`) as `game.pack` and the list beside the page
//! (`files.json`, `torque::app::manifest`); `--web-pack <file>` writes the pack alone, for a build that carries it (the
//! `embed-pack` feature).
//!
//! Built for the web, it is the page's client (the library's `web` module): join-only, over a game socket to the server that
//! served the page, or the host player's or a lone player's, on a server in the page's worker; the page calls the functions
//! of `page`.

#[cfg(not(target_arch = "wasm32"))]
use torque::app::cli::{Args, unknown};
#[cfg(not(target_arch = "wasm32"))]
use torque::app::manifest::{self, Manifest};
#[cfg(not(target_arch = "wasm32"))]
use torque_example_fps::client::{ArenaGame, ClientOptions};
#[cfg(not(target_arch = "wasm32"))]
use torque_example_fps::files::{self, GameFiles};
#[cfg(not(target_arch = "wasm32"))]
use torque_example_fps::shots;

/// The pack's file name in a web build's folder.
#[cfg(not(target_arch = "wasm32"))]
const WEB_PACK: &str = "game.pack";

/// The pictures' size.
#[cfg(not(target_arch = "wasm32"))]
const SHOT_SIZE: (u32, u32) = (1280, 720);

#[cfg(target_arch = "wasm32")]
fn main() {
    torque_example_fps::web::init();
}

/// What the page calls (`web/index.html` and `web/files.js`): the library's `web` module.
#[cfg(target_arch = "wasm32")]
mod page {
    use torque::app::manifest;
    use torque_example_fps::{web, web_host};
    use wasm_bindgen::prelude::wasm_bindgen;

    /// Whether the module carries its files; if not, the page fetches them by `files.json`.
    #[wasm_bindgen]
    pub fn embedded() -> bool {
        web::embedded()
    }

    /// The most bytes `files.json` may have.
    #[wasm_bindgen(js_name = manifestMaxBytes)]
    pub fn manifest_max_bytes() -> u32 {
        u32::try_from(manifest::MAX_BYTES).unwrap_or(u32::MAX)
    }

    #[wasm_bindgen(js_name = startEmbedded)]
    pub fn start_embedded() -> Result<(), String> {
        web::start_embedded()
    }

    #[wasm_bindgen(js_name = readManifest)]
    pub fn read_manifest(text: &str) -> Result<(), String> {
        web::read_manifest(text)
    }

    #[wasm_bindgen(js_name = addFile)]
    pub fn add_file(path: &str, bytes: Vec<u8>) -> Result<(), String> {
        web::add_file(path, bytes)
    }

    #[wasm_bindgen(js_name = startFetched)]
    pub fn start_fetched() -> Result<(), String> {
        web::start_fetched()
    }

    /// The host player's game: the client, joined to the server in the page's worker over the local link.
    #[wasm_bindgen(js_name = startHostClient)]
    pub fn start_host_client() -> Result<(), String> {
        web::start_host_client()
    }

    /// The game played alone: as the host player's, on a page with no room.
    #[wasm_bindgen(js_name = startSoloClient)]
    pub fn start_solo_client() -> Result<(), String> {
        web::start_solo_client()
    }

    /// A client joined by an invite link: the page holds its data channel to the host.
    #[wasm_bindgen(js_name = startLinkClient)]
    pub fn start_link_client() -> Result<(), String> {
        web::start_link_client()
    }

    #[wasm_bindgen(js_name = linkOpened)]
    pub fn link_opened() {
        web::link_opened()
    }

    #[wasm_bindgen(js_name = linkEnded)]
    pub fn link_ended(why: &str) {
        web::link_ended(why)
    }

    /// A datagram for the client on the page's link (false: dropped).
    #[wasm_bindgen(js_name = localDatagram)]
    pub fn local_datagram(bytes: &[u8]) -> bool {
        web::local_datagram(bytes)
    }

    /// The page's function that posts the host player's datagrams to the worker.
    #[wasm_bindgen(js_name = setLocalSender)]
    pub fn set_local_sender(sender: js_sys::Function) {
        web::set_local_sender(sender)
    }

    /// In the worker: starts the server on the fetched files and `map` (empty: the first); returns the map.
    #[wasm_bindgen(js_name = hostStart)]
    pub fn host_start(map: &str) -> Result<String, String> {
        web_host::host_start(map)
    }

    #[wasm_bindgen(js_name = hostOpen)]
    pub fn host_open(id: u32, ip: &str) -> bool {
        web_host::host_open(id, ip)
    }

    #[wasm_bindgen(js_name = hostDatagram)]
    pub fn host_datagram(id: u32, bytes: &[u8]) -> bool {
        web_host::host_datagram(id, bytes)
    }

    #[wasm_bindgen(js_name = hostClose)]
    pub fn host_close(id: u32) {
        web_host::host_close(id)
    }

    #[wasm_bindgen(js_name = hostTick)]
    pub fn host_tick() -> js_sys::Array {
        web_host::host_tick()
    }

    #[wasm_bindgen(js_name = hostClosed)]
    pub fn host_closed() -> Vec<u32> {
        web_host::host_closed()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::from_env();
    let mut shots_dir = None;
    let mut motion_dir = None;
    let mut menu_dir = None;
    let mut polish_dir = None;
    let mut polish2_dir = None;
    let mut hitbox_dir = None;
    let mut lit_dir = None;
    let mut pack_file = None;
    let mut web_dir = None;
    let mut options = ClientOptions::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--shots" => shots_dir = Some(args.dir(&arg)?),
            "--motion-shots" => motion_dir = Some(args.dir(&arg)?),
            "--menu-shots" => menu_dir = Some(args.dir(&arg)?),
            "--polish-shots" => polish_dir = Some(args.dir(&arg)?),
            "--polish2-shots" => polish2_dir = Some(args.dir(&arg)?),
            "--hitbox-shots" => hitbox_dir = Some(args.dir(&arg)?),
            "--lit-particle-shots" => lit_dir = Some(args.dir(&arg)?),
            "--web-pack" => pack_file = Some(args.dir(&arg)?),
            "--web-files" => web_dir = Some(args.dir(&arg)?),
            "--bind" => options.host_address = args.ipv4(&arg)?,
            "--name" => options.name = args.value(&arg, "a name")?,
            _ => return Err(unknown(&arg).into()),
        }
    }
    if let Some(file) = pack_file {
        let pack = files::web_pack(&GameFiles::bundled()?);
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&file, &pack)?;
        println!("{} ({} bytes)", file.display(), pack.len());
        return Ok(());
    }
    if let Some(dir) = web_dir {
        let pack = files::web_pack(&GameFiles::bundled()?);
        let list = Manifest::of([(WEB_PACK, pack.as_slice())])?;
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(WEB_PACK), &pack)?;
        std::fs::write(dir.join(manifest::FILE_NAME), list.to_json())?;
        println!("{} ({} bytes)", dir.join(WEB_PACK).display(), pack.len());
        return Ok(());
    }
    if let Some(dir) = menu_dir {
        let sizes = [(1280, 720), (1920, 1080), (2560, 1080)];
        for path in shots::write_menus(&dir, &sizes, GameFiles::bundled)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    if let Some(dir) = lit_dir {
        for path in shots::write_lit_smoke(&dir, SHOT_SIZE, GameFiles::bundled)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    if let Some(dir) = hitbox_dir {
        for path in shots::write_hitboxes(&dir, SHOT_SIZE, GameFiles::bundled)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    if let Some(dir) = polish2_dir {
        for path in shots::write_polish2(&dir, SHOT_SIZE, GameFiles::bundled)?
            .into_iter()
            .chain(shots::write_sights(&dir, (1920, 1080), GameFiles::bundled)?)
        {
            println!("{}", path.display());
        }
        return Ok(());
    }
    if let Some(dir) = polish_dir {
        for path in shots::write_polish(&dir, SHOT_SIZE, GameFiles::bundled)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    if let Some(dir) = motion_dir {
        for path in shots::write_motion(&dir, SHOT_SIZE, GameFiles::bundled)? {
            println!("{}", path.display());
        }
        return Ok(());
    }
    match shots_dir {
        Some(dir) => {
            for path in shots::write_all(&dir, SHOT_SIZE, GameFiles::bundled)? {
                println!("{}", path.display());
            }
        }
        None => torque::app::window::run(ArenaGame::new(GameFiles::bundled()?, options)?)?,
    }
    Ok(())
}
