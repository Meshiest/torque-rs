//! A dedicated server's command line for browsers (the `p2p` feature, natively): the switches that put it on the web beside
//! UDP or instead of it, give browsers WebRTC data channels, or host it through a room service; where it then listens
//! ([`WebFlags::listen`]); and the lines it prints when it has started ([`Served::start_lines`]). A game's dedicated
//! server reads its own switches (`--bind`, `--port`, `--name`, its map) and hands the rest to [`WebFlags::take`].
//!
//! **The web** (`--transport udp|web|both`, default `udp`): `both` adds HTTP and WebSocket on TCP at the same address and
//! port as the UDP socket (desktop and browser players in one game); `web` opens no UDP socket. The page served is the
//! game's browser build (`node web/build.mjs <example>` writes `<target>/web/<example>`, found next to the program's
//! folder), or `--web-root <dir>`; `--no-files` serves none (the page comes from another web server). `--tls-cert <pem>
//! --tls-key <pem>` serves `https` and `wss`; without them the web listens on a private address only. A request must name
//! an IP address, `localhost` or a `--web-host <name>` (repeatable) as its host; a browser's game socket must come from the
//! page this server serves, or from an `--allow-origin <origin>` (repeatable; `*` takes any page). TLS handshakes run on
//! the server's thread: an ECDSA (P-256) key makes each far cheaper than an RSA one. The address rule looks at the bind
//! address only: a private address that a router maps 1:1 to a public one serves plain `http` and `ws` to the internet,
//! so use TLS there.
//!
//! **WebTransport** (`--webtransport`, with `--transport web` or `both`, the `quic` feature): the QUIC link listens on the
//! UDP port of the same number as the web, with WebTransport sessions on, instead of plain UDP (desktops join over QUIC,
//! browsers over WebTransport; a browser without it, or on a page that is not a secure context, stays on its game socket
//! and a data channel). The page is told what it needs in `webtransport.json` (the QUIC port number, the path, the rotating certificate
//! hashes); the server's identity key is `identity.key` in its data folder, or `--identity <file>`. A page served by
//! another web server joins only from an `--allow-origin`; `--web-host` names count for the CONNECT's authority too.
//!
//! **Data channels** (`--webrtc`, with `--transport web` or `both`): browsers get a WebRTC data channel beside their game
//! socket, on a UDP port of its own (`--rtc-port <port>`, default the game's port number plus one; 0 any free one). Its
//! candidates are the bind address and each `--rtc-public-ip <address>`. A public server (`--bind 0.0.0.0`) needs
//! `--rtc-public-ip` or `--rtc-lan` ([`rtc_candidates`]: the computer's private LAN addresses are told to every browser
//! only when asked). For checks only, each with a warning at startup ([`WebFlags::warnings`]): `--rtc-loss <percent>`
//! drops that share of the channels' datagrams both ways, and `--rtc-stats` prints the channels' traffic every 2 s.
//!
//! **A room** (`--room <ws://address:port>`, a `torque-rooms` service): the server opens a room and players join by its
//! code over data channels straight to this computer, with no port forwarded and no UDP game socket. The channels' socket
//! binds the server's address and port. Not with `--transport web` or `both`. `--rtc-lan` takes joiners' private (LAN)
//! addresses too (off by default: a joiner could aim the host's checks at machines on the network).

use std::cell::RefCell;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use torque_server::net::transport::{BindPolicy, LanRules, SharedTransport, Transport};
use torque_server::net::web::p2p::{P2pHost, P2pSetup};
use torque_server::net::web::rtc::RtcSetup;
use torque_server::net::web::{Access, Origins as Origins_, Tls, TlsFiles, WebFiles, WebSetup};

use crate::cli::Args;
#[cfg(feature = "quic")]
use crate::server::QuicListen;
use crate::server::{GameServer, Listen, WebListen, lan_addresses};
use crate::transports::BoundTransport;

/// Which transports a dedicated server listens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transports {
    /// UDP only.
    #[default]
    Udp,
    /// HTTP and WebSocket only: no UDP socket is opened.
    Web,
    /// UDP, and HTTP and WebSocket on the same port number.
    Both,
}

impl Transports {
    /// `udp`, `web` or `both`.
    pub fn parse(text: &str) -> Result<Transports, String> {
        match text {
            "udp" => Ok(Transports::Udp),
            "web" => Ok(Transports::Web),
            "both" => Ok(Transports::Both),
            _ => Err(format!("--transport takes udp, web or both, not {text:?}")),
        }
    }
}

