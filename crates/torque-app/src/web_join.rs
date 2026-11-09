//! A browser's way into a game (the `p2p` feature, built for the web): where the page's client finds its server, the
//! link it opens to it, and that link's state written on the page.
//!
//! **Where** ([`Target`]): a server that served the page (a dedicated server on the web: `ws://<the page's host>/ws`,
//! `wss://` on an `https` page, or `?server=<ws or wss URL>` when the page is served elsewhere; [`socket_url`]), a game a
//! player hosts, found by its code (`?room=<code>`) through the room service at `?rooms=<ws or wss URL>` (else the page's
//! own host at `/ws`: the service serves the page; [`rooms_url`]), the server in this page's Web Worker (the host
//! player's own game: [`LocalLink`], fed by [`local_datagram`]), or a player's host reached by an invite link, whose data
//! channel the page holds (`web/invites.js`: a [`PageLink`], fed by [`link_datagram`]; [`link_opened`] and [`link_ended`]
//! tell its state). The two links keep apart: the worker's datagrams never reach a client on the invite link.
//!
//! **Switching in place** ([`switch_to`]): a page played alone switches its running game between playing alone, an invite
//! link's game and hosting with links, with no new page (a page runs one game: its window's event loop is the page's
//! own). The page asks, the link the switch goes to starts afresh at once, and the game takes the request at its next
//! step ([`take_switch`]). A page with a room service switches the same way: to hosting with a room (`host`, the page's
//! server started first), and a game joins a room by its code itself (a [`Target::Room`] opened in place); its address
//! says what it plays ([`set_address`], with no new page).
//!
//! What the page hands the client on its link is hostile as on UDP: it is held in a [`HeldDatagrams`] (none over the
//! engine's most bytes for a datagram, the count and the bytes held capped).
//!
//! **The link** ([`Target::open`]): a game socket over WebSocket with a WebRTC data channel beside it when the server
//! offers one, every datagram on the data channel to a player's host, or the local link to the worker. The client names
//! its server [`SERVER`] whatever it is: each link carries datagrams to its own server only. A link's [`LinkWatch`] says
//! which way the datagrams go and why the link ended for good (the host left, the server closed the connection).
//!
//! **The page** ([`mark_link`]): the root element carries `data-torque-link` (`websocket`, `connecting` or
//! `datachannel`), `data-torque-rtc-tried` and `data-torque-rtc-failed` (`1` once a data channel was tried, or failed or
//! was refused), `data-torque-rtc-in` (the messages the channel brought), `data-torque-rtc-gap` (the longest time between
//! two of them, in milliseconds, since the window was last reset) and `data-torque-ended` (why the link ended, empty
//! while it holds).

use std::cell::{Cell, RefCell};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::rc::Rc;

use torque_client::net::Datagrams;
use torque_client::net::held::HeldDatagrams;
use torque_client::net::room_join::RoomDatagrams;
use torque_client::net::web_rtc::{Link, LinkWatch, WebRtcDatagrams};
use torque_client::net::web_socket::WebSocketDatagrams;

use crate::transports::{Connect, RelayTarget, RoomTarget, WebTarget};
use crate::web_page::mark;

/// The address the client names its server by: each link goes to its own server whatever it is sent to.
pub const SERVER: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_000);

/// What a link the page feeds holds: datagrams from the worker's server, or from a player's host on the page's data
/// channel, and the page's function that sends one.
#[derive(Default)]
struct Local {
    held: HeldDatagrams,
    sender: Option<js_sys::Function>,
}

type Queue = &'static std::thread::LocalKey<RefCell<Local>>;

thread_local! {
    /// The link to the server in the page's worker ([`Target::Local`]).
    static LOCAL: RefCell<Local> = RefCell::default();
    /// The link to a player's host the page holds ([`Target::Page`]).
    static LINK: RefCell<Local> = RefCell::default();
    /// The state of the link to a player's host the page holds: a new one for each switch to it.
    static PAGE_LINK: RefCell<LinkWatch> = RefCell::default();
    /// A switch the page asked for, not yet taken by the game.
    static SWITCH: Cell<Option<Mode>> = const { Cell::new(None) };
}

