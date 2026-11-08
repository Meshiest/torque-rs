//! The test client: the other half of every codec in [`crate::protocol`] and [`crate::handshake`]. It is the fake client of the
//! scenarios (a handshake driver and a packet layer a test or the harness feeds datagrams), the datagram decoder
//! ([`decode_datagram`]: header, rate prefix and handshake fields with fewer than 8 bits left over, no connection state) and the
//! writer of the client-to-server packet head ([`C2sHead`]).
//!
//! It checks nothing a conforming client would not, and it can be told to send what a conforming one never would (a bad class
//! name, group, game string, protocol, argument count) so the hostile and reject cases are driven from here. Nothing here touches a socket or a clock:
//! the caller passes datagrams and `now`.

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::cap::{self, NET_MAX_CONNECT_ARGS};
use crate::connect::ConnectBlock;
use crate::handshake::c_string;
use crate::handshake::{self, Accept, ConnectFields, GAME_CONNECTION_CLASS};
use crate::huffman::Table;
use crate::profile::HandshakeProfile;
use crate::protocol::{
    Header, PacketType, Protocol, RateConfig, RatePrefix, Received, Role, SendPacing,
};

/// The ghost-id widths a client of `profile` takes from this accept: the form's
/// [`crate::profile::HandshakeForm::ghost_bits_range`], the top raised to the extension's width when the accept granted the
/// extension. Any other width is refused with the profile's ghost-bits reject string.
pub fn accept_bits_range(
    profile: &HandshakeProfile,
    accept: &Accept,
) -> std::ops::RangeInclusive<u32> {
    let (min, mut max) = profile.ghost_bits_bounds();
    if accept.extension
        && let Some(bits) = profile.extension.as_ref().and_then(|e| e.ghost_bits)
    {
        max = max.max(bits);
    }
    let max = max.min(cap::NET_MAX_GHOST_BITS);
    u32::from(min)..=u32::from(max)
}

/// Where the client is in the handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientState {
    Idle,
    /// A challenge request is out.
    Challenging,
    /// A connect request is out.
    Connecting,
    Connected,
    Rejected(Vec<u8>),
    Disconnected(Vec<u8>),
}

/// What a datagram did to the client.
#[derive(Debug)]
pub enum ClientEvent {
    /// Nothing (a datagram it ignores).
    Nothing,
    /// Send this to the server.
    Send(Vec<u8>),
    Accepted(Accept),
    Rejected(Vec<u8>),
    Disconnected(Vec<u8>),
    /// A game packet: the packet layer's result and where the payload starts.
    Game {
        received: Received,
        payload_bit: usize,
    },
    /// A valid header followed by a bad rate prefix.
    GameError(NetError),
}

/// The client half of a connection.
#[derive(Debug)]
pub struct Client {
    seq: u32,
    class: Vec<u8>,
    fields: ConnectFields,
    profile: HandshakeProfile,
    rates: RateConfig,
    pacing: Option<Box<dyn SendPacing>>,
    state: ClientState,
    digest: Option<[u32; 4]>,
    protocol: Option<Protocol>,
    accept: Option<Accept>,
}

impl Client {
    /// A client of Torque's profile and the standard rates that will connect with the given sequence and request fields.
    pub fn new(seq: u32, fields: ConnectFields) -> Client {
        Client {
            seq,
            class: GAME_CONNECTION_CLASS.to_vec(),
            fields,
            profile: HandshakeProfile::default(),
            rates: RateConfig::standard(),
            pacing: None,
            state: ClientState::Idle,
            digest: None,
            protocol: None,
            accept: None,
        }
    }

    /// The client half of a local connection (`connectLocal`): connected at once with sequence 0 and the accept the
    /// server half wrote (`protocol`, `ghost_bits`), no handshake datagram.
    pub fn local(
        fields: ConnectFields,
        rates: RateConfig,
        protocol: u32,
        ghost_bits: u32,
        now: u32,
    ) -> Client {
        Client {
            seq: 0,
            class: GAME_CONNECTION_CLASS.to_vec(),
            fields,
            profile: HandshakeProfile::default(),
            rates,
            pacing: None,
            state: ClientState::Connected,
            digest: None,
            protocol: Some(Protocol::new(Role::Client, 0, rates, now)),
            accept: Some(Accept {
                seq: 0,
                protocol,
                ghost_bits: Some(ghost_bits),
                extension: false,
                channels: false,
            }),
        }
    }

