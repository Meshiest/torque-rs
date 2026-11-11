//! A game in a browser and its page (`web/index.html`): the entry every web build shares.
//!
//! **The page's calls.** [`web_exports!`](crate::web_exports) gives a game's program the functions the page calls: whether
//! the module carries its files, the list's cap, `readManifest`, `addFile` and `addFileDigest` for the files the page fetches
//! by `files.json` ([`read_manifest`], [`add_file`], [`add_file_digest`]: a file is taken only when its size and hash are the
//! list's; `addFileDigest` takes the hash the page computed with the browser's crypto, off the module's thread), the game's
//! start (on the files fetched, [`take_fetched`], or on those the module carries), the page bridge's
//! ([`crate::page_bridge`]), and the page's WebTransport session to the server that served it (`webTransportOpened`,
//! `webTransportDatagram`, `webTransportEnded`, from `web/webtransport.js`: `web_transport_opened` and the two with it,
//! which hand it to `web_join` in a build with the `p2p` feature and do nothing in one without).
//!
//! **The quality** is the page's to pick (its settings panel; Firefox starts at Medium until the player picks): the panel calls
//! the script engine's `setRenderQuality` through the bridge ([`request_quality`]), and the window applies it at its next
//! frame ([`take_quality`]).
//!
//! **The page's displays.** The window writes on the page's root element the loading stage (`data-torque-stage`: `gpu` while the
//! canvas's device is asked for, `map` while the game says it is loading ([`crate::GameApp::loading`]) or the view is still
//! making its sky's clouds ([`crate::view::WorldView::clouds_waiting`]), then `ready`) and, every
//! [`FrameTimes::EVERY`], the frames a second (`data-torque-fps`) and the mean and longest time between two frames in ms
//! (`data-torque-frame-ms`, `mean max`), the quality last applied (`data-torque-quality`), and the size the game draws and
//! lays its GUI out at (`data-torque-size`, `WxH`: the canvas's device pixels under the page's render scale, the client's
//! `platform::web_input::RenderScale`). A game marks what a check waits on with `mark_play`.

use std::cell::{Cell, RefCell};

use torque_render_scene::hd::Quality;
use web_time::{Duration, Instant};

use crate::manifest::Manifest;

/// The pack's file name in a web build's folder.
pub const WEB_PACK: &str = "game.pack";

/// The files fetched so far and the list they were fetched by.
struct Fetched {
    manifest: Manifest,
    files: Vec<(String, Vec<u8>)>,
}

thread_local! {
    static FETCHED: RefCell<Option<Fetched>> = const { RefCell::new(None) };
    static QUALITY: Cell<Option<Quality>> = const { Cell::new(None) };
}

/// Reads the list of the files to fetch (Err: it is not one, or it breaks a cap), forgetting any files added before.
pub fn read_manifest(text: &str) -> Result<(), String> {
    let manifest = Manifest::parse(text)?;
    FETCHED.with_borrow_mut(|f| {
        *f = Some(Fetched {
            manifest,
            files: Vec::new(),
        })
    });
    Ok(())
}

/// Takes the fetched file `path` (Err, the file left out: the list does not name it, or its size or hash is not the list's).
pub fn add_file(path: &str, bytes: Vec<u8>) -> Result<(), String> {
    take(path, bytes, |entry, bytes| entry.check(bytes))
}

/// Takes the fetched file `path` like [`add_file`], on `digest`, its SHA-256 computed by the caller (the page, with the
/// browser's crypto, off the module's thread), instead of hashing the bytes here. The size, the path and the list's hash are
/// checked as before, so the list stays the one source of truth; the digest is trusted only as the page's own hash of the
/// bytes it holds, and must never be a value read from the network or the cache.
pub fn add_file_digest(path: &str, bytes: Vec<u8>, digest: &[u8]) -> Result<(), String> {
    take(path, bytes, |entry, bytes| {
        entry.check_digest(bytes.len(), digest)
    })
}

