//! QPACK (RFC 9204) with the static table and literals only. Both ends advertise a dynamic table of capacity 0 (the default:
//! no `QPACK_MAX_TABLE_CAPACITY` setting), so a field section that refers to a dynamic entry, or that asks for one
//! (`Required Insert Count` above 0), is a decoding error and the peer's encoder stream never legitimately carries an
//! instruction. One CONNECT and one response are all a session ever encodes.
//!
//! Every length is capped before a byte is kept: a field section is at most [`MAX_SECTION_BYTES`] on the wire and
//! [`MAX_FIELDS`] lines, a name at most [`MAX_NAME`] bytes and a value at most [`MAX_VALUE`] (decoded, Huffman or not).

use super::huffman;

/// The most bytes of one encoded field section (a browser's CONNECT is a few hundred).
pub const MAX_SECTION_BYTES: usize = 4 << 10;
/// The most field lines in one section.
pub const MAX_FIELDS: usize = 32;
/// The longest field name, decoded.
pub const MAX_NAME: usize = 128;
/// The longest field value, decoded (an `:authority` or `origin` is well under this).
pub const MAX_VALUE: usize = 1 << 10;

/// The static table (RFC 9204 appendix A), by index.
pub const STATIC: [(&str, &str); 99] = [
    (":authority", ""),
    (":path", "/"),
    ("age", "0"),
    ("content-disposition", ""),
    ("content-length", "0"),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("referer", ""),
    ("set-cookie", ""),
    (":method", "CONNECT"),
    (":method", "DELETE"),
    (":method", "GET"),
    (":method", "HEAD"),
    (":method", "OPTIONS"),
    (":method", "POST"),
    (":method", "PUT"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "103"),
    (":status", "200"),
    (":status", "304"),
    (":status", "404"),
    (":status", "503"),
    ("accept", "*/*"),
    ("accept", "application/dns-message"),
    ("accept-encoding", "gzip, deflate, br"),
    ("accept-ranges", "bytes"),
    ("access-control-allow-headers", "cache-control"),
    ("access-control-allow-headers", "content-type"),
    ("access-control-allow-origin", "*"),
    ("cache-control", "max-age=0"),
    ("cache-control", "max-age=2592000"),
    ("cache-control", "max-age=604800"),
    ("cache-control", "no-cache"),
    ("cache-control", "no-store"),
    ("cache-control", "public, max-age=31536000"),
    ("content-encoding", "br"),
    ("content-encoding", "gzip"),
    ("content-type", "application/dns-message"),
    ("content-type", "application/javascript"),
    ("content-type", "application/json"),
    ("content-type", "application/x-www-form-urlencoded"),
    ("content-type", "image/gif"),
    ("content-type", "image/jpeg"),
    ("content-type", "image/png"),
    ("content-type", "text/css"),
    ("content-type", "text/html; charset=utf-8"),
    ("content-type", "text/plain"),
    ("content-type", "text/plain;charset=utf-8"),
    ("range", "bytes=0-"),
    ("strict-transport-security", "max-age=31536000"),
    (
        "strict-transport-security",
        "max-age=31536000; includesubdomains",
    ),
    (
        "strict-transport-security",
        "max-age=31536000; includesubdomains; preload",
    ),
    ("vary", "accept-encoding"),
    ("vary", "origin"),
    ("x-content-type-options", "nosniff"),
    ("x-xss-protection", "1; mode=block"),
    (":status", "100"),
    (":status", "204"),
    (":status", "206"),
    (":status", "302"),
    (":status", "400"),
    (":status", "403"),
    (":status", "421"),
    (":status", "425"),
    (":status", "500"),
    ("accept-language", ""),
    ("access-control-allow-credentials", "FALSE"),
    ("access-control-allow-credentials", "TRUE"),
    ("access-control-allow-headers", "*"),
    ("access-control-allow-methods", "get"),
    ("access-control-allow-methods", "get, post, options"),
    ("access-control-allow-methods", "options"),
    ("access-control-expose-headers", "content-length"),
    ("access-control-request-headers", "content-type"),
    ("access-control-request-method", "get"),
    ("access-control-request-method", "post"),
    ("alt-svc", "clear"),
    ("authorization", ""),
    (
        "content-security-policy",
        "script-src 'none'; object-src 'none'; base-uri 'none'",
    ),
    ("early-data", "1"),
    ("expect-ct", ""),
    ("forwarded", ""),
    ("if-range", ""),
    ("origin", ""),
    ("purpose", "prefetch"),
    ("server", ""),
    ("timing-allow-origin", "*"),
    ("upgrade-insecure-requests", "1"),
    ("user-agent", ""),
    ("x-forwarded-for", ""),
    ("x-frame-options", "deny"),
    ("x-frame-options", "sameorigin"),
];

