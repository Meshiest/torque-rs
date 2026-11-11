//! The bytes of HTTP/3 (RFC 9114) this session needs: variable-length integers (RFC 9000 section 16), frames (a type, a
//! length and a payload), the SETTINGS frame, the unidirectional stream types, and the capsules of a WebTransport session
//! (RFC 9297 section 3.2, draft-ietf-webtrans-http3). A frame's length is checked against its cap before any of its payload
//! is kept.

use quinn_proto::VarInt;
use quinn_proto::coding::Codec;

/// The most bytes a frame's payload may declare on any stream; a longer one closes the connection (`H3_EXCESSIVE_LOAD`).
/// A browser's SETTINGS and its CONNECT's HEADERS are each well under 1 KiB.
pub const MAX_FRAME_PAYLOAD: usize = 8 << 10;
/// The most settings one SETTINGS frame may carry.
pub const MAX_SETTINGS: usize = 32;

/// Frame types (RFC 9114 section 7.2).
pub mod frame {
    pub const DATA: u64 = 0x00;
    pub const HEADERS: u64 = 0x01;
    pub const SETTINGS: u64 = 0x04;
    pub const GOAWAY: u64 = 0x07;
    /// The WebTransport stream frame (draft-ietf-webtrans-http3 section 4.2): never accepted here.
    pub const WEBTRANSPORT_STREAM: u64 = 0x41;
}

/// Unidirectional stream types (RFC 9114 section 6.2, RFC 9204 section 4.2, draft-ietf-webtrans-http3 section 4.3).
pub mod stream {
    pub const CONTROL: u64 = 0x00;
    pub const PUSH: u64 = 0x01;
    pub const QPACK_ENCODER: u64 = 0x02;
    pub const QPACK_DECODER: u64 = 0x03;
    pub const WEBTRANSPORT: u64 = 0x54;
}

/// Settings identifiers (RFC 9114 section 7.2.4.1, RFC 9220, RFC 9297, draft-ietf-webtrans-http3 section 3.1).
pub mod setting {
    pub const QPACK_MAX_TABLE_CAPACITY: u64 = 0x01;
    pub const MAX_FIELD_SECTION_SIZE: u64 = 0x06;
    pub const QPACK_BLOCKED_STREAMS: u64 = 0x07;
    pub const ENABLE_CONNECT_PROTOCOL: u64 = 0x08;
    pub const H3_DATAGRAM: u64 = 0x33;
    /// `H3_DATAGRAM` as draft-ietf-masque-h3-datagram-04 numbered it; some browsers still send both.
    pub const H3_DATAGRAM_DRAFT: u64 = 0xff_d277;
    /// `ENABLE_WEBTRANSPORT` of draft-ietf-webtrans-http3 up to -02.
    pub const ENABLE_WEBTRANSPORT: u64 = 0x2b60_3742;
    /// `WT_MAX_SESSIONS` of draft-ietf-webtrans-http3-07 and later: the sessions the sender takes.
    pub const WT_MAX_SESSIONS: u64 = 0xc671_706a;
}

/// Error codes (RFC 9114 section 8.1, RFC 9204 section 6).
pub mod error {
    pub const H3_NO_ERROR: u64 = 0x100;
    pub const H3_GENERAL_PROTOCOL_ERROR: u64 = 0x101;
    pub const H3_STREAM_CREATION_ERROR: u64 = 0x103;
    pub const H3_CLOSED_CRITICAL_STREAM: u64 = 0x104;
    pub const H3_FRAME_UNEXPECTED: u64 = 0x105;
    pub const H3_FRAME_ERROR: u64 = 0x106;
    pub const H3_EXCESSIVE_LOAD: u64 = 0x107;
    pub const H3_SETTINGS_ERROR: u64 = 0x109;
    pub const H3_MISSING_SETTINGS: u64 = 0x10a;
    pub const H3_REQUEST_REJECTED: u64 = 0x10b;
    pub const H3_MESSAGE_ERROR: u64 = 0x10e;
    pub const QPACK_DECOMPRESSION_FAILED: u64 = 0x200;
    pub const QPACK_ENCODER_STREAM_ERROR: u64 = 0x201;
    pub const QPACK_DECODER_STREAM_ERROR: u64 = 0x202;
}

/// Capsule types on a CONNECT stream (draft-ietf-webtrans-http3 section 5).
pub mod capsule {
    pub const CLOSE_WEBTRANSPORT_SESSION: u64 = 0x2843;
    pub const DRAIN_WEBTRANSPORT_SESSION: u64 = 0x78ae;
}

/// Reads one variable-length integer from the front of `bytes`: the value and the bytes after it; None when the bytes end
/// inside it.
#[must_use]
pub fn varint(bytes: &[u8]) -> Option<(u64, &[u8])> {
    let mut cursor = bytes;
    let value = VarInt::decode(&mut cursor).ok()?;
    Some((value.into_inner(), cursor))
}

