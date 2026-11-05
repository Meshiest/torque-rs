//! The one thing the data-channel socket reads of a datagram before a peer owns it: whether it is a STUN binding request,
//! and which of our ICE user fragments it names. Everything else from an address no peer has answered is dropped unread.

/// STUN's magic cookie (RFC 5389), bytes 4 to 8 of every STUN message.
const MAGIC_COOKIE: [u8; 4] = [0x21, 0x12, 0xa4, 0x42];
/// A binding request's message type.
const BINDING_REQUEST: [u8; 2] = [0x00, 0x01];
/// The USERNAME attribute.
const USERNAME: u16 = 0x0006;
/// The XOR-MAPPED-ADDRESS attribute.
const XOR_MAPPED_ADDRESS: u16 = 0x0020;
/// The most attributes looked at before the USERNAME (a browser puts it first or second).
const MAX_ATTRIBUTES: usize = 16;

/// The local user fragment a STUN binding request names (its USERNAME is `<ours>:<theirs>`), if the datagram is one
/// whose header and attributes are well formed up to that attribute.
pub(crate) fn binding_ufrag(datagram: &[u8]) -> Option<&[u8]> {
    let header = datagram.get(..20)?;
    if header.get(..2)? != BINDING_REQUEST || header.get(4..8)? != MAGIC_COOKIE {
        return None;
    }
    let len = usize::from(u16::from_be_bytes([*header.get(2)?, *header.get(3)?]));
    if len % 4 != 0 || 20 + len != datagram.len() {
        return None;
    }
    let mut at = 20;
    for _ in 0..MAX_ATTRIBUTES {
        let head = datagram.get(at..at + 4)?;
        let kind = u16::from_be_bytes([*head.first()?, *head.get(1)?]);
        let size = usize::from(u16::from_be_bytes([*head.get(2)?, *head.get(3)?]));
        let value = datagram.get(at + 4..at + 4 + size)?;
        if kind == USERNAME {
            let colon = value.iter().position(|&b| b == b':')?;
            return value.get(..colon).filter(|u| !u.is_empty());
        }
        at += 4 + size.div_ceil(4) * 4;
    }
    None
}

/// A binding request with transaction `id` and no attributes (asking a STUN server which address it sees us at).
pub(crate) fn binding_request(id: &[u8; 12]) -> [u8; 20] {
    let mut d = [0u8; 20];
    d[..2].copy_from_slice(&BINDING_REQUEST);
    d[4..8].copy_from_slice(&MAGIC_COOKIE);
    d[8..].copy_from_slice(id);
    d
}