/// Why a field section could not be decoded (the connection is closed with `QPACK_DECOMPRESSION_FAILED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QpackError {
    /// The bytes end inside a field line or a prefix.
    Truncated,
    /// More bytes than [`MAX_SECTION_BYTES`].
    SectionTooLarge,
    /// More lines than [`MAX_FIELDS`].
    TooManyFields,
    /// A reference to the dynamic table, which has no entries here.
    DynamicReference,
    /// A static index past the table.
    BadIndex,
    /// A name or value past its cap.
    StringTooLong,
    /// A Huffman string that does not decode.
    Huffman,
    /// A name or value that is not UTF-8, or a name with an upper-case letter or a control character.
    BadString,
    /// A prefixed integer longer than four continuation bytes.
    BadInteger,
}

/// A decoded field section: the lines in order.
pub type Fields = Vec<(String, String)>;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, QpackError> {
        let b = *self.bytes.get(self.at).ok_or(QpackError::Truncated)?;
        self.at += 1;
        Ok(b)
    }

    fn peek(&self) -> Result<u8, QpackError> {
        self.bytes
            .get(self.at)
            .copied()
            .ok_or(QpackError::Truncated)
    }

    /// A prefixed integer (RFC 7541 section 5.1) whose first byte's low `prefix` bits start it; the high bits are the flags.
    fn integer(&mut self, prefix: u8) -> Result<(u8, usize), QpackError> {
        let first = self.byte()?;
        // An 8-bit prefix (the Required Insert Count) has no flag bits.
        let (flags, mask) = if prefix == 8 {
            (0, 0xffu8)
        } else {
            (first >> prefix, 0xffu8 >> (8 - prefix))
        };
        let mut value = usize::from(first & mask);
        if value < usize::from(mask) {
            return Ok((flags, value));
        }
        // Four continuation bytes reach 2^28, far past any length allowed here.
        for shift in (0..28).step_by(7) {
            let b = self.byte()?;
            value += usize::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok((flags, value));
            }
        }
        Err(QpackError::BadInteger)
    }

    /// A string literal: the `H` flag above `prefix` bits of length, then the bytes, at most `max` decoded.
    fn string(&mut self, prefix: u8, max: usize) -> Result<Vec<u8>, QpackError> {
        let (flags, len) = self.integer(prefix)?;
        let huffman = flags & 1 == 1;
        // The section is at most MAX_SECTION_BYTES, so `at + len` cannot overflow.
        if self.at + len > self.bytes.len() {
            return Err(QpackError::Truncated);
        }
        let raw = &self.bytes[self.at..self.at + len];
        self.at += len;
        if huffman {
            let mut out = Vec::with_capacity(len * 2);
            huffman::decode(raw, max, &mut out).map_err(|e| match e {
                huffman::HuffmanError::TooLong => QpackError::StringTooLong,
                huffman::HuffmanError::BadPadding => QpackError::Huffman,
            })?;
            Ok(out)
        } else if len > max {
            Err(QpackError::StringTooLong)
        } else {
            Ok(raw.to_vec())
        }
    }
}

fn static_entry(index: usize) -> Result<(&'static str, &'static str), QpackError> {
    STATIC.get(index).copied().ok_or(QpackError::BadIndex)
}

fn name_of(bytes: Vec<u8>) -> Result<String, QpackError> {
    if bytes.is_empty()
        || bytes
            .iter()
            .any(|&b| b.is_ascii_uppercase() || b.is_ascii_control() || b == b' ')
    {
        return Err(QpackError::BadString);
    }
    String::from_utf8(bytes).map_err(|_| QpackError::BadString)
}

fn value_of(bytes: Vec<u8>) -> Result<String, QpackError> {
    if bytes.iter().any(|&b| b == 0 || b == b'\n' || b == b'\r') {
        return Err(QpackError::BadString);
    }
    String::from_utf8(bytes).map_err(|_| QpackError::BadString)
}