/// A datagram for the client on the link to the worker's server. False: dropped ([`HeldDatagrams`]: too long, or the
/// queue full).
pub fn local_datagram(bytes: &[u8]) -> bool {
    LOCAL.with_borrow_mut(|l| l.held.push(bytes))
}

/// A datagram for the client on the page's data channel to a player's host. False: dropped (as [`local_datagram`]).
pub fn link_datagram(bytes: &[u8]) -> bool {
    LINK.with_borrow_mut(|l| l.held.push(bytes))
}

/// The page's data channel to a player's host opened ([`Target::Page`]).
pub fn link_opened() {
    PAGE_LINK.with_borrow(|w| w.borrow_mut().link = Link::Channel);
}

/// The page's data channel to a player's host ended, for good: `why`, in a player's words (its first 200 characters),
/// is shown on the menu.
pub fn link_ended(why: &str) {
    PAGE_LINK.with_borrow(|w| {
        let mut w = w.borrow_mut();
        if w.ended.is_none() {
            w.ended = Some(why.chars().take(200).collect());
        }
    });
}

/// What a page switches its running game to ([`switch_to`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Played alone (the arena: on the server in the page's worker; the vehicle demo: at its menu, its game in the page).
    Solo,
    /// The game of a player's host reached by an invite link, once the page's data channel to it has opened.
    Link,
    /// The game this page hosts (with links, or with a room), its server in the page's worker.
    Host,
}

impl Mode {
    /// The mode by its name: `solo`, `link` or `host`.
    pub fn named(name: &str) -> Option<Mode> {
        match name {
            "solo" => Some(Mode::Solo),
            "link" => Some(Mode::Link),
            "host" => Some(Mode::Host),
            _ => None,
        }
    }
}

/// The page asks its running game to switch to `mode` (`solo`, `link` or `host`; Err: none of those). The link the switch
/// goes to starts afresh at once (to `link`: a new invite link's state, nothing held from an earlier one; else nothing
/// held from the worker), and the game switches at its next step ([`take_switch`]). A later ask replaces one not yet
/// taken.
pub fn switch_to(mode: &str) -> Result<(), String> {
    let mode = Mode::named(mode).ok_or_else(|| format!("no mode {mode:?}: solo, link or host"))?;
    if mode == Mode::Link {
        PAGE_LINK.set(LinkWatch::default());
        LINK.with_borrow_mut(|l| l.held = HeldDatagrams::default());
    } else {
        LOCAL.with_borrow_mut(|l| l.held = HeldDatagrams::default());
    }
    SWITCH.set(Some(mode));
    Ok(())
}

/// The switch the page asked for since the last call ([`switch_to`]).
pub fn take_switch() -> Option<Mode> {
    SWITCH.take()
}

/// Takes the link's state off the page (a game switched to one with no link of its own).
pub fn unmark_link() {
    if let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    {
        for name in [
            "link",
            "rtc-tried",
            "rtc-failed",
            "rtc-in",
            "rtc-gap",
            "ended",
        ] {
            let _ = root.remove_attribute(&format!("data-torque-{name}"));
        }
    }
}

/// Calls the page's `torqueAsk(what)`, if it has one: `host`, `join` or `back` only. A page played alone
/// (`web/invites.js`): its invite panels, or `back`, its own game again, played alone; any other page that switches its
/// game in place (`web/host.js` `roomPage`): `host`, it hosts a game with a room, and `back`, it hosts no more. False:
/// the page has none, or `what` is none of those.
pub fn ask_page(what: &str) -> bool {
    if !matches!(what, "host" | "join" | "back") {
        return false;
    }
    let Some(window) = web_sys::window() else {
        return false;
    };
    if let Ok(f) = js_sys::Reflect::get(&window, &"torqueAsk".into())
        && let Some(f) = wasm_bindgen::JsCast::dyn_ref::<js_sys::Function>(&f)
    {
        return f.call1(&wasm_bindgen::JsValue::NULL, &what.into()).is_ok();
    }
    false
}