/// The XOR-MAPPED-ADDRESS of a binding success response to transaction `id` (IPv4 only), if the datagram is one.
pub(crate) fn mapped_address(datagram: &[u8], id: &[u8; 12]) -> Option<std::net::SocketAddr> {
    let header = datagram.get(..20)?;
    if header.get(..2)? != [0x01, 0x01]
        || header.get(4..8)? != MAGIC_COOKIE
        || header.get(8..20)? != id
    {
        return None;
    }
    let len = usize::from(u16::from_be_bytes([*header.get(2)?, *header.get(3)?]));
    if len % 4 != 0 || 20 + len != datagram.len() {
        return None;
    }
    let mut at = 20;
    for _ in 0..MAX_ATTRIBUTES {
        let head = datagram.get(at..at + 4)?;
        let kind = u16::from_be_bytes([*head.first()?, *head.get(1)?]);
        let size = usize::from(u16::from_be_bytes([*head.get(2)?, *head.get(3)?]));
        let value = datagram.get(at + 4..at + 4 + size)?;
        if kind == XOR_MAPPED_ADDRESS && size == 8 && value.get(1) == Some(&1) {
            let port = u16::from_be_bytes([*value.get(2)?, *value.get(3)?]) ^ 0x2112;
            let mut ip = [0u8; 4];
            for (i, b) in ip.iter_mut().enumerate() {
                *b = value.get(4 + i)? ^ MAGIC_COOKIE.get(i)?;
            }
            return Some(std::net::SocketAddr::from((ip, port)));
        }
        at += 4 + size.div_ceil(4) * 4;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(attributes: &[(u16, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (kind, value) in attributes {
            body.extend_from_slice(&kind.to_be_bytes());
            body.extend_from_slice(&(value.len() as u16).to_be_bytes());
            body.extend_from_slice(value);
            body.resize(body.len().div_ceil(4) * 4, 0);
        }
        let mut d = vec![0x00, 0x01];
        d.extend_from_slice(&(body.len() as u16).to_be_bytes());
        d.extend_from_slice(&MAGIC_COOKIE);
        d.extend_from_slice(&[7; 12]);
        d.extend_from_slice(&body);
        d
    }

    #[test]
    fn a_binding_answer_says_where_we_are_seen() {
        let id = [3u8; 12];
        let req = binding_request(&id);
        assert_eq!(binding_ufrag(&req), None, "it names no fragment");
        // 192.0.2.7:40000 as a STUN server writes it (RFC 5389: port XOR 0x2112, address XOR the cookie).
        let port = (40000u16 ^ 0x2112).to_be_bytes();
        let ip = [192 ^ 0x21, 0x12, 2 ^ 0xa4, 7 ^ 0x42];
        let mut d = vec![0x01, 0x01, 0, 12];
        d.extend_from_slice(&MAGIC_COOKIE);
        d.extend_from_slice(&id);
        d.extend_from_slice(&[
            0x00, 0x20, 0, 8, 0, 1, port[0], port[1], ip[0], ip[1], ip[2], ip[3],
        ]);
        assert_eq!(
            mapped_address(&d, &id),
            Some("192.0.2.7:40000".parse().unwrap())
        );
        assert_eq!(mapped_address(&d, &[4; 12]), None, "another transaction");
        let mut v6 = d.clone();
        v6[25] = 2;
        assert_eq!(mapped_address(&v6, &id), None);
        assert_eq!(mapped_address(&d[..d.len() - 1], &id), None);
        for cut in 0..d.len() {
            let _ = mapped_address(&d[..cut], &id);
        }
    }

    #[test]
    fn a_binding_request_names_our_fragment() {
        let d = request(&[(0x0024, &[0, 0, 0, 1]), (USERNAME, b"abcd:wxyz")]);
        assert_eq!(binding_ufrag(&d), Some(&b"abcd"[..]));
        // Not a request, a bad cookie, a length that is not the datagram's, no colon, an empty fragment, no username.
        let mut response = d.clone();
        response[1] = 0x01;
        response[0] = 0x01;
        assert_eq!(binding_ufrag(&response), None);
        let mut cookie = d.clone();
        cookie[5] ^= 1;
        assert_eq!(binding_ufrag(&cookie), None);
        let mut long = d.clone();
        long.push(0);
        assert_eq!(binding_ufrag(&long), None);
        assert_eq!(binding_ufrag(&request(&[(USERNAME, b"abcd")])), None);
        assert_eq!(binding_ufrag(&request(&[(USERNAME, b":wxyz")])), None);
        assert_eq!(binding_ufrag(&request(&[(0x0008, &[0; 20])])), None);
        // An attribute running past the end.
        let mut cut = request(&[(USERNAME, b"abcd:wxyz")]);
        cut[22] = 0;
        cut[23] = 200;
        assert_eq!(binding_ufrag(&cut), None);
        assert_eq!(binding_ufrag(&[0x00, 0x01, 0, 0]), None);
        // Garbage of every first byte never panics.
        for b in 0..=255u8 {
            let mut g = d.clone();
            g[0] = b;
            let _ = binding_ufrag(&g);
            let _ = binding_ufrag(&g[..g.len() / 2]);
        }
    }
}