/// Where a dedicated server binds: its address, its port number (0 any free one) and whether `0.0.0.0` (every interface:
/// players outside the network can join) is allowed as the address; no other public address is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bind {
    pub address: Ipv4Addr,
    pub port: u16,
    pub public: bool,
}

/// The extra addresses a dedicated server's data channels advertise to browsers beside the bind address: `named` (each
/// `--rtc-public-ip`), and on `0.0.0.0` the computer's `lan` addresses, which are private. A public server (`0.0.0.0`) is
/// seen by the internet, so it leaves the private ones out unless `with_lan` asks for them (a LAN address told to every
/// browser discloses the network behind the server). With neither a public address named nor `with_lan`, it refuses to run
/// data channels; with `with_lan` alone the candidates are the LAN's, which only players on that network can use.
pub fn rtc_candidates(
    public: bool,
    named: &[Ipv4Addr],
    with_lan: bool,
    lan: &[Ipv4Addr],
) -> Result<Vec<Ipv4Addr>, String> {
    if !public {
        return Ok(named.to_vec());
    }
    if named.is_empty() && !with_lan {
        return Err("--webrtc on a public server (--bind 0.0.0.0) needs --rtc-public-ip <address>, the address browsers reach this computer by, or --rtc-lan to advertise this computer's private LAN addresses (only players on that network can use them)".to_string());
    }
    let mut found = named.to_vec();
    if with_lan {
        for &ip in lan {
            if !found.contains(&ip) {
                found.push(ip);
            }
        }
    }
    Ok(found)
}

/// A server on `transports` at `bind`: UDP alone, or the web as `setup` says (its files, its TLS, its limits) with UDP
/// beside it for [`Transports::Both`].
pub fn web_listen(transports: Transports, bind: Bind, setup: WebSetup) -> Listen {
    match transports {
        Transports::Udp => Listen::Udp {
            address: bind.address,
            port: bind.port,
            public: bind.public,
        },
        Transports::Web | Transports::Both => Listen::Web {
            address: bind.address,
            port: bind.port,
            public: bind.public,
            web: Rc::new(WebListen {
                udp: transports == Transports::Both,
                #[cfg(feature = "quic")]
                quic: None,
                setup,
            }),
        },
    }
}

/// A server on the web at `bind` with the QUIC link beside it instead of plain UDP (`quic`: its identity key, its caps and
/// its WebTransport sessions), whatever `transports` says of UDP.
#[cfg(feature = "quic")]
pub fn web_quic_listen(bind: Bind, setup: WebSetup, quic: Rc<QuicListen>) -> Listen {
    Listen::Web {
        address: bind.address,
        port: bind.port,
        public: bind.public,
        web: Rc::new(WebListen {
            udp: false,
            quic: Some(quic),
            setup,
        }),
    }
}

/// A server hosted through the room service at `room` (its `ws://` URL): a player's host transport whose data channels'
/// socket binds `bind`, its room opened. `lan` takes joiners' private (LAN) addresses as candidates. The transport (for
/// [`Listen::Bound`]) and the room's code.
pub fn room_host(bind: Bind, room: &str, lan: bool) -> Result<(SharedTransport, String), String> {
    let ip = IpAddr::V4(bind.address);
    let policy = BindPolicy::new(ip, bind.public, &LanRules::PLAIN)
        .map_err(|e| format!("the address {}: {e}", bind.address))?;
    let mut host = P2pHost::new(
        policy,
        P2pSetup {
            service: room.to_string(),
            queue: 4096,
            rtc: RtcSetup {
                lan_candidates: lan,
                ..Default::default()
            },
        },
    );
    if let Err(e) = host.bind(ip, bind.port) {
        let why = host.status().join("; ");
        return Err(format!("hosting through {room}: {e} {why}"));
    }
    let code = host.code().ok_or("the room service gave no code")?;
    Ok((Rc::new(RefCell::new(host)), code))
}

/// The browser build's folder next to the running program's: `<target>/web/<example>` for `<target>/release/<program>`.
pub fn default_web_root(example: &str) -> Option<PathBuf> {
    let program = std::env::current_exe().ok()?;
    let dir = program.parent()?.parent()?.join("web").join(example);
    dir.is_dir().then_some(dir)
}