/// Decodes one field section (a HEADERS frame's payload).
///
/// # Errors
/// [`QpackError`]: the section is malformed, refers to the dynamic table, or passes a cap.
pub fn decode(bytes: &[u8]) -> Result<Fields, QpackError> {
    if bytes.len() > MAX_SECTION_BYTES {
        return Err(QpackError::SectionTooLarge);
    }
    let mut r = Reader { bytes, at: 0 };
    // The prefix: the Required Insert Count (8-bit prefix) must be 0 with no dynamic table; the Delta Base (sign and a
    // 7-bit prefix) is then meaningless and only has to parse.
    let (_, required_insert_count) = r.integer(8)?;
    if required_insert_count != 0 {
        return Err(QpackError::DynamicReference);
    }
    r.integer(7)?;
    let mut fields = Vec::new();
    while r.at < bytes.len() {
        if fields.len() >= MAX_FIELDS {
            return Err(QpackError::TooManyFields);
        }
        let first = r.peek()?;
        let (name, value) = if first & 0b1000_0000 != 0 {
            // Indexed field line: 1 T index(6).
            let (flags, index) = r.integer(6)?;
            if flags & 1 == 0 {
                return Err(QpackError::DynamicReference);
            }
            let (n, v) = static_entry(index)?;
            (n.to_string(), v.to_string())
        } else if first & 0b1100_0000 == 0b0100_0000 {
            // Literal field line with name reference: 01 N T index(4), then the value.
            let (flags, index) = r.integer(4)?;
            if flags & 1 == 0 {
                return Err(QpackError::DynamicReference);
            }
            let (n, _) = static_entry(index)?;
            let value = value_of(r.string(7, MAX_VALUE)?)?;
            (n.to_string(), value)
        } else if first & 0b1110_0000 == 0b0010_0000 {
            // Literal field line with literal name: 001 N H length(3), the name, then the value.
            let name = name_of(r.string(3, MAX_NAME)?)?;
            let value = value_of(r.string(7, MAX_VALUE)?)?;
            (name, value)
        } else {
            // 0001: indexed with post-base index; 0000: literal with post-base name reference. Both are dynamic.
            return Err(QpackError::DynamicReference);
        };
        fields.push((name, value));
    }
    Ok(fields)
}

fn put_integer(out: &mut Vec<u8>, flags: u8, prefix: u8, value: usize) {
    let mask = usize::from(if prefix == 8 {
        0xffu8
    } else {
        0xffu8 >> (8 - prefix)
    });
    let head = if prefix == 8 { 0 } else { flags << prefix };
    if value < mask {
        out.push(head | value as u8);
        return;
    }
    out.push(head | mask as u8);
    let mut rest = value - mask;
    while rest >= 128 {
        out.push((rest as u8 & 0x7f) | 0x80);
        rest >>= 7;
    }
    out.push(rest as u8);
}

/// A string literal whose lowest flag bit, `H`, is 0: plain bytes.
fn put_string(out: &mut Vec<u8>, flags: u8, prefix: u8, text: &str) {
    put_integer(out, flags, prefix, text.len());
    out.extend_from_slice(text.as_bytes());
}

