//! A client's datagrams over a WebRTC data channel (the `webrtc` feature), beside its game socket: the browser's side of a
//! server's data channels (torque-server's `net::web::rtc`).
//!
//! [`WebRtcDatagrams`] holds a [`WebSocketDatagrams`] and starts on it. When the server says it takes data channels
//! ([`signal::HELLO`]), the client makes a peer connection with one channel (`game`, `ordered: false, maxRetransmits: 0`),
//! sends its offer over the socket and takes the answer back; once the channel opens, datagrams go on it both ways. If it
//! does not open within [`OPEN_TIMEOUT_MS`] (UDP blocked, ICE failing), or it closes, the client drops it, tells the
//! server ([`signal::CLOSED`]) and goes on over the socket, which never closes meanwhile. Datagrams from either link are
//! taken. A browser makes the peer connection with the page's `RTCPeerConnection`; natively (tests, tools) it is made
//! here over UDP.
//!
//! [`LinkWatch`] shows a page or a test which link carries the datagrams and how the channel's messages arrive.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use super::web_socket::WebSocketDatagrams;
use super::{Datagrams, MAX_INBOX, RECV_BYTES};

/// The signaling messages, as the server's (torque-server's `net::web::rtc::signal`, which a test compares).
pub mod signal {
    /// The server takes data channels.
    pub const HELLO: &str = "rtc";
    /// The client's SDP offer.
    pub const OFFER: &str = "offer";
    /// The server's SDP answer.
    pub const ANSWER: &str = "answer";
    /// The server takes no more data channels now.
    pub const BUSY: &str = "busy";
    /// The data channel is closed.
    pub const CLOSED: &str = "closed";
    /// The channel's label.
    pub const CHANNEL_LABEL: &str = "game";
}

/// Why a browser's link ended when its game socket closed ([`LinkState::ended`]): the data channel closes with it.
pub const SOCKET_CLOSED: &str = "The server closed the connection.";

/// How long a data channel has to open before the client stays on its game socket.
pub const OPEN_TIMEOUT_MS: u64 = 10_000;

/// Which link carries the datagrams.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Link {
    /// The game socket (no data channel yet, or none to be had).
    #[default]
    Socket,
    /// The game socket, while a data channel is being set up.
    Connecting,
    /// The data channel.
    Channel,
}

impl Link {
    pub fn name(self) -> &'static str {
        match self {
            Link::Socket => "websocket",
            Link::Connecting => "connecting",
            Link::Channel => "datachannel",
        }
    }
}

/// What a page or a test sees of a client's links.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LinkState {
    pub link: Link,
    /// Whether a data channel was tried (the server offered one).
    pub tried: bool,
    /// Whether one failed or was refused (the client is on its socket for good).
    pub failed: bool,
    /// Messages taken from the channel.
    pub messages_in: u64,
    /// Messages sent on the channel.
    pub messages_out: u64,
    /// The longest time between two of the channel's messages, in milliseconds.
    pub max_gap_ms: f64,
    /// When the channel's last message came (milliseconds on the link's clock).
    pub last_ms: Option<f64>,
    /// Why the link ended for good, in a player's words (a player's host's joiner, `room_join`: the host left, the room
    /// refused, the host could not be reached; a browser on a game socket: [`SOCKET_CLOSED`]).
    pub ended: Option<String>,
}

/// A shared view of a client's links.
pub type LinkWatch = Rc<RefCell<LinkState>>;

/// Counts a message from the channel at `now_ms`.
fn heard(watch: &LinkWatch, now_ms: f64) {
    let mut w = watch.borrow_mut();
    if let Some(last) = w.last_ms {
        w.max_gap_ms = w.max_gap_ms.max(now_ms - last);
    }
    w.last_ms = Some(now_ms);
    w.messages_in += 1;
}