/// The web switches of a dedicated server's command line, as read (module docs).
#[derive(Debug, Clone, Default)]
pub struct WebFlags {
    pub transports: Transports,
    web_root: Option<PathBuf>,
    no_files: bool,
    cert: Option<PathBuf>,
    key: Option<PathBuf>,
    access: Access,
    origins: Vec<String>,
    webrtc: bool,
    rtc_port: Option<u16>,
    rtc_public: Vec<Ipv4Addr>,
    rtc_lan: bool,
    rtc_loss: u8,
    rtc_stats: bool,
    room: Option<String>,
    webtransport: bool,
    identity: Option<PathBuf>,
}

impl WebFlags {
    /// Reads `arg` (and its value from `args`) when it is one of these switches; false when it is not one.
    pub fn take(&mut self, arg: &str, args: &mut Args) -> Result<bool, String> {
        match arg {
            "--transport" => {
                self.transports = Transports::parse(&args.value(arg, "udp, web or both")?)?
            }
            "--web-root" => self.web_root = Some(args.dir(arg)?),
            "--no-files" => self.no_files = true,
            "--tls-cert" => self.cert = Some(args.dir(arg)?),
            "--tls-key" => self.key = Some(args.dir(arg)?),
            "--web-host" => self.access.hosts.push(args.value(arg, "a host name")?),
            "--allow-origin" => self.origins.push(args.value(arg, "an origin")?),
            "--webrtc" => self.webrtc = true,
            "--rtc-port" => self.rtc_port = Some(args.number(arg)?),
            "--rtc-public-ip" => self.rtc_public.push(args.ipv4(arg)?),
            "--rtc-lan" => self.rtc_lan = true,
            "--rtc-loss" => self.rtc_loss = args.number::<u8>(arg)?.min(100),
            "--rtc-stats" => self.rtc_stats = true,
            "--room" => self.room = Some(args.value(arg, "a room service's ws:// URL")?),
            "--webtransport" => self.webtransport = true,
            "--identity" => self.identity = Some(args.dir(arg)?),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The switches that go together, checked: the web's need the web, the data channels' need `--webrtc`, and a room
    /// takes neither.
    pub fn check(&self) -> Result<(), String> {
        let web = self.web_root.is_some()
            || self.no_files
            || self.cert.is_some()
            || self.key.is_some()
            || !self.access.hosts.is_empty()
            || !self.origins.is_empty();
        if self.transports == Transports::Udp && (web || self.webrtc) {
            return Err("--web-root, --no-files, --tls-cert, --tls-key, --web-host, --allow-origin and --webrtc are for the web: add --transport web or --transport both".into());
        }
        if !self.webrtc
            && (self.rtc_port.is_some()
                || !self.rtc_public.is_empty()
                || (self.rtc_lan && self.room.is_none())
                || self.rtc_loss > 0
                || self.rtc_stats)
        {
            return Err("--rtc-port, --rtc-public-ip, --rtc-lan, --rtc-loss and --rtc-stats are for the data channels: add --webrtc".into());
        }
        if self.room.is_some() && (self.transports != Transports::Udp || self.webrtc) {
            return Err(
                "--room hosts over data channels alone: leave out --transport and --webrtc".into(),
            );
        }
        if self.webtransport && self.transports == Transports::Udp {
            return Err("--webtransport puts the QUIC link beside the web: add --transport web or --transport both".into());
        }
        if self.identity.is_some() && !self.webtransport {
            return Err("--identity names the QUIC link's key: add --webtransport".into());
        }
        if cfg!(not(feature = "quic")) && self.webtransport {
            return Err("--webtransport needs the `quic` feature of this build".into());
        }
        if self.cert.is_some() != self.key.is_some() {
            return Err("--tls-cert and --tls-key go together".into());
        }
        Ok(())
    }

    /// The warnings to print at startup: the switches that are for checks only.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        let loss = self.rtc_loss;
        if loss > 0 {
            out.push(format!("--rtc-loss {loss} drops {loss}% of the data channels' datagrams both ways; it is for checks, not for a server players use"));
        }
        if self.rtc_stats {
            out.push("--rtc-stats prints each data channel peer's traffic every 2 s; it is for checks, not for a server players use".to_string());
        }
        out
    }

    /// Where the server listens, as the switches say ([`WebFlags::check`] first): UDP at `bind`, the web beside it or
    /// instead, or a room. `example` names the browser build served when no `--web-root` is given (`<target>/web/<example>`).
    pub fn listen(&self, bind: Bind, example: &str) -> Result<Served, String> {
        self.check()?;
        let mut served = Served {
            listen: Listen::Udp {
                address: bind.address,
                port: bind.port,
                public: bind.public,
            },
            code: None,
            room: self.room.clone(),
            transports: self.transports,
            scheme: "http",
            webrtc: self.webrtc,
            webtransport: self.webtransport,
            rtc_lan_only: false,
            rtc_stats: self.rtc_stats,
            public: bind.public,
        };
        if let Some(room) = &self.room {
            let (transport, code) = room_host(bind, room, self.rtc_lan)?;
            served.listen = Listen::Bound(BoundTransport::new(BoundTransport::ROOMS, transport));
            served.code = Some(code);
            return Ok(served);
        }
        if self.transports == Transports::Udp {
            return Ok(served);
        }
        let tls = match (&self.cert, &self.key) {
            (Some(cert), Some(key)) => {
                served.scheme = "https";
                Some(Tls::load(&TlsFiles {
                    cert: cert.clone(),
                    key: key.clone(),
                })?)
            }
            _ => None,
        };
        let files = if self.no_files {
            None
        } else {
            let root = match &self.web_root {
                Some(dir) => dir.clone(),
                None => default_web_root(example)
                    .ok_or("no folder for the page: pass --web-root <dir> or --no-files")?,
            };
            Some(WebFiles::from_dir(&root).map_err(|e| {
                format!(
                    "the page's files in {}: {e} (build them with `node web/build.mjs {example}`, or pass --web-root or --no-files)",
                    root.display()
                )
            })?)
        };
        let mut access = self.access.clone();
        access.origins = if self.origins.iter().any(|o| o == "*") {
            Origins_::Any
        } else if self.origins.is_empty() {
            Origins_::SameHost
        } else {
            Origins_::List(self.origins.clone())
        };
        let rtc = if self.webrtc {
            let port = match self.rtc_port {
                Some(port) => port,
                None if bind.port == 0 => 0,
                None => bind
                    .port
                    .checked_add(1)
                    .ok_or("--rtc-port is needed when --port is 65535")?,
            };
            served.rtc_lan_only = bind.public && self.rtc_public.is_empty();
            let public = rtc_candidates(
                bind.public,
                &self.rtc_public,
                self.rtc_lan,
                &lan_addresses(),
            )?;
            Some(RtcSetup {
                port,
                public,
                loss_percent: self.rtc_loss,
                ..RtcSetup::default()
            })
        } else {
            None
        };
        #[cfg(feature = "quic")]
        let quic = self.webtransport.then(|| {
            use torque_net_quic::{Origins, WebTransportSession, WebTransportSetup};
            let origins = match &access.origins {
                Origins_::Any => Origins::Any,
                Origins_::List(list) => Origins::List(list.clone()),
                Origins_::SameHost => Origins::SameHost,
            };
            Rc::new(QuicListen {
                identity: match &self.identity {
                    Some(path) => crate::server::IdentitySource::File(path.clone()),
                    None => crate::server::IdentitySource::Default,
                },
                limits: torque_net_quic::QuicLimits::default(),
                web_transport: Some(WebTransportSetup {
                    session: WebTransportSession {
                        origins,
                        path: "/".to_string(),
                        hosts: access.hosts.clone(),
                    },
                    certificates: torque_net_quic::CertificateChoice::Rotating,
                }),
            })
        });
        let setup = WebSetup {
            files,
            tls,
            access,
            rtc,
            ..WebSetup::default()
        };
        #[cfg(feature = "quic")]
        if let Some(quic) = quic {
            served.listen = web_quic_listen(bind, setup, quic);
            return Ok(served);
        }
        served.listen = web_listen(self.transports, bind, setup);
        Ok(served)
    }
}

/// Where a dedicated server listens ([`WebFlags::listen`]): what it starts on, and what its start-up lines say.
#[derive(Debug)]
pub struct Served {
    /// What the server starts on.
    pub listen: Listen,
    /// The room's code, when it is hosted through a room service.
    pub code: Option<String>,
    room: Option<String>,
    transports: Transports,
    scheme: &'static str,
    webrtc: bool,
    webtransport: bool,
    rtc_lan_only: bool,
    rtc_stats: bool,
    public: bool,
}

impl Served {
    /// The lines to print once `server` (named `name`, playing `map`) has started on [`Served::listen`], and its status
    /// lines' pace set: where it listens, where browsers open the page (at `page`: `/`, or `/?join` for a page that plays
    /// alone unless asked to join), the room's code, the data channels' state, and how players on the network reach it.
    pub fn start_lines(
        &self,
        server: &mut GameServer,
        name: &str,
        map: &str,
        page: &str,
    ) -> Vec<String> {
        let mut out = Vec::new();
        if let (Some(room), Some(code)) = (&self.room, &self.code) {
            out.push(format!("{name} hosted through {room}, map {map}"));
            out.push(format!("room code {code}"));
            if let Some(addr) = server.address() {
                out.push(format!("data channels on udp {addr}"));
                if addr.ip().is_loopback() {
                    out.push("only this computer can reach it: pass --bind <this computer's LAN address> for other players".to_string());
                }
            }
            server.set_status_every(Some(Duration::from_secs(1)));
            return out;
        }
        let Some(addr) = server.address() else {
            return out;
        };
        out.push(format!("{name} on {addr}, map {map}"));
        if self.transports != Transports::Udp {
            out.push(format!("browsers join {}://{addr}{page}", self.scheme));
        }
        if self.webtransport {
            out.push(format!(
                "WebTransport and QUIC on udp {addr}: browsers on a secure page join over WebTransport, desktops over the QUIC link"
            ));
            out.extend(server.transport_status());
        }
        if self.webrtc {
            out.extend(server.transport_status());
            if self.rtc_lan_only {
                out.push("data channels advertise only this computer's LAN addresses: players outside the network cannot use WebRTC and stay on the WebSocket (add --rtc-public-ip for them)".to_string());
            }
        }
        if self.rtc_stats {
            server.set_status_every(Some(Duration::from_secs(2)));
        }
        if self.public {
            let port = addr.port();
            let lan = lan_addresses();
            if lan.is_empty() {
                out.push("no LAN address found on this computer".to_string());
            }
            for ip in lan {
                out.push(format!("players on this network join {ip}:{port}"));
            }
            out.push(format!("players outside the network need UDP {port} forwarded to this computer, and the firewall to allow it"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC: Ipv4Addr = Ipv4Addr::new(203, 0, 113, 7);
    const LAN: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 20);

    #[test]
    fn a_public_server_advertises_no_private_address_unless_asked() {
        // A private bind keeps what it is given (the library adds the bind address).
        assert_eq!(rtc_candidates(false, &[], false, &[LAN]), Ok(vec![]));
        assert_eq!(
            rtc_candidates(false, &[PUBLIC], false, &[LAN]),
            Ok(vec![PUBLIC])
        );
        // A public bind needs a public address, and does not add the LAN's on its own.
        assert!(rtc_candidates(true, &[], false, &[LAN]).is_err());
        assert_eq!(rtc_candidates(true, &[], true, &[LAN]), Ok(vec![LAN]));
        assert_eq!(
            rtc_candidates(true, &[PUBLIC], false, &[LAN]),
            Ok(vec![PUBLIC])
        );
        // The flag adds them, once each.
        assert_eq!(
            rtc_candidates(true, &[PUBLIC], true, &[LAN, LAN, PUBLIC]),
            Ok(vec![PUBLIC, LAN])
        );
    }

    fn flags(words: &[&str]) -> Result<WebFlags, String> {
        let mut args = Args::new(words.iter().map(|w| w.to_string()));
        let mut flags = WebFlags::default();
        while let Some(arg) = args.next() {
            if !flags.take(&arg, &mut args)? {
                return Err(crate::cli::unknown(&arg));
            }
        }
        flags.check().map(|()| flags)
    }

    #[test]
    fn the_switches_that_go_together_are_checked() {
        assert!(flags(&[]).is_ok());
        assert!(flags(&["--transport", "both", "--webrtc", "--rtc-stats"]).is_ok());
        assert!(flags(&["--room", "ws://127.0.0.1:1", "--rtc-lan"]).is_ok());
        assert!(flags(&["--transport", "lan"]).is_err());
        // The web's switches need the web, the data channels' need --webrtc.
        assert!(flags(&["--no-files"]).is_err());
        assert!(flags(&["--webrtc"]).is_err());
        assert!(flags(&["--transport", "web", "--rtc-lan"]).is_err());
        assert!(flags(&["--transport", "both", "--rtc-loss", "10"]).is_err());
        // A room takes neither.
        assert!(flags(&["--room", "ws://127.0.0.1:1", "--transport", "both"]).is_err());
        // A certificate needs its key.
        assert!(flags(&["--transport", "web", "--tls-cert", "a.pem"]).is_err());
        // Not a switch of these.
        assert_eq!(
            flags(&["--name", "x"]).err(),
            Some("unknown argument --name".to_string())
        );
        assert_eq!(
            flags(&["--transport", "both", "--webrtc", "--rtc-loss", "250"])
                .map(|f| f.warnings().len()),
            Ok(1)
        );
    }
}