/// Appends `value` as a variable-length integer (at most 2^62 - 1; larger is written as that).
pub fn put_varint(out: &mut Vec<u8>, value: u64) {
    VarInt::from_u64(value).unwrap_or(VarInt::MAX).encode(out);
}

/// Where a frame read stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameRead<'a> {
    /// A whole frame: its type, its payload and the bytes after it.
    Frame {
        kind: u64,
        payload: &'a [u8],
        rest: &'a [u8],
    },
    /// The bytes end inside the frame; more are needed.
    Partial,
    /// The frame declares a payload longer than [`MAX_FRAME_PAYLOAD`].
    TooLong,
}

/// Reads one frame from the front of `bytes`.
#[must_use]
pub fn read_frame(bytes: &[u8]) -> FrameRead<'_> {
    let Some((kind, rest)) = varint(bytes) else {
        return FrameRead::Partial;
    };
    let Some((len, rest)) = varint(rest) else {
        return FrameRead::Partial;
    };
    if len > MAX_FRAME_PAYLOAD as u64 {
        return FrameRead::TooLong;
    }
    let len = len as usize;
    if rest.len() < len {
        return FrameRead::Partial;
    }
    let (payload, rest) = rest.split_at(len);
    FrameRead::Frame {
        kind,
        payload,
        rest,
    }
}