/// The page's function that posts the client's datagrams to the worker's server (called with a `Uint8Array`).
pub fn set_local_sender(sender: js_sys::Function) {
    LOCAL.with_borrow_mut(|l| l.sender = Some(sender));
}

/// The page's function that sends the client's datagrams on its data channel to a player's host (called with a
/// `Uint8Array`).
pub fn set_link_sender(sender: js_sys::Function) {
    LINK.with_borrow_mut(|l| l.sender = Some(sender));
}

fn send_on(queue: Queue, to: SocketAddrV4, bytes: &[u8]) -> bool {
    to == SERVER
        && queue.with_borrow(|l| {
            l.sender.as_ref().is_some_and(|f| {
                f.call1(
                    &wasm_bindgen::JsValue::NULL,
                    &js_sys::Uint8Array::from(bytes),
                )
                .is_ok()
            })
        })
}

fn recv_on(queue: Queue) -> Option<(SocketAddrV4, Vec<u8>)> {
    queue.with_borrow_mut(|l| l.held.pop()).map(|d| (SERVER, d))
}

/// The host player's link to the server in the page's worker.
pub struct LocalLink;

impl Datagrams for LocalLink {
    fn send_to(&mut self, to: SocketAddrV4, bytes: &[u8]) -> bool {
        send_on(&LOCAL, to, bytes)
    }

    fn recv(&mut self) -> Option<(SocketAddrV4, Vec<u8>)> {
        recv_on(&LOCAL)
    }

    fn port(&self) -> u16 {
        0
    }

    /// The worker's link stands for a data channel, as the server's relay transport says.
    fn link_kind(&self) -> torque_net::engine::LinkKind {
        torque_net::engine::LinkKind::WEB_RTC
    }
}

/// A client's link to a player's host reached by an invite link: the page's data channel to it.
pub struct PageLink;

impl Datagrams for PageLink {
    fn send_to(&mut self, to: SocketAddrV4, bytes: &[u8]) -> bool {
        send_on(&LINK, to, bytes)
    }

    fn recv(&mut self) -> Option<(SocketAddrV4, Vec<u8>)> {
        recv_on(&LINK)
    }

    fn port(&self) -> u16 {
        0
    }

    /// The worker's link stands for a data channel, as the server's relay transport says.
    fn link_kind(&self) -> torque_net::engine::LinkKind {
        torque_net::engine::LinkKind::WEB_RTC
    }
}

/// Where the page's game is (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A server's web transport at this `ws://` or `wss://` URL.
    Server(String),
    /// A player's host, by its room's `code`, found through the room service at `url`.
    Room { url: String, code: String },
    /// The server in this page's worker.
    Local,
    /// A player's host the page reached by an invite link: the page holds the data channel.
    Page,
}

impl Target {
    /// A new link to the game (the join, and each join after).
    pub fn open(&self) -> Result<(Box<dyn Datagrams>, LinkWatch), String> {
        match self {
            Target::Server(url) => {
                let socket = WebRtcDatagrams::new(WebSocketDatagrams::open(url, SERVER)?);
                let watch = socket.watch();
                Ok((Box::new(socket), watch))
            }
            Target::Room { url, code } => {
                let socket = RoomDatagrams::open(url, code, SERVER)?;
                let watch = socket.watch();
                Ok((Box::new(socket), watch))
            }
            Target::Local => Ok((Box::new(LocalLink), LinkWatch::default())),
            Target::Page => {
                let watch = PAGE_LINK.with_borrow(Rc::clone);
                if watch.borrow().ended.is_some() {
                    return Err("open a new invite link to join again".into());
                }
                Ok((Box::new(PageLink), watch))
            }
        }
    }