    /// Reads accepts (and writes nothing else differently) as a client of `profile`: its protocol versions, the ghost-id width its
    /// accept carries, its extension and its reject strings.
    pub fn with_profile(mut self, profile: HandshakeProfile) -> Client {
        self.profile = profile;
        self
    }

    pub fn profile(&self) -> &HandshakeProfile {
        &self.profile
    }

    /// The rates the connection's packet layer starts with once accepted (a local client is given its rates by
    /// [`Client::local`]).
    pub fn with_rates(mut self, rates: RateConfig) -> Client {
        self.rates = rates;
        self
    }

    /// What a game adds to the connection's send gate ([`SendPacing`]).
    pub fn with_pacing(mut self, pacing: Option<Box<dyn SendPacing>>) -> Client {
        if let Some(p) = self.protocol.as_mut() {
            p.set_pacing(pacing.clone());
        }
        self.pacing = pacing;
        self
    }

    /// Sends this class name instead of `GameConnection` (a hostile case).
    pub fn with_class(mut self, class: &[u8]) -> Client {
        self.class = class.to_vec();
        self
    }

    pub fn state(&self) -> &ClientState {
        &self.state
    }

    pub fn accept(&self) -> Option<Accept> {
        self.accept
    }

    pub fn digest(&self) -> Option<[u32; 4]> {
        self.digest
    }

    pub fn sequence(&self) -> u32 {
        self.seq
    }

    pub fn fields_mut(&mut self) -> &mut ConnectFields {
        &mut self.fields
    }

    pub fn protocol(&self) -> Option<&Protocol> {
        self.protocol.as_ref()
    }

    pub fn protocol_mut(&mut self) -> Option<&mut Protocol> {
        self.protocol.as_mut()
    }

    /// The challenge request.
    pub fn challenge_request(&mut self) -> Vec<u8> {
        self.state = ClientState::Challenging;
        self.profile.types.challenge_request(self.seq)
    }

    /// The connect request over the digest the server returned (or `digest`, to send a wrong one).
    pub fn connect_request(&mut self, table: &Table, digest: Option<[u32; 4]>) -> Vec<u8> {
        self.state = ClientState::Connecting;
        handshake::connect_request(
            &self.profile,
            table,
            self.seq,
            digest.or(self.digest).unwrap_or_default(),
            &self.class,
            &self.fields,
        )
    }

    /// A disconnect from this client.
    pub fn disconnect_datagram(&mut self, table: &Table, reason: &[u8]) -> Vec<u8> {
        self.state = ClientState::Disconnected(reason.to_vec());
        self.profile.types.disconnect(table, self.seq, reason)
    }

    /// Takes a datagram from the server. A challenge response is answered with the connect request at once.
    pub fn on_datagram(&mut self, table: &Table, data: &[u8], now: u32) -> ClientEvent {
        if data.first().is_some_and(|b| b & 1 != 0) {
            return self.game_packet(data, now);
        }
        let types = self.profile.types;
        let Some(ty) = data.first().copied() else {
            return ClientEvent::Nothing;
        };
        match ty {
            _ if ty == types.challenge_response && self.state == ClientState::Challenging => {
                match types.parse_challenge_response(data) {
                    Ok((seq, digest)) if seq == self.seq => {
                        self.digest = Some(digest);
                        ClientEvent::Send(self.connect_request(table, None))
                    }
                    _ => ClientEvent::Nothing,
                }
            }
            _ if ty == types.connect_accept && self.state == ClientState::Connecting => {
                self.on_accept(data, now)
            }
            _ if ty == types.connect_reject && self.state == ClientState::Connecting => {
                match types.parse_reject(table, data) {
                    Ok((seq, reason)) if seq == self.seq => {
                        self.state = ClientState::Rejected(reason.clone());
                        ClientEvent::Rejected(reason)
                    }
                    _ => ClientEvent::Nothing,
                }
            }
            _ if ty == types.disconnect && self.state == ClientState::Connected => {
                match types.parse_disconnect(table, data) {
                    Ok((seq, reason)) if seq == self.seq => {
                        self.state = ClientState::Disconnected(reason.clone());
                        ClientEvent::Disconnected(reason)
                    }
                    _ => ClientEvent::Nothing,
                }
            }
            _ => ClientEvent::Nothing,
        }
    }