/// Appends a frame of `kind` with `payload`.
pub fn put_frame(out: &mut Vec<u8>, kind: u64, payload: &[u8]) {
    put_varint(out, kind);
    put_varint(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

/// The settings a SETTINGS frame carried, as pairs in order.
pub type Settings = Vec<(u64, u64)>;

/// Why a SETTINGS payload could not be read (`H3_SETTINGS_ERROR`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsError {
    /// The bytes end inside a setting.
    Truncated,
    /// More than [`MAX_SETTINGS`] settings.
    TooMany,
    /// A setting that appears twice (RFC 9114 section 7.2.4).
    Duplicate,
    /// An HTTP/2 setting (0x00 to 0x05, reserved in HTTP/3).
    Reserved,
}

/// Reads a SETTINGS frame's payload.
///
/// # Errors
/// [`SettingsError`].
pub fn read_settings(mut payload: &[u8]) -> Result<Settings, SettingsError> {
    let mut out = Settings::new();
    while !payload.is_empty() {
        if out.len() >= MAX_SETTINGS {
            return Err(SettingsError::TooMany);
        }
        let (id, rest) = varint(payload).ok_or(SettingsError::Truncated)?;
        let (value, rest) = varint(rest).ok_or(SettingsError::Truncated)?;
        if (0x00..=0x05).contains(&id) && id != setting::QPACK_MAX_TABLE_CAPACITY {
            return Err(SettingsError::Reserved);
        }
        if out.iter().any(|&(seen, _)| seen == id) {
            return Err(SettingsError::Duplicate);
        }
        out.push((id, value));
        payload = rest;
    }
    Ok(out)
}

/// The value of setting `id` in `settings`, if it is there.
#[must_use]
pub fn setting_value(settings: &[(u64, u64)], id: u64) -> Option<u64> {
    settings.iter().find(|&&(s, _)| s == id).map(|&(_, v)| v)
}

/// A SETTINGS frame's payload of `settings`.
#[must_use]
pub fn put_settings(settings: &[(u64, u64)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(settings.len() * 10);
    for &(id, value) in settings {
        put_varint(&mut out, id);
        put_varint(&mut out, value);
    }
    out
}

/// The capsule a DATA frame's payload on the CONNECT stream carries, as far as this session reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capsule {
    /// The peer closes the session: an application code and a reason (at most 1,024 bytes by the draft; cut here to
    /// [`MAX_CLOSE_REASON`]).
    Close { code: u32, reason: String },
    /// The peer will close soon: nothing to do.
    Drain,
    /// Any other capsule: ignored.
    Other(u64),
}

/// The most bytes of a close capsule's reason kept.
pub const MAX_CLOSE_REASON: usize = 256;

/// Reads the capsules of a DATA payload (several may share one frame); a truncated or over-long one ends the list.
#[must_use]
pub fn read_capsules(mut payload: &[u8]) -> Vec<Capsule> {
    let mut out = Vec::new();
    while !payload.is_empty() {
        let FrameRead::Frame {
            kind,
            payload: body,
            rest,
        } = read_frame(payload)
        else {
            break;
        };
        payload = rest;
        out.push(match kind {
            capsule::CLOSE_WEBTRANSPORT_SESSION => {
                let code = body
                    .get(..4)
                    .and_then(|b| b.try_into().ok())
                    .map_or(0, u32::from_be_bytes);
                let reason = body.get(4..).unwrap_or_default();
                let reason = &reason[..reason.len().min(MAX_CLOSE_REASON)];
                Capsule::Close {
                    code,
                    reason: String::from_utf8_lossy(reason).into_owned(),
                }
            }
            capsule::DRAIN_WEBTRANSPORT_SESSION => Capsule::Drain,
            other => Capsule::Other(other),
        });
        if out.len() >= 8 {
            break;
        }
    }
    out
}

/// A CLOSE_WEBTRANSPORT_SESSION capsule in a DATA frame.
#[must_use]
pub fn put_close_capsule(code: u32, reason: &str) -> Vec<u8> {
    let reason = reason.as_bytes();
    let reason = &reason[..reason.len().min(MAX_CLOSE_REASON)];
    let mut body = Vec::with_capacity(4 + reason.len());
    body.extend_from_slice(&code.to_be_bytes());
    body.extend_from_slice(reason);
    let mut capsule = Vec::new();
    put_frame(&mut capsule, capsule::CLOSE_WEBTRANSPORT_SESSION, &body);
    let mut out = Vec::new();
    put_frame(&mut out, frame::DATA, &capsule);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_and_settings_round_trip_and_the_caps_hold() {
        let settings = [
            (setting::ENABLE_CONNECT_PROTOCOL, 1),
            (setting::H3_DATAGRAM, 1),
            (setting::WT_MAX_SESSIONS, 1),
            (setting::ENABLE_WEBTRANSPORT, 1),
        ];
        let mut wire = Vec::new();
        put_frame(&mut wire, frame::SETTINGS, &put_settings(&settings));
        wire.extend_from_slice(&[0x07, 0x01, 0x00]);
        let FrameRead::Frame {
            kind,
            payload,
            rest,
        } = read_frame(&wire)
        else {
            panic!("a frame")
        };
        assert_eq!(kind, frame::SETTINGS);
        assert_eq!(read_settings(payload).expect("settings"), settings.to_vec());
        assert_eq!(setting_value(&settings, setting::WT_MAX_SESSIONS), Some(1));
        assert_eq!(setting_value(&settings, 0x99), None);
        assert!(matches!(
            read_frame(rest),
            FrameRead::Frame {
                kind: 0x07,
                payload: &[0],
                rest: &[]
            }
        ));
        for cut in 0..wire.len() - 3 {
            assert_eq!(read_frame(&wire[..cut]), FrameRead::Partial, "cut {cut}");
        }
        let mut long = Vec::new();
        put_varint(&mut long, frame::HEADERS);
        put_varint(&mut long, MAX_FRAME_PAYLOAD as u64 + 1);
        assert_eq!(read_frame(&long), FrameRead::TooLong);
        assert_eq!(
            read_frame(&[0x04, 0x44]),
            FrameRead::Partial,
            "a two-byte length cut"
        );
        let mut big = Vec::new();
        put_varint(&mut big, 0x3fff_ffff_ffff_ffff);
        put_varint(&mut big, 0x3fff_ffff_ffff_ffff);
        assert_eq!(read_frame(&big), FrameRead::TooLong);
    }

    #[test]
    fn bad_settings_are_refused() {
        assert_eq!(read_settings(&[0x33]), Err(SettingsError::Truncated));
        assert_eq!(read_settings(&[0x02, 0x01]), Err(SettingsError::Reserved));
        assert_eq!(
            read_settings(&[0x33, 1, 0x33, 1]),
            Err(SettingsError::Duplicate)
        );
        let many = put_settings(
            &(0..=MAX_SETTINGS as u64)
                .map(|i| (0x100 + i, 1))
                .collect::<Vec<_>>(),
        );
        assert_eq!(read_settings(&many), Err(SettingsError::TooMany));
        assert_eq!(read_settings(&[]).expect("empty"), Settings::new());
    }

    #[test]
    fn capsules_are_read_and_a_close_round_trips() {
        let wire = put_close_capsule(7, "bye");
        let FrameRead::Frame { kind, payload, .. } = read_frame(&wire) else {
            panic!("a DATA frame")
        };
        assert_eq!(kind, frame::DATA);
        assert_eq!(
            read_capsules(payload),
            vec![Capsule::Close {
                code: 7,
                reason: "bye".into()
            }]
        );
        let mut two = Vec::new();
        put_frame(&mut two, 0x17, b"x");
        put_frame(&mut two, capsule::DRAIN_WEBTRANSPORT_SESSION, b"");
        put_frame(&mut two, capsule::CLOSE_WEBTRANSPORT_SESSION, b"\x00\x00");
        two.push(0x2f);
        assert_eq!(
            read_capsules(&two),
            vec![
                Capsule::Other(0x17),
                Capsule::Drain,
                Capsule::Close {
                    code: 0,
                    reason: String::new()
                }
            ]
        );
        let long = "r".repeat(MAX_CLOSE_REASON + 10);
        let wire = put_close_capsule(1, &long);
        let FrameRead::Frame { payload, .. } = read_frame(&wire) else {
            panic!("a DATA frame")
        };
        assert!(
            matches!(&read_capsules(payload)[..], [Capsule::Close { reason, .. }] if reason.len() == MAX_CLOSE_REASON)
        );
    }
}