fn take(
    path: &str,
    bytes: Vec<u8>,
    check: impl FnOnce(&crate::manifest::ManifestFile, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    FETCHED.with_borrow_mut(|f| {
        let f = f.as_mut().ok_or("no list of files was read")?;
        let entry = f
            .manifest
            .file(path)
            .ok_or_else(|| format!("{path} is not in the list of files"))?;
        check(entry, &bytes)?;
        let path = entry.path.clone();
        f.files.retain(|(p, _)| *p != path);
        f.files.push((path, bytes));
        Ok(())
    })
}

/// The fetched files, by their paths (Err: no list was read, or a file it names was not fetched); the list is forgotten.
pub fn take_fetched() -> Result<Vec<(String, Vec<u8>)>, String> {
    let Fetched { manifest, files } = FETCHED.take().ok_or("no list of files was read")?;
    if let Some(missing) = manifest
        .files
        .iter()
        .find(|m| !files.iter().any(|(p, _)| *p == m.path))
    {
        return Err(format!("{} was not fetched", missing.path));
    }
    Ok(files)
}

/// The page asks for the quality `name` (`low`, `medium`, `high` or `ultra`, any case); the window applies it at its next frame
/// (Err: no such quality).
pub fn request_quality(name: &str) -> Result<(), String> {
    let quality = Quality::named(name.trim()).ok_or_else(|| format!("no quality {name:?}"))?;
    QUALITY.set(Some(quality));
    Ok(())
}

/// The quality the page asked for since the last call.
pub fn take_quality() -> Option<Quality> {
    QUALITY.take()
}

/// Writes a web build's files into `dir`: `pack` as [`WEB_PACK`] and the list naming it (`files.json`). The pack's path.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_web_files(dir: &std::path::Path, pack: &[u8]) -> Result<std::path::PathBuf, String> {
    let list = Manifest::of([(WEB_PACK, pack)])?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join(WEB_PACK);
    std::fs::write(&file, pack).map_err(|e| format!("{}: {e}", file.display()))?;
    let names = dir.join(crate::manifest::FILE_NAME);
    std::fs::write(&names, list.to_json()).map_err(|e| format!("{}: {e}", names.display()))?;
    Ok(file)
}

/// The time between frames, summed up every [`FrameTimes::EVERY`].
#[derive(Debug, Clone)]
pub struct FrameTimes {
    last: Option<Instant>,
    since: Option<Instant>,
    frames: u32,
    sum: Duration,
    max: Duration,
}

/// A summary of the frames since the last one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameSummary {
    pub fps: f64,
    /// The mean and the longest time between two frames (ms).
    pub mean_ms: f64,
    pub max_ms: f64,
}

impl Default for FrameTimes {
    fn default() -> FrameTimes {
        FrameTimes::new()
    }
}

impl FrameTimes {
    /// How often a summary is made.
    pub const EVERY: Duration = Duration::from_millis(500);

    pub fn new() -> FrameTimes {
        FrameTimes {
            last: None,
            since: None,
            frames: 0,
            sum: Duration::ZERO,
            max: Duration::ZERO,
        }
    }

    /// A frame was presented at `now`: the summary when [`FrameTimes::EVERY`] has passed since the last one.
    pub fn frame(&mut self, now: Instant) -> Option<FrameSummary> {
        let Some(last) = self.last.replace(now) else {
            self.since = Some(now);
            return None;
        };
        let gap = now.saturating_duration_since(last);
        self.frames += 1;
        self.sum += gap;
        self.max = self.max.max(gap);
        let since = *self.since.get_or_insert(last);
        let span = now.saturating_duration_since(since);
        if span < Self::EVERY {
            return None;
        }
        let summary = FrameSummary {
            fps: f64::from(self.frames) / span.as_secs_f64(),
            mean_ms: self.sum.as_secs_f64() * 1000.0 / f64::from(self.frames),
            max_ms: self.max.as_secs_f64() * 1000.0,
        };
        self.since = Some(now);
        self.frames = 0;
        self.sum = Duration::ZERO;
        self.max = Duration::ZERO;
        Some(summary)
    }
}

/// The value of `name` in a query string (`?a=1&b=2`).
pub fn query_value(query: &str, name: &str) -> Option<String> {
    query
        .trim_start_matches('?')
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// In a browser: a panic shown in the page's console.
#[cfg(target_arch = "wasm32")]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// In a browser: the value of `name` in the page's query string.
#[cfg(target_arch = "wasm32")]
pub fn query(name: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    query_value(&search, name)
}

/// In a browser: `value` on the page's root element as `data-torque-<name>`.
#[cfg(target_arch = "wasm32")]
pub fn mark(name: &str, value: &str) {
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        let _ = root.set_attribute(&format!("data-torque-{name}"), value);
    }
}

/// In a browser: the frames' summary on the page.
#[cfg(target_arch = "wasm32")]
pub fn mark_frames(s: &FrameSummary) {
    mark("fps", &format!("{:.1}", s.fps));
    mark("frame-ms", &format!("{:.2} {:.1}", s.mean_ms, s.max_ms));
}