    /// The client reads the accept as a client of its profile does (`readConnectAccept`): a protocol within the profile's versions,
    /// and a ghost-id width within [`accept_bits_range`] when the accept carries one.
    fn on_accept(&mut self, data: &[u8], now: u32) -> ClientEvent {
        let profile = &self.profile;
        let Ok(accept) = handshake::parse_accept(profile, data) else {
            return ClientEvent::Nothing;
        };
        if accept.seq != self.seq {
            return ClientEvent::Nothing;
        }
        let reason =
            if !(profile.protocol_min..=profile.protocol_current).contains(&accept.protocol) {
                Some(profile.rejects.protocol.clone())
            } else if accept
                .ghost_bits
                .is_some_and(|bits| !accept_bits_range(profile, &accept).contains(&bits))
            {
                Some(profile.rejects.ghost_bits.clone())
            } else {
                None
            };
        if let Some(reason) = reason {
            self.state = ClientState::Rejected(reason.clone());
            return ClientEvent::Rejected(reason);
        }
        self.state = ClientState::Connected;
        self.accept = Some(accept);
        let mut protocol = Protocol::new(Role::Client, self.seq, self.rates, now);
        protocol.set_pacing(self.pacing.clone());
        self.protocol = Some(protocol);
        ClientEvent::Accepted(accept)
    }

    fn game_packet(&mut self, data: &[u8], now: u32) -> ClientEvent {
        let Some(protocol) = self.protocol.as_mut() else {
            return ClientEvent::Nothing;
        };
        let mut r = BitReader::new(data);
        match protocol.receive(&mut r, now) {
            Ok(received) => ClientEvent::Game {
                received,
                payload_bit: r.position(),
            },
            Err(e) => ClientEvent::GameError(e),
        }
    }

    /// Starts a data packet if the send gate allows one (`force` skips the delay, not the window).
    pub fn begin_data_packet(&mut self, now: u32, force: bool) -> Option<BitWriter> {
        let protocol = self.protocol.as_mut()?;
        protocol
            .may_send(now, force)
            .then(|| protocol.begin_data_packet(now))
    }
}

/// The head of a client-to-server payload, in the order the server reads it: a camera-position flag, the control object's checksum,
/// the 32-bit index of the first move and the 5-bit count. The moves, the first-person pair ([`write_first_person`]) and the fov
/// pair ([`write_fov`]) follow (the whole block is [`crate::control::ClientBlock`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct C2sHead {
    pub camera_flag: bool,
    pub control_checksum: u32,
    pub move_start: u32,
    pub move_count: u8,
}

impl C2sHead {
    pub fn write(&self, w: &mut BitWriter) {
        w.write_flag(self.camera_flag);
        w.write_u32(self.control_checksum);
        w.write_u32(self.move_start);
        w.write_bits(u32::from(self.move_count), 5);
    }

    pub fn read(r: &mut BitReader) -> Result<C2sHead, NetError> {
        Ok(C2sHead {
            camera_flag: r.read_flag()?,
            control_checksum: r.read_u32()?,
            move_start: r.read_u32()?,
            move_count: r.read_bits(5)? as u8,
        })
    }
}

/// The first-person pair after the moves: a flag and, when set, the setting.
pub fn write_first_person(w: &mut BitWriter, first_person: Option<bool>) {
    w.write_flag(first_person.is_some());
    if let Some(on) = first_person {
        w.write_flag(on);
    }
}

/// The field-of-view pair that ends the client's payload before the events: a flag and, when set, 8 bits.
pub fn write_fov(w: &mut BitWriter, fov: Option<u8>) {
    w.write_flag(fov.is_some());
    if let Some(f) = fov {
        w.write_u8(f);
    }
}

pub fn read_fov(r: &mut BitReader) -> Result<Option<u8>, NetError> {
    Ok(if r.read_flag()? {
        Some(r.read_u8()?)
    } else {
        None
    })
}

/// A connect request as it is on the wire, with nothing checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRequest {
    pub seq: u32,
    pub digest: [u32; 4],
    pub class: Vec<u8>,
    pub fields: ConnectFields,
}