    /// The id of the client transport that opens this target (`web`, `rooms` or `relay`).
    pub fn transport_id(&self) -> &'static str {
        match self {
            Target::Server(_) => "web",
            Target::Room { .. } => "rooms",
            Target::Local | Target::Page => "relay",
        }
    }

    /// Connects `client` to the game over the transport that opens this target (the engine's `web`, `rooms` or `relay`
    /// plugin, [`crate::transports`]), and gives the link's state. The same link as [`Target::open`], through the
    /// engine's transports: Err when the plugin is off.
    pub fn connect(&self, client: &mut impl Connect) -> Result<LinkWatch, String> {
        let info = match self {
            Target::Server(url) => client.connect_dyn("web", &WebTarget::Url(url.clone()))?,
            Target::Room { url, code } => client.connect_dyn(
                "rooms",
                &RoomTarget {
                    url: url.clone(),
                    code: code.clone(),
                },
            )?,
            Target::Local => client.connect_dyn("relay", &RelayTarget::Local)?,
            Target::Page => client.connect_dyn("relay", &RelayTarget::Page)?,
        };
        info.take::<LinkWatch>()
            .ok_or_else(|| format!("the {} transport gave no link state", self.transport_id()))
    }

    /// The game the page's address asks to join: `?room=<code>` a player's (Err: not a [`room_code`]), else with `?join` (or `?server=`) the server
    /// that served the page (or `?server=`'s); none asked, or the page is one to be played alone (its root element's
    /// `data-torque-solo="1"`: a site with no game server).
    pub fn from_page() -> Result<Option<Target>, String> {
        if solo_page() {
            return Ok(None);
        }
        if let Some(code) = query("room") {
            return Ok(Some(Target::Room {
                url: rooms_url()?,
                code: room_code(&code)?,
            }));
        }
        if query("join").is_some() || query("server").is_some() {
            return socket_url().map(|url| Some(Target::Server(url)));
        }
        Ok(None)
    }
}

/// Whether the page is one to be played alone (its root element's `data-torque-solo="1"`, `web/build.mjs --solo`: a site
/// with no game server, where nothing is hosted or joined).
pub fn solo_page() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|r| r.get_attribute("data-torque-solo"))
        .is_some_and(|v| v == "1")
}

/// The room code a host page shows (`data-torque-room`, `web/host.js`: `ABCD-EFGH`), if it has one yet and it is
/// letters, digits and `-` only.
pub fn page_room() -> Option<String> {
    let code = web_sys::window()?
        .document()?
        .document_element()?
        .get_attribute("data-torque-room")?;
    (!code.is_empty()
        && code.len() <= MAX_CODE
        && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    .then_some(code)
}

/// The page's own address's settings every mode keeps ([`set_address`]): the room service, the player's name, the
/// pointer-lock guard, the host's LAN rule and the map.
const KEPT: [&str; 5] = ["rooms", "name", "nolock", "lan", "map"];

/// The longest room code [`room_code`] takes (`ABCD-EFGH` with room to spare); a code is letters, digits and `-` only.
pub const MAX_CODE: usize = 16;

/// `code` as a room code, trimmed (Err: empty, over [`MAX_CODE`], or not letters, digits and `-` only).
pub fn room_code(code: &str) -> Result<String, String> {
    let code = code.trim();
    if code.is_empty()
        || code.len() > MAX_CODE
        || !code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(format!("{code:?} is not a room code (like ABCD-EFGH)"));
    }
    Ok(code.to_string())
}

/// What the page's address says the page plays ([`set_address`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Address<'a> {
    /// Its own game: none of the others.
    Own,
    /// A game it hosts (`?host=1`).
    Host,
    /// The game of a room, by its code (`?room=<code>`, a code [`room_code`] took).
    Room(&'a str),
}