/// In a browser: what a check waits on, on the page: the frames drawn (`data-torque-frames`), the map being played
/// (`data-torque-map`, empty off it), whether the client holds its own figure (`data-torque-figure`, `1` or `0`) and where
/// (`data-torque-position`, `x y z`).
#[cfg(target_arch = "wasm32")]
pub fn mark_play(frames: u64, map: &str, figure: Option<[f32; 3]>) {
    mark("frames", &frames.to_string());
    mark("map", map);
    mark("figure", if figure.is_some() { "1" } else { "0" });
    let position = figure
        .map(|p| format!("{} {} {}", p[0], p[1], p[2]))
        .unwrap_or_default();
    mark("position", &position);
}

/// The functions a web build's page calls (`web/index.html`, `web/files.js`, `web/bridge.js`), in the game's program (the
/// module's exports must be the program's own): `embedded`, `manifestMaxBytes`, `startEmbedded`, `readManifest`, `addFile`,
/// `addFileDigest`, `startFetched` (and `prepare`, when the game names one), and the page bridge's `torqueRequest`, `torqueOn` and `torqueOff` ([`crate::page_bridge`]). `embedded` is whether the module carries its files; `start_embedded` and `start_fetched` start the game
/// (functions returning `Result<(), String>`). `prepare` is an async function the page awaits before `startFetched` (a game opens
/// the browser's storage there, which answers later than a call is made); it cannot fail the start. The program depends on
/// `wasm-bindgen` (and `wasm-bindgen-futures` with `prepare`).
///
/// A game that runs the Lua a server sends (`torque-plugin-client-script`) adds `client_guests:`, a function returning the
/// build's accepted realm guests as hexadecimal hashes (`torque_client_script_host::accepted_hex`): the module then exports
/// `acceptedClientGuests`, and a page built with `web/build.mjs --client-scripts` checks its copy of the guest against that
/// list before it starts the page's runner (`web/client-script.js`). A module without it never gets the runner.
///
/// `prepare:` and `client_guests:` may come in either order, after `start_fetched:`.
///
/// ```ignore
/// #[cfg(target_arch = "wasm32")]
/// torque::app::web_exports! {
///     embedded: false,
///     start_embedded: my_game::web::start_embedded,
///     start_fetched: my_game::web::start_fetched,
///     prepare: my_game::web::prepare, // optional
///     // Only for a game with server scripts:
///     client_guests: torque_client_script_host::accepted_hex,
/// }
/// ```
#[macro_export]
macro_rules! web_exports {
    (
        embedded: $embedded:expr,
        start_embedded: $start_embedded:path,
        start_fetched: $start_fetched:path,
        client_guests: $client_guests:path,
        prepare: $prepare:path $(,)?
    ) => {
        $crate::web_exports! {
            embedded: $embedded,
            start_embedded: $start_embedded,
            start_fetched: $start_fetched,
            prepare: $prepare,
            client_guests: $client_guests,
        }
    };
    (
        embedded: $embedded:expr,
        start_embedded: $start_embedded:path,
        start_fetched: $start_fetched:path
        $(, prepare: $prepare:path)?
        $(, client_guests: $client_guests:path)? $(,)?
    ) => {
        mod page {
            use wasm_bindgen::prelude::wasm_bindgen;

            $(
                /// The realm guests this build runs, as SHA-256 hashes in lower-case hexadecimal: the page runs its copy of
                /// the guest only if its hash is one of them.
                #[wasm_bindgen(js_name = acceptedClientGuests)]
                pub fn accepted_client_guests() -> Vec<String> {
                    $client_guests()
                }
            )?

            /// Whether the module carries its files; if not, the page fetches them by `files.json`.
            #[wasm_bindgen]
            pub fn embedded() -> bool {
                $embedded
            }

            /// The most bytes `files.json` may have.
            #[wasm_bindgen(js_name = manifestMaxBytes)]
            pub fn manifest_max_bytes() -> u32 {
                u32::try_from($crate::manifest::MAX_BYTES).unwrap_or(u32::MAX)
            }

            #[wasm_bindgen(js_name = startEmbedded)]
            pub fn start_embedded() -> Result<(), String> {
                $start_embedded()
            }

            #[wasm_bindgen(js_name = readManifest)]
            pub fn read_manifest(text: &str) -> Result<(), String> {
                $crate::web_page::read_manifest(text)
            }

            #[wasm_bindgen(js_name = addFile)]
            pub fn add_file(path: &str, bytes: Vec<u8>) -> Result<(), String> {
                $crate::web_page::add_file(path, bytes)
            }

            /// `addFile` on a SHA-256 the page computed (`crypto.subtle`), not hashed in the module.
            #[wasm_bindgen(js_name = addFileDigest)]
            pub fn add_file_digest(
                path: &str,
                bytes: Vec<u8>,
                digest: &[u8],
            ) -> Result<(), String> {
                $crate::web_page::add_file_digest(path, bytes, digest)
            }

            $(
                /// What the game needs of the page before `startFetched`.
                #[wasm_bindgen]
                pub async fn prepare() {
                    $prepare().await
                }
            )?

            /// The page's WebTransport session to the server that served it is open: the function that sends on it
            /// (called with a `Uint8Array`), and the largest datagram it takes.
            #[wasm_bindgen(js_name = webTransportOpened)]
            pub fn web_transport_opened(sender: wasm_bindgen::JsValue, max_datagram: u32) {
                $crate::web_page::web_transport_opened(sender, max_datagram)
            }

            /// A datagram for the client on the page's WebTransport session (false: dropped).
            #[wasm_bindgen(js_name = webTransportDatagram)]
            pub fn web_transport_datagram(bytes: &[u8]) -> bool {
                $crate::web_page::web_transport_datagram(bytes)
            }

            /// The page's WebTransport session ended, for good: why, in a player's words.
            #[wasm_bindgen(js_name = webTransportEnded)]
            pub fn web_transport_ended(why: &str) {
                $crate::web_page::web_transport_ended(why)
            }

            #[wasm_bindgen(js_name = startFetched)]
            pub fn start_fetched() -> Result<(), String> {
                $start_fetched()
            }

            /// The page bridge: a request of the page's (`torque.call`, `eval`, `getVar`, `setVar`).
            #[wasm_bindgen(js_name = torqueRequest)]
            pub fn torque_request(
                kind: &str,
                a: String,
                b: String,
                args: Vec<String>,
                resolve: wasm_bindgen::JsValue,
                reject: wasm_bindgen::JsValue,
            ) {
                $crate::page_bridge::page::request(kind, a, b, args, resolve, reject)
            }

            /// The page bridge: the page registers its handler `name` (`torque.on`).
            #[wasm_bindgen(js_name = torqueOn)]
            pub fn torque_on(name: &str) -> Result<(), String> {
                $crate::page_bridge::register_handler(name).map_err(|e| e.to_json())
            }

            /// The page bridge: the page drops its handler `name` (`torque.off`).
            #[wasm_bindgen(js_name = torqueOff)]
            pub fn torque_off(name: &str) {
                $crate::page_bridge::unregister_handler(name)
            }
        }
    };
}

