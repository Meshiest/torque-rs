//! A client's datagrams to a player's host (the `p2p` feature): the joiner's side of torque-server's `net::web::p2p`.
//!
//! [`RoomDatagrams`] opens a socket to the room service, joins the room of a code, makes a peer connection with one data
//! channel (`game`, `ordered: false, maxRetransmits: 0`) using the ICE servers the room gives (STUN, and TURN with
//! short-lived credentials), and signals through the room: its offer and its candidates go to the host, the host's answer
//! and candidates come back. Once the channel is open the room socket is closed and every datagram goes on the channel;
//! none ever goes through the service, and before the channel opens a datagram sent is dropped (the game's connect
//! request is sent again). When the channel closes, ICE fails, or the room refuses, the link ends for good and says why
//! ([`LinkState::ended`]): the game ends with a message. Received datagrams are hostile as on UDP: each is cut to
//! [`RECV_BYTES`] and at most [`MAX_INBOX`] wait.
//!
//! In a browser the peer connection is the page's `RTCPeerConnection`; natively it is made here over UDP with str0m (not
//! yet).

#[cfg(target_arch = "wasm32")]
use std::collections::VecDeque;

#[cfg(any(target_arch = "wasm32", test))]
use super::web_rtc::LinkWatch;
#[cfg(target_arch = "wasm32")]
use super::{MAX_INBOX, RECV_BYTES};

/// How long the channel has to open after the room is joined.
pub const OPEN_TIMEOUT_MS: f64 = 20_000.0;
/// How long ICE may stay `disconnected` before the link ends (it often comes back on its own).
#[cfg(target_arch = "wasm32")]
const DISCONNECTED_MS: f64 = 5_000.0;
/// The most candidates sent to the host.
pub const MAX_CANDIDATES: usize = 16;
/// The most room messages kept waiting.
#[cfg(target_arch = "wasm32")]
const MAX_ROOM_MESSAGES: usize = 64;

/// What ended the link, for a player.
pub fn ended_text(why: &str) -> String {
    match why {
        "host-left" => "The host left the game.".into(),
        "no-room" => "No game has that code.".into(),
        "full" => "That game is full.".into(),
        "rate" | "busy" => "The room service is busy: try again in a minute.".into(),
        "timeout" | "unreachable" => "Could not reach the host.".into(),
        "dropped" => "The host turned the connection away.".into(),
        "service" => "Could not reach the room service.".into(),
        other => format!("The connection ended ({other})."),
    }
}

/// Keeps a received datagram (cut to [`RECV_BYTES`]; at most [`MAX_INBOX`] wait).
#[cfg(target_arch = "wasm32")]
fn hold(held: &mut VecDeque<Vec<u8>>, data: &[u8]) {
    if held.len() < MAX_INBOX {
        held.push_back(
            data.get(..data.len().min(RECV_BYTES))
                .unwrap_or_default()
                .to_vec(),
        );
    }
}

/// Marks the link ended for good with `why` (the first reason stays).
#[cfg(any(target_arch = "wasm32", test))]
fn end(watch: &LinkWatch, why: &str) {
    let mut w = watch.borrow_mut();
    if w.ended.is_none() {
        w.ended = Some(ended_text(why));
    }
    if w.link != super::web_rtc::Link::Channel {
        w.failed = true;
    }
    w.link = super::web_rtc::Link::Socket;
}