/// Writes the mode the page plays in its address, with no new page (`history.replaceState`): the page opened on it
/// again plays the same, and a room's address can be handed to others. The address's [`KEPT`] settings stay, the rest
/// go. Err: a room code that is not one, or the address could not be written.
pub fn set_address(address: Address) -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let location = window.location();
    let search = location.search().map_err(|e| format!("{e:?}"))?;
    let mut pairs: Vec<String> = search
        .trim_start_matches('?')
        .split('&')
        .filter(|pair| KEPT.contains(&pair.split_once('=').map_or(*pair, |(k, _)| k)))
        .map(str::to_string)
        .collect();
    match address {
        Address::Own => {}
        Address::Host => pairs.push("host=1".to_string()),
        Address::Room(code) => pairs.push(format!("room={}", room_code(code)?)),
    }
    let path = location.pathname().map_err(|e| format!("{e:?}"))?;
    let url = if pairs.is_empty() {
        path
    } else {
        format!("{path}?{}", pairs.join("&"))
    };
    window
        .history()
        .and_then(|h| h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url)))
        .map_err(|e| format!("{e:?}"))
}

/// The value of `name` in the page's query string, decoded (`+` a space); a name with no `=` gives an empty value.
pub fn query(name: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let raw = search
        .trim_start_matches('?')
        .split('&')
        .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.replace('+', " "))?;
    js_sys::decode_uri_component(&raw)
        .ok()
        .and_then(|v| v.as_string())
}

/// The room service's URL: `?rooms=`, else the page's own host at `/ws`.
pub fn rooms_url() -> Result<String, String> {
    match query("rooms") {
        Some(url) if url.starts_with("ws://") || url.starts_with("wss://") => Ok(url),
        Some(url) => Err(format!(
            "?rooms= takes a ws:// or wss:// address, not {url:?}"
        )),
        None => page_socket_url(),
    }
}

/// The game socket's URL: `?server=`, else the page's own host at `/ws`.
pub fn socket_url() -> Result<String, String> {
    match query("server") {
        Some(url) if url.starts_with("ws://") || url.starts_with("wss://") => Ok(url),
        Some(url) => Err(format!(
            "?server= takes a ws:// or wss:// address, not {url:?}"
        )),
        None => page_socket_url(),
    }
}

/// The page's own host at `/ws` (`wss` on an `https` page).
fn page_socket_url() -> Result<String, String> {
    let location = web_sys::window().ok_or("no window")?.location();
    let protocol = location.protocol().map_err(|e| format!("{e:?}"))?;
    let host = location.host().map_err(|e| format!("{e:?}"))?;
    let scheme = if protocol == "https:" { "wss" } else { "ws" };
    // The server's web transport opens game sockets on this path (its `WEB_SOCKET_PATH`).
    Ok(format!("{scheme}://{host}/ws"))
}

/// The link's state on the page (module docs); `reset_gap` starts the longest gap's window again.
pub fn mark_link(watch: &LinkWatch, reset_gap: bool) {
    let mut link = watch.borrow_mut();
    let flag = |b: bool| if b { "1" } else { "0" };
    mark("link", link.link.name());
    mark("rtc-tried", flag(link.tried));
    mark("rtc-failed", flag(link.failed));
    mark("rtc-in", &link.messages_in.to_string());
    mark("rtc-gap", &format!("{:.0}", link.max_gap_ms));
    mark("ended", link.ended.as_deref().unwrap_or(""));
    if reset_gap {
        link.max_gap_ms = 0.0;
    }
}

/// Why `watch`'s link ended, as a script's string literal's text (quotes and backslashes made plain); none while it holds.
pub fn ended_text(watch: &LinkWatch) -> Option<String> {
    let text = watch.borrow().ended.clone()?;
    Some(text.replace(['"', '\\'], "'"))
}
