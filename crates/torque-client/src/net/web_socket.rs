//! A client's datagrams over a WebSocket (the `web-socket` feature): the game socket a server's web transport takes, one
//! binary message per datagram. A browser opens it with the page's `WebSocket`; natively (tests, tools) it is a plain `ws`
//! connection made here.
//!
//! The socket has one far end, the server: every datagram sent goes to it whatever address it is sent to (the connection
//! names the server by a stand-in address, [`WebSocketDatagrams::server`]), and every datagram received comes from that
//! address. Received datagrams are hostile as on UDP: each is cut to [`RECV_BYTES`] and at most [`MAX_INBOX`] wait.

use super::{Datagrams, MAX_INBOX, RECV_BYTES};

/// The most datagrams held while the socket is still opening (sent once it is open).
pub const MAX_PENDING: usize = 64;

#[cfg(not(target_arch = "wasm32"))]
pub use native::WebSocketDatagrams;
#[cfg(target_arch = "wasm32")]
pub use web::WebSocketDatagrams;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::collections::VecDeque;
    use std::io::{self, ErrorKind, Read, Write};
    use std::net::{SocketAddr, SocketAddrV4, TcpStream};
    use std::time::Duration;

    use tungstenite::protocol::{Role, WebSocket, WebSocketConfig};
    use tungstenite::{Error as WsError, Message};

    use super::*;

    /// How long the connection and the handshake may take.
    const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
    /// The most bytes of the handshake's answer.
    const MAX_ANSWER: usize = 4096;

    /// A `ws` game socket to a server's web transport.
    pub struct WebSocketDatagrams {
        ws: WebSocket<TcpStream>,
        server: SocketAddrV4,
        port: u16,
        closed: bool,
        held: VecDeque<Vec<u8>>,
    }

    impl WebSocketDatagrams {
        /// Opens `ws://<to>/ws` (blocking until the handshake is done, at most a few seconds); the connection then names
        /// the server `server`.
        pub fn connect(to: SocketAddr, server: SocketAddrV4) -> io::Result<WebSocketDatagrams> {
            let mut tcp = TcpStream::connect_timeout(&to, OPEN_TIMEOUT)?;
            tcp.set_read_timeout(Some(OPEN_TIMEOUT))?;
            tcp.set_nodelay(true)?;
            let key = nonce(&tcp);
            write!(
                tcp,
                "GET {} HTTP/1.1\r\nHost: {to}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
                 Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n\r\n",
                torque_server::net::web::WEB_SOCKET_PATH
            )?;
            let mut answer = Vec::new();
            let mut byte = [0u8; 256];
            let end = loop {
                if let Some(end) = answer.windows(4).position(|w| w == b"\r\n\r\n") {
                    break end + 4;
                }
                if answer.len() > MAX_ANSWER {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        "the answer is too long",
                    ));
                }
                let n = tcp.read(&mut byte)?;
                if n == 0 {
                    return Err(io::Error::new(
                        ErrorKind::UnexpectedEof,
                        "the server closed the connection",
                    ));
                }
                answer.extend_from_slice(&byte[..n]);
            };
            let head = String::from_utf8_lossy(&answer[..end]);
            let accept = torque_server::net::web::accept_key(&key);
            let switched = head.starts_with("HTTP/1.1 101 ");
            let accepted = head.lines().any(|l| {
                l.split_once(':').is_some_and(|(k, v)| {
                    k.trim().eq_ignore_ascii_case("sec-websocket-accept") && v.trim() == accept
                })
            });
            if !switched || !accepted {
                let line = head.lines().next().unwrap_or("").to_string();
                return Err(io::Error::new(
                    ErrorKind::ConnectionRefused,
                    format!("the server did not open a game socket: {line}"),
                ));
            }
            tcp.set_read_timeout(None)?;
            tcp.set_nonblocking(true)?;
            let port = tcp.local_addr()?.port();
            let config = WebSocketConfig::default()
                .read_buffer_size(4096)
                .write_buffer_size(0)
                .max_message_size(Some(RECV_BYTES * 32))
                .max_frame_size(Some(RECV_BYTES * 32));
            let rest = answer[end..].to_vec();
            Ok(WebSocketDatagrams {
                ws: WebSocket::from_partially_read(tcp, rest, Role::Client, Some(config)),
                server,
                port,
                closed: false,
                held: VecDeque::new(),
            })
        }

        /// The address the connection names the server by.
        pub fn server(&self) -> SocketAddrV4 {
            self.server
        }

        /// Whether the socket is still open.
        pub fn is_open(&self) -> bool {
            !self.closed
        }

        /// Sends `bytes` as one message without looking at its size (a test of the server's caps).
        pub fn send_raw(&mut self, bytes: &[u8]) -> bool {
            self.send_message(Message::binary(bytes.to_vec()))
        }

        /// Sends a text message (a test: the server takes binary only).
        pub fn send_text(&mut self, text: &str) -> bool {
            self.send_message(Message::text(text))
        }

        fn send_message(&mut self, message: Message) -> bool {
            if self.closed {
                return false;
            }
            match self.ws.send(message) {
                Ok(()) => true,
                Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => true,
                Err(_) => {
                    self.closed = true;
                    false
                }
            }
        }

        /// Reads what has come, keeping the datagrams (at most [`MAX_INBOX`]).
        fn read_all(&mut self) {
            while !self.closed && self.held.len() < MAX_INBOX {
                match self.ws.read() {
                    Ok(Message::Binary(data)) => {
                        let keep = data.len().min(RECV_BYTES);
                        self.held.push_back(data[..keep].to_vec());
                    }
                    Ok(_) => {}
                    Err(WsError::Io(e)) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => self.closed = true,
                }
            }
        }
    }

    /// A handshake key: 16 bytes from the clock and the connection's addresses (the server checks only its form).
    fn nonce(tcp: &TcpStream) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let port = tcp.local_addr().map_or(0, |a| a.port());
        let mut bytes = [0u8; 16];
        bytes[..12].copy_from_slice(&nanos.to_le_bytes()[..12]);
        bytes[12..14].copy_from_slice(&port.to_le_bytes());
        torque_server::net::web::http::base64(&bytes)
    }

    impl Datagrams for WebSocketDatagrams {
        fn send_to(&mut self, to: SocketAddrV4, bytes: &[u8]) -> bool {
            to == self.server && self.send_message(Message::binary(bytes.to_vec()))
        }

        fn recv(&mut self) -> Option<(SocketAddrV4, Vec<u8>)> {
            if self.held.is_empty() {
                self.read_all();
            }
            self.held.pop_front().map(|d| (self.server, d))
        }

        fn port(&self) -> u16 {
            self.port
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::net::SocketAddrV4;
    use std::rc::Rc;

    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::{BinaryType, MessageEvent, WebSocket};

    use super::*;

    /// The page's `WebSocket` to a server's web transport.
    pub struct WebSocketDatagrams {
        socket: WebSocket,
        server: SocketAddrV4,
        incoming: Rc<RefCell<VecDeque<Vec<u8>>>>,
        pending: VecDeque<Vec<u8>>,
        _on_message: Closure<dyn FnMut(MessageEvent)>,
    }

    impl WebSocketDatagrams {
        /// Opens `url` (`ws://host:port/ws` or `wss://...`); the connection then names the server `server`.
        pub fn open(url: &str, server: SocketAddrV4) -> Result<WebSocketDatagrams, String> {
            let socket = WebSocket::new(url).map_err(|e| format!("{url}: {e:?}"))?;
            socket.set_binary_type(BinaryType::Arraybuffer);
            let incoming = Rc::new(RefCell::new(VecDeque::new()));
            let inbox = Rc::clone(&incoming);
            let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                let Ok(buffer) = e.data().dyn_into::<js_sys::ArrayBuffer>() else {
                    return;
                };
                let array = js_sys::Uint8Array::new(&buffer);
                let len = (array.length() as usize).min(RECV_BYTES);
                let mut data = vec![0u8; len];
                array.subarray(0, len as u32).copy_to(&mut data);
                let mut q = inbox.borrow_mut();
                if q.len() < MAX_INBOX {
                    q.push_back(data);
                }
            });
            socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            Ok(WebSocketDatagrams {
                socket,
                server,
                incoming,
                pending: VecDeque::new(),
                _on_message: on_message,
            })
        }

        /// The address the connection names the server by.
        pub fn server(&self) -> SocketAddrV4 {
            self.server
        }

        /// Whether the socket is open or still opening.
        pub fn is_open(&self) -> bool {
            matches!(
                self.socket.ready_state(),
                WebSocket::CONNECTING | WebSocket::OPEN
            )
        }

        fn flush_pending(&mut self) {
            if self.socket.ready_state() != WebSocket::OPEN {
                return;
            }
            while let Some(data) = self.pending.pop_front() {
                if self.socket.send_with_u8_array(&data).is_err() {
                    break;
                }
            }
        }
    }

    impl Drop for WebSocketDatagrams {
        fn drop(&mut self) {
            self.socket.set_onmessage(None);
            let _ = self.socket.close();
        }
    }

    impl Datagrams for WebSocketDatagrams {
        fn send_to(&mut self, to: SocketAddrV4, bytes: &[u8]) -> bool {
            if to != self.server {
                return false;
            }
            match self.socket.ready_state() {
                WebSocket::CONNECTING if self.pending.len() < MAX_PENDING => {
                    self.pending.push_back(bytes.to_vec());
                    true
                }
                WebSocket::OPEN => {
                    self.flush_pending();
                    self.socket.send_with_u8_array(bytes).is_ok()
                }
                _ => false,
            }
        }

        fn recv(&mut self) -> Option<(SocketAddrV4, Vec<u8>)> {
            self.flush_pending();
            let data = self.incoming.borrow_mut().pop_front()?;
            Some((self.server, data))
        }

        fn port(&self) -> u16 {
            // A browser's socket has no port of its own to show; the stand-in's.
            self.server.port()
        }
    }
}