/// Reads a connect request without judging it (the datagram decoder), as a server of `profile` reads it. More than
/// [`NET_MAX_CONNECT_ARGS`] arguments is a [`NetError::Cap`], as is a count the bits left could not hold. It also reads the
/// optional [connect block](crate::connect::ConnectBlock) after the arguments: bytes that do not start with its magic are no
/// block (and ignored, as a Torque server ignores them), but a tail that starts with the magic and is then garbled, oversize or
/// unknown is an `Err` ([`NetError::Invalid`]).
pub fn parse_connect_request(
    table: &Table,
    profile: &HandshakeProfile,
    data: &[u8],
) -> Result<ParsedRequest, NetError> {
    let mut r = BitReader::new(data);
    if r.read_u8()? != profile.types.connect_request {
        return Err(NetError::Invalid("datagram type"));
    }
    let seq = r.read_u32()?;
    let mut digest = [0u32; 4];
    for word in &mut digest {
        *word = r.read_u32()?;
    }
    let class = c_string(table.read(&mut r)?);
    let group = r.read_u32()?;
    let crc = r.read_u32()?;
    let game = c_string(table.read(&mut r)?);
    let current_protocol = r.read_u32()?;
    let min_protocol = r.read_u32()?;
    let password = c_string(table.read(&mut r)?);
    let extra = profile.form.read_request(&mut r)?;
    let argc = cap::count(
        "connect arguments",
        u64::from(r.read_u32()?),
        NET_MAX_CONNECT_ARGS,
        r.bits_left(),
        9,
    )?;
    let mut args = Vec::with_capacity(argc);
    for _ in 0..argc {
        args.push(c_string(table.read(&mut r)?));
    }
    let block = ConnectBlock::read(&mut r)
        .map_err(|_| NetError::Invalid("connect block"))?
        .map(Box::new);
    Ok(ParsedRequest {
        seq,
        digest,
        class,
        fields: ConnectFields {
            group,
            crc,
            game,
            current_protocol,
            min_protocol,
            password,
            extra,
            args,
            block,
        },
    })
}

/// A datagram taken apart with no connection state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    ChallengeRequest {
        seq: u32,
    },
    ChallengeResponse {
        seq: u32,
        digest: [u32; 4],
    },
    ConnectRequest(Box<ParsedRequest>),
    Reject {
        seq: u32,
        reason: Vec<u8>,
    },
    Accept(Accept),
    Disconnect {
        seq: u32,
        reason: Vec<u8>,
    },
    /// A game packet: its header, its rate prefix (data packets only) and the bit its payload starts at.
    Game {
        header: Header,
        rate: Option<RatePrefix>,
        payload_bit: usize,
    },
    /// A byte-0 type this task does not decode.
    Other(u8),
}

/// Decodes any datagram of either direction, the handshake's as `profile` writes them. An `Err` is a datagram the format does not
/// allow.
pub fn decode_datagram(
    table: &Table,
    profile: &HandshakeProfile,
    data: &[u8],
) -> Result<Decoded, NetError> {
    let first = *data.first().ok_or(NetError::Overrun)?;
    if first & 1 != 0 {
        let mut r = BitReader::new(data);
        let header = Header::read(&mut r)?;
        let rate = if header.ptype == PacketType::Data {
            Some(RatePrefix::read(&mut r)?)
        } else {
            None
        };
        return Ok(Decoded::Game {
            header,
            rate,
            payload_bit: r.position(),
        });
    }
    let types = &profile.types;
    Ok(match first {
        ty if ty == types.challenge_request => Decoded::ChallengeRequest {
            seq: handshake::peek_type(data)?.1,
        },
        ty if ty == types.challenge_response => {
            let (seq, digest) = types.parse_challenge_response(data)?;
            Decoded::ChallengeResponse { seq, digest }
        }
        ty if ty == types.connect_request => {
            Decoded::ConnectRequest(Box::new(parse_connect_request(table, profile, data)?))
        }
        ty if ty == types.connect_reject => {
            let (seq, reason) = types.parse_reject(table, data)?;
            Decoded::Reject { seq, reason }
        }
        ty if ty == types.connect_accept => {
            Decoded::Accept(handshake::parse_accept(profile, data)?)
        }
        ty if ty == types.disconnect => {
            let (seq, reason) = types.parse_disconnect(table, data)?;
            Decoded::Disconnect { seq, reason }
        }
        other => Decoded::Other(other),
    })
}

#[cfg(test)]
mod tests;