/// Keeps a received datagram (cut to [`RECV_BYTES`]; at most [`MAX_INBOX`] wait).
fn hold(held: &mut VecDeque<Vec<u8>>, data: &[u8]) {
    if held.len() < MAX_INBOX {
        held.push_back(
            data.get(..data.len().min(RECV_BYTES))
                .unwrap_or_default()
                .to_vec(),
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::WebRtcDatagrams;
#[cfg(target_arch = "wasm32")]
pub use web::WebRtcDatagrams;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::io::ErrorKind;
    use std::net::{SocketAddr, SocketAddrV4, UdpSocket};
    use std::sync::Arc;
    use std::time::Instant;

    use str0m::change::{SdpAnswer, SdpPendingOffer};
    use str0m::channel::{ChannelConfig, ChannelId, Reliability};
    use str0m::net::{Protocol, Receive};
    use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc, RtcConfig};

    use super::*;

    /// The most datagrams read off the channel's socket in one receive.
    const READS: usize = 64;
    /// The most outputs taken from the peer in one go.
    const OUTPUTS: usize = 256;

    struct Peer {
        rtc: Rtc,
        udp: UdpSocket,
        local: SocketAddr,
        pending: Option<SdpPendingOffer>,
        channel: Option<ChannelId>,
        started: Instant,
        buf: Vec<u8>,
    }

    /// Datagrams over a data channel when one opens, else over the game socket (module docs).
    pub struct WebRtcDatagrams {
        ws: WebSocketDatagrams,
        peer: Option<Peer>,
        watch: LinkWatch,
        held: VecDeque<Vec<u8>>,
        clock: Instant,
        open_timeout_ms: u64,
    }

    impl WebRtcDatagrams {
        /// Starts on `ws`; a data channel is tried when the server offers one.
        pub fn new(ws: WebSocketDatagrams) -> WebRtcDatagrams {
            WebRtcDatagrams {
                ws,
                peer: None,
                watch: LinkWatch::default(),
                held: VecDeque::new(),
                clock: Instant::now(),
                open_timeout_ms: OPEN_TIMEOUT_MS,
            }
        }

        /// How long the channel has to open (a test shortens it).
        pub fn set_open_timeout(&mut self, ms: u64) {
            self.open_timeout_ms = ms;
        }

        /// The view of the links.
        pub fn watch(&self) -> LinkWatch {
            Rc::clone(&self.watch)
        }

        /// Whether datagrams go on the data channel.
        pub fn on_channel(&self) -> bool {
            self.peer.as_ref().is_some_and(|p| p.channel.is_some())
        }

        /// The game socket (a test sends on it directly).
        pub fn socket_mut(&mut self) -> &mut WebSocketDatagrams {
            &mut self.ws
        }

        /// The channel's socket address (none: no data channel being made).
        pub fn channel_addr(&self) -> Option<SocketAddr> {
            self.peer.as_ref().map(|p| p.local)
        }

        /// Sends `bytes` on the data channel without looking at their size or kind (a test of the server's caps).
        pub fn send_channel_raw(&mut self, bytes: &[u8], binary: bool) -> bool {
            let Some(peer) = self.peer.as_mut() else {
                return false;
            };
            let Some(id) = peer.channel else {
                return false;
            };
            let sent = peer
                .rtc
                .channel(id)
                .is_some_and(|mut c| c.write(binary, bytes).unwrap_or(false));
            self.drain();
            sent
        }

        /// Opens a second channel on the peer connection (a test: the server allows one).
        pub fn open_second_channel(&mut self) -> bool {
            let Some(peer) = self.peer.as_mut() else {
                return false;
            };
            let mut api = peer.rtc.direct_api();
            api.create_data_channel(ChannelConfig {
                label: "extra".into(),
                ordered: false,
                reliability: Reliability::MaxRetransmits { retransmits: 0 },
                negotiated: None,
                protocol: String::new(),
            });
            self.drain();
            true
        }

        fn now_ms(&self) -> f64 {
            self.clock.elapsed().as_secs_f64() * 1000.0
        }

        fn fail(&mut self, tell: bool) {
            if self.peer.take().is_some() && tell {
                self.ws.send_text(signal::CLOSED);
            }
            let mut w = self.watch.borrow_mut();
            w.link = Link::Socket;
            w.failed = true;
        }

        fn signals(&mut self) {
            while let Some(text) = self.ws.take_text() {
                let (kind, body) = text.split_once('\n').unwrap_or((&text, ""));
                match kind {
                    signal::HELLO if self.peer.is_none() && !self.watch.borrow().tried => {
                        self.watch.borrow_mut().tried = true;
                        if self.start().is_none() {
                            self.fail(false);
                        }
                    }
                    signal::ANSWER => {
                        let ok = self.peer.as_mut().is_some_and(|p| {
                            let Some(pending) = p.pending.take() else {
                                return false;
                            };
                            SdpAnswer::from_sdp_string(body)
                                .is_ok_and(|a| p.rtc.sdp_api().accept_answer(pending, a).is_ok())
                        });
                        if !ok {
                            self.fail(true);
                        }
                    }
                    signal::BUSY | signal::CLOSED => self.fail(false),
                    _ => {}
                }
            }
        }

        fn start(&mut self) -> Option<()> {
            let ip = self.ws.local_ip()?;
            let udp = UdpSocket::bind(SocketAddr::new(ip, 0)).ok()?;
            udp.set_nonblocking(true).ok()?;
            let local = udp.local_addr().ok()?;
            let now = Instant::now();
            let mut rtc = RtcConfig::new()
                .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
                .set_stats_interval(None)
                .clear_codecs()
                .build(now);
            rtc.add_local_candidate(Candidate::host(local, "udp").ok()?);
            let mut api = rtc.sdp_api();
            api.add_channel_with_config(ChannelConfig {
                label: signal::CHANNEL_LABEL.into(),
                ordered: false,
                reliability: Reliability::MaxRetransmits { retransmits: 0 },
                negotiated: None,
                protocol: String::new(),
            });
            let (offer, pending) = api.apply()?;
            let text = format!("{}\n{}", signal::OFFER, offer.to_sdp_string());
            if !self.ws.send_text(&text) {
                return None;
            }
            self.peer = Some(Peer {
                rtc,
                udp,
                local,
                pending: Some(pending),
                channel: None,
                started: now,
                buf: vec![0; 2048],
            });
            self.watch.borrow_mut().link = Link::Connecting;
            Some(())
        }

        fn drive(&mut self) {
            let Some(peer) = self.peer.as_mut() else {
                return;
            };
            let now = Instant::now();
            for _ in 0..READS {
                match peer.udp.recv_from(&mut peer.buf) {
                    Ok((n, from)) => {
                        let Some(data) = peer.buf.get(..n) else {
                            continue;
                        };
                        if let Ok(r) = Receive::new(Protocol::Udp, from, peer.local, data) {
                            let _ = peer.rtc.handle_input(Input::Receive(now, r));
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => continue,
                }
            }
            let _ = peer.rtc.handle_input(Input::Timeout(now));
            let late = peer.channel.is_none()
                && now.duration_since(peer.started).as_millis() > u128::from(self.open_timeout_ms);
            if late {
                self.fail(true);
                return;
            }
            self.drain();
        }

        fn drain(&mut self) {
            let now_ms = self.now_ms();
            let Some(peer) = self.peer.as_mut() else {
                return;
            };
            let mut dead = false;
            for _ in 0..OUTPUTS {
                match peer.rtc.poll_output() {
                    Ok(Output::Timeout(_)) => break,
                    Ok(Output::Transmit(t)) => {
                        let _ = peer.udp.send_to(&t.contents, t.destination);
                    }
                    Ok(Output::Event(Event::ChannelOpen(id, label))) => {
                        if label == signal::CHANNEL_LABEL && peer.channel.is_none() {
                            peer.channel = Some(id);
                            self.watch.borrow_mut().link = Link::Channel;
                        }
                    }
                    Ok(Output::Event(Event::ChannelData(d))) => {
                        if Some(d.id) == peer.channel && d.binary {
                            heard(&self.watch, now_ms);
                            hold(&mut self.held, &d.data);
                        }
                    }
                    Ok(Output::Event(
                        Event::ChannelClose(_)
                        | Event::Closed
                        | Event::IceConnectionStateChange(IceConnectionState::Disconnected),
                    )) => {
                        dead = true;
                        break;
                    }
                    Ok(Output::Event(_)) => {}
                    Err(_) => {
                        dead = true;
                        break;
                    }
                }
            }
            if dead {
                self.fail(true);
            }
        }
    }

    impl Datagrams for WebRtcDatagrams {
        fn send_to(&mut self, to: SocketAddrV4, bytes: &[u8]) -> bool {
            if to != self.ws.server() {
                return false;
            }
            if let Some(peer) = self.peer.as_mut()
                && let Some(id) = peer.channel
            {
                let sent = peer
                    .rtc
                    .channel(id)
                    .is_some_and(|mut c| c.write(true, bytes).unwrap_or(false));
                if sent {
                    self.watch.borrow_mut().messages_out += 1;
                }
                self.drain();
                return sent;
            }
            self.ws.send_to(to, bytes)
        }

        fn recv(&mut self) -> Option<(SocketAddrV4, Vec<u8>)> {
            self.signals();
            if self.held.is_empty() {
                self.drive();
            }
            if let Some(data) = self.held.pop_front() {
                return Some((self.ws.server(), data));
            }
            self.ws.recv()
        }

        fn port(&self) -> u16 {
            self.ws.port()
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::net::SocketAddrV4;

    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        MessageEvent, RtcDataChannel, RtcDataChannelInit, RtcDataChannelState, RtcDataChannelType,
        RtcIceConnectionState, RtcPeerConnection, RtcSdpType, RtcSessionDescriptionInit,
    };

    use super::*;

    /// What the page's callbacks share with the client.
    #[derive(Default)]
    struct Shared {
        held: VecDeque<Vec<u8>>,
        /// The offer made, to be sent.
        offer: Option<String>,
        open: bool,
        dead: bool,
    }

    struct Peer {
        pc: RtcPeerConnection,
        channel: RtcDataChannel,
        started: f64,
        _on_message: Closure<dyn FnMut(MessageEvent)>,
        _on_open: Closure<dyn FnMut()>,
        _on_close: Closure<dyn FnMut()>,
        _on_ice: Closure<dyn FnMut()>,
    }

    impl Drop for Peer {
        fn drop(&mut self) {
            self.channel.set_onmessage(None);
            self.channel.set_onopen(None);
            self.channel.set_onclose(None);
            self.pc.set_oniceconnectionstatechange(None);
            self.channel.close();
            self.pc.close();
        }
    }

    /// Datagrams over a data channel when one opens, else over the game socket (module docs).
    pub struct WebRtcDatagrams {
        ws: WebSocketDatagrams,
        peer: Option<Peer>,
        shared: Rc<RefCell<Shared>>,
        watch: LinkWatch,
    }

    fn now_ms() -> f64 {
        js_sys::Date::now()
    }

    impl WebRtcDatagrams {
        /// Starts on `ws`; a data channel is tried when the server offers one.
        pub fn new(ws: WebSocketDatagrams) -> WebRtcDatagrams {
            WebRtcDatagrams {
                ws,
                peer: None,
                shared: Rc::default(),
                watch: LinkWatch::default(),
            }
        }

        /// The view of the links.
        pub fn watch(&self) -> LinkWatch {
            Rc::clone(&self.watch)
        }

        /// Whether datagrams go on the data channel.
        pub fn on_channel(&self) -> bool {
            self.peer.is_some() && self.shared.borrow().open
        }

        fn fail(&mut self, tell: bool) {
            if self.peer.take().is_some() && tell {
                self.ws.send_text(signal::CLOSED);
            }
            *self.shared.borrow_mut() = Shared::default();
            let mut w = self.watch.borrow_mut();
            w.link = Link::Socket;
            w.failed = true;
        }

        fn signals(&mut self) {
            while let Some(text) = self.ws.take_text() {
                let (kind, body) = text.split_once('\n').unwrap_or((&text, ""));
                match kind {
                    signal::HELLO if self.peer.is_none() && !self.watch.borrow().tried => {
                        self.watch.borrow_mut().tried = true;
                        if self.start().is_err() {
                            self.fail(false);
                        }
                    }
                    signal::ANSWER => {
                        let Some(peer) = self.peer.as_ref() else {
                            continue;
                        };
                        let answer = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
                        answer.set_sdp(body);
                        let pc = peer.pc.clone();
                        let shared = Rc::clone(&self.shared);
                        wasm_bindgen_futures::spawn_local(async move {
                            let set = JsFuture::from(pc.set_remote_description(&answer)).await;
                            if set.is_err() {
                                shared.borrow_mut().dead = true;
                            }
                        });
                    }
                    signal::BUSY | signal::CLOSED => self.fail(false),
                    _ => {}
                }
            }
        }

        fn start(&mut self) -> Result<(), wasm_bindgen::JsValue> {
            let pc = RtcPeerConnection::new()?;
            let init = RtcDataChannelInit::new();
            init.set_ordered(false);
            init.set_max_retransmits(0);
            let channel =
                pc.create_data_channel_with_data_channel_dict(signal::CHANNEL_LABEL, &init);
            channel.set_binary_type(RtcDataChannelType::Arraybuffer);
            let shared = Rc::clone(&self.shared);
            let watch = Rc::clone(&self.watch);
            let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                let Ok(buffer) = e.data().dyn_into::<js_sys::ArrayBuffer>() else {
                    return;
                };
                let array = js_sys::Uint8Array::new(&buffer);
                let len = (array.length() as usize).min(RECV_BYTES);
                let mut data = vec![0u8; len];
                array.subarray(0, len as u32).copy_to(&mut data);
                heard(&watch, now_ms());
                hold(&mut shared.borrow_mut().held, &data);
            });
            channel.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            let shared = Rc::clone(&self.shared);
            let on_open = Closure::<dyn FnMut()>::new(move || shared.borrow_mut().open = true);
            channel.set_onopen(Some(on_open.as_ref().unchecked_ref()));
            let shared = Rc::clone(&self.shared);
            let on_close = Closure::<dyn FnMut()>::new(move || shared.borrow_mut().dead = true);
            channel.set_onclose(Some(on_close.as_ref().unchecked_ref()));
            let shared = Rc::clone(&self.shared);
            let ice_pc = pc.clone();
            let on_ice = Closure::<dyn FnMut()>::new(move || {
                if matches!(
                    ice_pc.ice_connection_state(),
                    RtcIceConnectionState::Failed
                        | RtcIceConnectionState::Disconnected
                        | RtcIceConnectionState::Closed
                ) {
                    shared.borrow_mut().dead = true;
                }
            });
            pc.set_oniceconnectionstatechange(Some(on_ice.as_ref().unchecked_ref()));
            let offer_pc = pc.clone();
            let shared = Rc::clone(&self.shared);
            wasm_bindgen_futures::spawn_local(async move {
                let made = async {
                    let offer = JsFuture::from(offer_pc.create_offer()).await?;
                    let offer: RtcSessionDescriptionInit = offer.unchecked_into();
                    JsFuture::from(offer_pc.set_local_description(&offer)).await?;
                    offer_pc
                        .local_description()
                        .map(|d| d.sdp())
                        .ok_or(wasm_bindgen::JsValue::NULL)
                }
                .await;
                let mut s = shared.borrow_mut();
                match made {
                    Ok(sdp) => s.offer = Some(sdp),
                    Err(_) => s.dead = true,
                }
            });
            self.peer = Some(Peer {
                pc,
                channel,
                started: now_ms(),
                _on_message: on_message,
                _on_open: on_open,
                _on_close: on_close,
                _on_ice: on_ice,
            });
            self.watch.borrow_mut().link = Link::Connecting;
            Ok(())
        }

        fn step(&mut self) {
            // The game socket closed (the server stopped or closed it): the data channel goes with it, so the link is over.
            if !self.ws.is_open() && self.watch.borrow().ended.is_none() {
                self.watch.borrow_mut().ended = Some(SOCKET_CLOSED.to_string());
            }
            self.signals();
            let Some(peer) = self.peer.as_ref() else {
                return;
            };
            let (offer, open, dead) = {
                let mut s = self.shared.borrow_mut();
                (s.offer.take(), s.open, s.dead)
            };
            let late = !open && now_ms() - peer.started > OPEN_TIMEOUT_MS as f64;
            if dead || late {
                self.fail(true);
                return;
            }
            if let Some(sdp) = offer {
                self.ws.send_text(&format!("{}\n{sdp}", signal::OFFER));
            }
            if open {
                self.watch.borrow_mut().link = Link::Channel;
            }
        }
    }

    impl Datagrams for WebRtcDatagrams {
        fn send_to(&mut self, to: SocketAddrV4, bytes: &[u8]) -> bool {
            if to != self.ws.server() {
                return false;
            }
            self.step();
            if let Some(peer) = self.peer.as_ref()
                && self.shared.borrow().open
                && peer.channel.ready_state() == RtcDataChannelState::Open
            {
                let sent = peer.channel.send_with_u8_array(bytes).is_ok();
                if sent {
                    self.watch.borrow_mut().messages_out += 1;
                }
                return sent;
            }
            self.ws.send_to(to, bytes)
        }

        fn recv(&mut self) -> Option<(SocketAddrV4, Vec<u8>)> {
            self.step();
            if let Some(data) = self.shared.borrow_mut().held.pop_front() {
                return Some((self.ws.server(), data));
            }
            self.ws.recv()
        }

        fn port(&self) -> u16 {
            self.ws.port()
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::signal;
    use torque_server::net::web::rtc::signal as server;

    #[test]
    fn the_signals_are_the_servers() {
        assert_eq!(signal::HELLO, server::HELLO);
        assert_eq!(signal::OFFER, server::OFFER);
        assert_eq!(signal::ANSWER, server::ANSWER);
        assert_eq!(signal::BUSY, server::BUSY);
        assert_eq!(signal::CLOSED, server::CLOSED);
        assert_eq!(signal::CHANNEL_LABEL, server::CHANNEL_LABEL);
    }
}