/// Encodes `fields` as one field section, with the static table where it has the line or the name and plain literals
/// otherwise.
#[must_use]
pub fn encode(fields: &[(&str, &str)]) -> Vec<u8> {
    let mut out = vec![0, 0];
    for &(name, value) in fields {
        if let Some(index) = STATIC.iter().position(|&(n, v)| n == name && v == value) {
            put_integer(&mut out, 0b11, 6, index);
        } else if let Some(index) = STATIC.iter().position(|&(n, _)| n == name) {
            put_integer(&mut out, 0b0101, 4, index);
            put_string(&mut out, 0, 7, value);
        } else {
            put_string(&mut out, 0b00100, 3, name);
            put_string(&mut out, 0, 7, value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connect() -> Vec<(&'static str, &'static str)> {
        vec![
            (":method", "CONNECT"),
            (":protocol", "webtransport"),
            (":scheme", "https"),
            (":authority", "127.0.0.1:28000"),
            (":path", "/"),
            ("origin", "http://localhost:28000"),
        ]
    }

    #[test]
    fn a_section_round_trips_through_the_static_table_and_literals() {
        let fields = connect();
        let wire = encode(&fields);
        let back = decode(&wire).expect("decodes");
        let expect: Fields = fields
            .iter()
            .map(|&(n, v)| (n.to_string(), v.to_string()))
            .collect();
        assert_eq!(back, expect);
        // `:method CONNECT` is one byte (static 15), `:path /` one byte (static 1).
        assert_eq!(wire[2], 0b1100_0000 | 15);
    }

    #[test]
    fn huffman_literals_decode_like_plain_ones() {
        // Literal name and value, both Huffman: 001 N=0 H=1 length(3), then H=1 length(7).
        let mut wire = vec![0, 0];
        let name = huffman::encode(b"origin");
        put_integer(&mut wire, 0b00101, 3, name.len());
        wire.extend_from_slice(&name);
        let value = huffman::encode(b"https://games.example");
        put_integer(&mut wire, 0b1, 7, value.len());
        wire.extend_from_slice(&value);
        // Name reference with a Huffman value: 01 N=0 T=1 index(4) of `:authority`, then 1 len(7).
        put_integer(&mut wire, 0b0101, 4, 0);
        let authority = huffman::encode(b"games.example:4433");
        put_integer(&mut wire, 0b1, 7, authority.len());
        wire.extend_from_slice(&authority);
        let fields = decode(&wire).expect("decodes");
        assert_eq!(
            fields,
            vec![
                ("origin".to_string(), "https://games.example".to_string()),
                (":authority".to_string(), "games.example:4433".to_string()),
            ]
        );
    }

    #[test]
    fn dynamic_references_bad_indexes_and_caps_are_refused() {
        assert_eq!(
            decode(&[1, 0]),
            Err(QpackError::DynamicReference),
            "required insert count 1"
        );
        assert_eq!(
            decode(&[0, 0, 0b1000_0000]),
            Err(QpackError::DynamicReference),
            "dynamic indexed"
        );
        assert_eq!(
            decode(&[0, 0, 0b0100_0000, 0]),
            Err(QpackError::DynamicReference),
            "dynamic name ref"
        );
        assert_eq!(
            decode(&[0, 0, 0b0001_0000]),
            Err(QpackError::DynamicReference),
            "post-base"
        );
        assert_eq!(
            decode(&[0, 0, 0b0000_0000, 0]),
            Err(QpackError::DynamicReference),
            "post-base name"
        );
        assert_eq!(
            decode(&[0, 0, 0b1100_0000 | 63, 36]),
            Err(QpackError::BadIndex),
            "index 99"
        );
        assert_eq!(
            decode(&[0, 0, 0b1100_0000 | 63]),
            Err(QpackError::Truncated)
        );
        assert_eq!(
            decode(&[0, 0, 0b0010_0000 | 7, 0x81, 0x81, 0x81, 0x81, 0x81]),
            Err(QpackError::BadInteger)
        );
        let mut many = vec![0, 0];
        many.extend(std::iter::repeat_n(0b1100_0000 | 15, MAX_FIELDS + 1));
        assert_eq!(decode(&many), Err(QpackError::TooManyFields));
        let long = vec![0; MAX_SECTION_BYTES + 1];
        assert_eq!(decode(&long), Err(QpackError::SectionTooLarge));
        let mut big_value = vec![0, 0, 0b0101_0000];
        put_integer(&mut big_value, 0, 7, MAX_VALUE + 1);
        big_value.extend(std::iter::repeat_n(b'a', MAX_VALUE + 1));
        assert_eq!(decode(&big_value), Err(QpackError::StringTooLong));
        let mut upper = vec![0, 0];
        put_string(&mut upper, 0b00100, 3, "Origin");
        put_string(&mut upper, 0, 7, "x");
        assert_eq!(decode(&upper), Err(QpackError::BadString));
        assert_eq!(decode(&[0, 0]).expect("empty"), Fields::new());
    }

    #[test]
    fn the_static_table_has_the_rfc_shape() {
        assert_eq!(STATIC[15], (":method", "CONNECT"));
        assert_eq!(STATIC[25], (":status", "200"));
        assert_eq!(STATIC[23], (":scheme", "https"));
        assert_eq!(STATIC[90], ("origin", ""));
        assert_eq!(STATIC[98], ("x-frame-options", "sameorigin"));
    }
}