#[cfg(target_arch = "wasm32")]
pub use web::RoomDatagrams;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::net::SocketAddrV4;
    use std::rc::Rc;

    use torque_server::net::room::{FromService, MAX_MESSAGE, ToService};
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        BinaryType, MessageEvent, RtcConfiguration, RtcDataChannel, RtcDataChannelInit,
        RtcDataChannelState, RtcDataChannelType, RtcIceCandidateInit, RtcIceConnectionState,
        RtcPeerConnection, RtcPeerConnectionIceEvent, RtcSdpType, RtcSessionDescriptionInit,
        WebSocket,
    };

    use super::super::Datagrams;
    use super::super::web_rtc::{Link, signal};
    use super::*;

    /// What the page's callbacks share with the client.
    #[derive(Default)]
    struct Shared {
        room: VecDeque<String>,
        room_open: bool,
        room_closed: bool,
        held: VecDeque<Vec<u8>>,
        offer: Option<String>,
        candidates: VecDeque<String>,
        open: bool,
        dead: bool,
        /// When ICE went `disconnected`, while it stays so.
        disconnected: Option<f64>,
    }

    struct Peer {
        pc: RtcPeerConnection,
        channel: RtcDataChannel,
        _on_message: Closure<dyn FnMut(MessageEvent)>,
        _on_open: Closure<dyn FnMut()>,
        _on_close: Closure<dyn FnMut()>,
        _on_ice: Closure<dyn FnMut()>,
        _on_candidate: Closure<dyn FnMut(RtcPeerConnectionIceEvent)>,
    }

    impl Drop for Peer {
        fn drop(&mut self) {
            self.channel.set_onmessage(None);
            self.channel.set_onopen(None);
            self.channel.set_onclose(None);
            self.pc.set_oniceconnectionstatechange(None);
            self.pc.set_onicecandidate(None);
            self.channel.close();
            self.pc.close();
        }
    }

    struct Room {
        ws: WebSocket,
        _on_message: Closure<dyn FnMut(MessageEvent)>,
        _on_open: Closure<dyn FnMut()>,
        _on_close: Closure<dyn FnMut()>,
    }

    impl Drop for Room {
        fn drop(&mut self) {
            self.ws.set_onmessage(None);
            self.ws.set_onopen(None);
            self.ws.set_onclose(None);
            self.ws.set_onerror(None);
            let _ = self.ws.close();
        }
    }

    /// A player's datagrams to the host of a room (module docs).
    pub struct RoomDatagrams {
        server: SocketAddrV4,
        code: String,
        room: Option<Room>,
        joined: bool,
        asked: bool,
        peer: Option<Peer>,
        shared: Rc<RefCell<Shared>>,
        watch: LinkWatch,
        started: f64,
        candidates_sent: usize,
    }

    fn now_ms() -> f64 {
        js_sys::Date::now()
    }

    impl RoomDatagrams {
        /// Opens the room service's socket at `url` (`ws://` or `wss://`) to join the room of `code`; the link names the
        /// host `server`.
        pub fn open(url: &str, code: &str, server: SocketAddrV4) -> Result<RoomDatagrams, String> {
            let ws = WebSocket::new(url).map_err(|e| format!("{e:?}"))?;
            ws.set_binary_type(BinaryType::Arraybuffer);
            let shared: Rc<RefCell<Shared>> = Rc::default();
            let s = Rc::clone(&shared);
            let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                let Ok(buffer) = e.data().dyn_into::<js_sys::ArrayBuffer>() else {
                    return;
                };
                let array = js_sys::Uint8Array::new(&buffer);
                if array.length() as usize > MAX_MESSAGE {
                    return;
                }
                let mut s = s.borrow_mut();
                if s.room.len() < MAX_ROOM_MESSAGES
                    && let Ok(text) = String::from_utf8(array.to_vec())
                {
                    s.room.push_back(text);
                }
            });
            ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            let s = Rc::clone(&shared);
            let on_open = Closure::<dyn FnMut()>::new(move || s.borrow_mut().room_open = true);
            ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
            let s = Rc::clone(&shared);
            let on_close = Closure::<dyn FnMut()>::new(move || s.borrow_mut().room_closed = true);
            ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
            ws.set_onerror(Some(on_close.as_ref().unchecked_ref()));
            let watch = LinkWatch::default();
            watch.borrow_mut().tried = true;
            watch.borrow_mut().link = Link::Connecting;
            Ok(RoomDatagrams {
                server,
                code: code.to_string(),
                room: Some(Room {
                    ws,
                    _on_message: on_message,
                    _on_open: on_open,
                    _on_close: on_close,
                }),
                joined: false,
                asked: false,
                peer: None,
                shared,
                watch,
                started: now_ms(),
                candidates_sent: 0,
            })
        }

        /// The view of the link.
        pub fn watch(&self) -> LinkWatch {
            Rc::clone(&self.watch)
        }

        fn say(&self, message: &ToService<'_>) {
            if let Some(room) = self.room.as_ref() {
                let _ = room.ws.send_with_u8_array(message.write().as_bytes());
            }
        }

        fn fail(&mut self, why: &str) {
            end(&self.watch, why);
            self.peer = None;
            self.room = None;
        }

        fn ended(&self) -> bool {
            self.watch.borrow().ended.is_some()
        }

        fn start(&mut self, ice: &str) -> Result<(), wasm_bindgen::JsValue> {
            let config = RtcConfiguration::new();
            if let Ok(servers) = js_sys::JSON::parse(ice)
                && js_sys::Array::is_array(&servers)
            {
                config.set_ice_servers(&servers);
            }
            let pc = RtcPeerConnection::new_with_configuration(&config)?;
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
                {
                    let mut w = watch.borrow_mut();
                    let now = now_ms();
                    if let Some(last) = w.last_ms {
                        w.max_gap_ms = w.max_gap_ms.max(now - last);
                    }
                    w.last_ms = Some(now);
                    w.messages_in += 1;
                }
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
            // `disconnected` can pass (ICE moving to another pair): only one that lasts ends the link.
            let on_ice = Closure::<dyn FnMut()>::new(move || {
                let mut s = shared.borrow_mut();
                match ice_pc.ice_connection_state() {
                    RtcIceConnectionState::Failed | RtcIceConnectionState::Closed => s.dead = true,
                    RtcIceConnectionState::Disconnected => {
                        s.disconnected.get_or_insert(now_ms());
                    }
                    _ => s.disconnected = None,
                }
            });
            pc.set_oniceconnectionstatechange(Some(on_ice.as_ref().unchecked_ref()));
            let shared = Rc::clone(&self.shared);
            let on_candidate = Closure::<dyn FnMut(RtcPeerConnectionIceEvent)>::new(
                move |e: RtcPeerConnectionIceEvent| {
                    let Some(candidate) = e.candidate() else {
                        return;
                    };
                    let line = candidate.candidate();
                    let mut s = shared.borrow_mut();
                    if !line.is_empty() && s.candidates.len() < MAX_CANDIDATES {
                        s.candidates.push_back(line);
                    }
                },
            );
            pc.set_onicecandidate(Some(on_candidate.as_ref().unchecked_ref()));
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
                _on_message: on_message,
                _on_open: on_open,
                _on_close: on_close,
                _on_ice: on_ice,
                _on_candidate: on_candidate,
            });
            Ok(())
        }

        fn room_message(&mut self, text: &str) {
            match FromService::parse(text) {
                Some(FromService::Joined { ice, .. }) if !self.joined => {
                    self.joined = true;
                    self.started = now_ms();
                    if self.start(ice).is_err() {
                        self.fail("unreachable");
                    }
                }
                Some(FromService::Signal(body)) => {
                    let Some(peer) = self.peer.as_ref() else {
                        return;
                    };
                    let (kind, rest) = body.split_once('\n').unwrap_or((body, ""));
                    let pc = peer.pc.clone();
                    let shared = Rc::clone(&self.shared);
                    match kind {
                        signal::ANSWER => {
                            let answer = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
                            answer.set_sdp(rest);
                            wasm_bindgen_futures::spawn_local(async move {
                                if JsFuture::from(pc.set_remote_description(&answer))
                                    .await
                                    .is_err()
                                {
                                    shared.borrow_mut().dead = true;
                                }
                            });
                        }
                        "candidate" if !rest.is_empty() => {
                            let init = RtcIceCandidateInit::new(rest);
                            init.set_sdp_mid(Some("0"));
                            wasm_bindgen_futures::spawn_local(async move {
                                // A candidate that does not fit is skipped; ICE goes on with the rest.
                                let _ = JsFuture::from(
                                    pc.add_ice_candidate_with_opt_rtc_ice_candidate_init(Some(
                                        &init,
                                    )),
                                )
                                .await;
                            });
                        }
                        signal::BUSY => self.fail("full"),
                        signal::CLOSED => self.fail("dropped"),
                        _ => {}
                    }
                }
                Some(FromService::Refused(why)) => self.fail(why),
                Some(FromService::Closed(why)) if !self.shared.borrow().open => self.fail(why),
                _ => {}
            }
        }

        /// Moves the link along: the room's messages, the offer and candidates out, the channel's state.
        fn step(&mut self) {
            if self.ended() {
                return;
            }
            let (room_open, room_closed) = {
                let s = self.shared.borrow();
                (s.room_open, s.room_closed)
            };
            if room_open && !self.asked {
                self.asked = true;
                self.say(&ToService::Join(&self.code));
            }
            loop {
                let next = self.shared.borrow_mut().room.pop_front();
                let Some(text) = next else {
                    break;
                };
                self.room_message(&text);
                if self.ended() {
                    return;
                }
            }
            let (offer, candidates, open, dead) = {
                let mut s = self.shared.borrow_mut();
                let lost = s
                    .disconnected
                    .is_some_and(|t| now_ms() - t > DISCONNECTED_MS);
                (
                    s.offer.take(),
                    std::mem::take(&mut s.candidates),
                    s.open,
                    s.dead || lost,
                )
            };
            if let Some(sdp) = offer {
                self.say(&ToService::Signal(&format!("{}\n{sdp}", signal::OFFER)));
            }
            for line in candidates {
                if self.candidates_sent < MAX_CANDIDATES {
                    self.candidates_sent += 1;
                    self.say(&ToService::Signal(&format!("candidate\n{line}")));
                }
            }
            let was_open = self.watch.borrow().link == Link::Channel;
            if dead {
                self.fail(if was_open { "host-left" } else { "unreachable" });
                return;
            }
            if open && !was_open {
                self.watch.borrow_mut().link = Link::Channel;
                // Signaling is done: the room socket goes.
                self.room = None;
            }
            if !open {
                if room_closed && self.room.is_some() {
                    self.fail(if room_open { "unreachable" } else { "service" });
                } else if self.joined && now_ms() - self.started > OPEN_TIMEOUT_MS {
                    self.fail("unreachable");
                }
            }
        }
    }

    impl Datagrams for RoomDatagrams {
        fn send_to(&mut self, to: SocketAddrV4, bytes: &[u8]) -> bool {
            if to != self.server {
                return false;
            }
            self.step();
            let Some(peer) = self.peer.as_ref() else {
                return false;
            };
            if peer.channel.ready_state() != RtcDataChannelState::Open {
                return false;
            }
            let sent = peer.channel.send_with_u8_array(bytes).is_ok();
            if sent {
                self.watch.borrow_mut().messages_out += 1;
            }
            sent
        }

        fn recv(&mut self) -> Option<(SocketAddrV4, Vec<u8>)> {
            self.step();
            let data = self.shared.borrow_mut().held.pop_front()?;
            Some((self.server, data))
        }

        fn port(&self) -> u16 {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reason_reads_as_a_sentence() {
        for why in [
            "host-left",
            "no-room",
            "full",
            "rate",
            "busy",
            "timeout",
            "unreachable",
            "dropped",
            "service",
            "other",
        ] {
            let text = ended_text(why);
            assert!(text.ends_with('.') && text.chars().next().is_some_and(char::is_uppercase));
        }
        let watch = super::LinkWatch::default();
        end(&watch, "host-left");
        end(&watch, "timeout");
        let w: super::super::web_rtc::LinkState = watch.borrow().clone();
        assert_eq!(w.ended.as_deref(), Some("The host left the game."));
        assert!(w.failed);
    }
}