/// The page's WebTransport session to its server is open ([`crate::web_join::web_transport_opened`]); `sender` is the page's
/// function that sends on it. Without the `p2p` feature a game joins no server from a page: nothing is kept.
#[cfg(target_arch = "wasm32")]
pub fn web_transport_opened(sender: wasm_bindgen::JsValue, max_datagram: u32) {
    #[cfg(feature = "p2p")]
    {
        use wasm_bindgen::JsCast as _;
        if let Ok(sender) = sender.dyn_into::<js_sys::Function>() {
            crate::web_join::web_transport_opened(sender, max_datagram);
        }
    }
    #[cfg(not(feature = "p2p"))]
    let _ = (sender, max_datagram);
}

/// A datagram for the client on the page's WebTransport session ([`crate::web_join::web_transport_datagram`]); false:
/// dropped, or no game here takes one.
#[cfg(target_arch = "wasm32")]
pub fn web_transport_datagram(bytes: &[u8]) -> bool {
    #[cfg(feature = "p2p")]
    {
        crate::web_join::web_transport_datagram(bytes)
    }
    #[cfg(not(feature = "p2p"))]
    {
        let _ = bytes;
        false
    }
}

/// The page's WebTransport session ended ([`crate::web_join::web_transport_ended`]).
#[cfg(target_arch = "wasm32")]
pub fn web_transport_ended(why: &str) {
    #[cfg(feature = "p2p")]
    crate::web_join::web_transport_ended(why);
    #[cfg(not(feature = "p2p"))]
    let _ = why;
}

#[cfg(test)]
#[path = "web_page_tests.rs"]
mod tests;
