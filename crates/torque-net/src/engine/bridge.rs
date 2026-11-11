//! What an engine-mode connect carries, as the connect fields the gate, the auth providers and the scripts read in both modes
//! ([`ConnectFields`], [`ConnectBlock`], [`AuthContext`]), so a host judges a connect the same way whichever framing brought it.
//!
//! The connect message's manifest, ticket and mods section are the bytes their own codecs write (an empty one is absent); its
//! password and arguments are text. A ticket is made for, and checked against, the server's address, the nonce the server
//! chose for this session (its challenge) and the server identity the client's link verified ([`ticket_context`]), so a
//! ticket made for one connection is refused on any other.

use std::net::SocketAddr;

use super::framing::{Connect, TableDigests, Versions};
use super::link::{ChannelBinding, ServerIdentity};
use crate::connect::{AuthContext, ConnectBlock};
use crate::handshake::ConnectFields;
use crate::manifest::Manifest;
use crate::modsync::ModsSection;
use crate::profile::HandshakeProfile;

/// The connect a client of `profile` sends: its game, the framing versions it speaks, `block` (manifest, ticket, mods), the
/// password and the arguments as text (bytes that are not UTF-8 are replaced).
///
/// # Errors
/// The block's manifest does not encode (the text says why).
pub fn connect_of(
    profile: &HandshakeProfile,
    block: Option<&ConnectBlock>,
    password: &[u8],
    args: &[Vec<u8>],
) -> Result<Connect, String> {
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    let manifest = block
        .and_then(|b| b.manifest.as_ref())
        .map(Manifest::encode)
        .transpose()
        .map_err(|e| format!("this build cannot send its manifest: {e}"))?
        .unwrap_or_default();
    Ok(Connect {
        versions: Versions::CURRENT,
        game: text(&profile.game),
        manifest,
        ticket: block.and_then(|b| b.ticket.clone()).unwrap_or_default(),
        mods: block
            .and_then(|b| b.mods.as_ref())
            .map(ModsSection::encode)
            .unwrap_or_default(),
        password: text(password),
        args: args.iter().map(|a| text(a)).collect(),
    })
}

/// The fields a server judges `connect` by: the profile's game and protocol versions (the session already matched the game
/// and the framing version), the password and the arguments, and the block of what the connect carried.
///
/// # Errors
/// The manifest or the mods section does not decode: the text is the reject's.
pub fn fields_of(connect: &Connect, profile: &HandshakeProfile) -> Result<ConnectFields, String> {
    let manifest = (!connect.manifest.is_empty())
        .then(|| Manifest::decode(&connect.manifest))
        .transpose()
        .map_err(|e| format!("the manifest does not read: {e}"))?;
    let mods = (!connect.mods.is_empty())
        .then(|| ModsSection::decode(&connect.mods))
        .transpose()
        .map_err(|e| format!("the mods section does not read: {e}"))?;
    let ticket = (!connect.ticket.is_empty()).then(|| connect.ticket.clone());
    let args = connect.args.iter().map(|a| a.as_bytes().to_vec()).collect();
    let block = ConnectBlock {
        manifest,
        ticket,
        mods,
    };
    Ok(ConnectFields::new(profile, connect.password.as_bytes(), args).with_block(block))
}

/// What a ticket for the server at `server` is made for in the session the server challenged with `challenge`: the server's
/// address, the challenge in place of TGE's, the first eight bytes of the server's identity when the link verified one
/// (none: zeros), and the link's channel binding when it derives one (none: nothing, so the bytes are those of a link
/// without one). The sequence TGE binds to is 0: the challenge is per session already.
#[must_use]
pub fn ticket_context(
    server: SocketAddr,
    challenge: u64,
    identity: Option<ServerIdentity>,
    binding: Option<ChannelBinding>,
) -> AuthContext {
    let word = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let id = identity.map_or([0; 32], |i| i.0);
    AuthContext {
        server,
        sequence: 0,
        challenge: [
            (challenge >> 32) as u32,
            challenge as u32,
            word(&id[..4]),
            word(&id[4..8]),
        ],
        binding,
    }
}

/// The digests of the tables both sides must agree on, from a manifest (an engine with none has empty tables).
#[must_use]
pub fn table_digests(manifest: Option<&Manifest>) -> TableDigests {
    manifest.map_or_else(TableDigests::default, |m| TableDigests {
        channels: m.channels.digest(),
        rpcs: m.rpcs.digest(),
        uploads: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connect_round_trips_into_the_fields_the_gate_reads() {
        let profile = HandshakeProfile::default();
        let block = ConnectBlock {
            manifest: Some(Manifest::new(1, 2).with_channels(["a.b"])),
            ticket: Some(vec![1, 2, 3]),
            mods: None,
        };
        let connect =
            connect_of(&profile, Some(&block), b"pw", &[b"one".to_vec()]).expect("a connect");
        let fields = fields_of(&connect, &profile).expect("fields");
        assert_eq!(fields.password, b"pw");
        assert_eq!(fields.args, [b"one".to_vec()]);
        assert_eq!(fields.game, profile.game);
        assert_eq!(fields.block.as_deref(), Some(&block));
        assert_eq!(
            table_digests(block.manifest.as_ref()).channels,
            Manifest::new(1, 2).with_channels(["a.b"]).channels.digest()
        );
    }

    #[test]
    fn a_connect_with_nothing_carries_no_block_entries_and_bad_bytes_are_refused() {
        let profile = HandshakeProfile::default();
        let connect = connect_of(&profile, None, b"", &[]).expect("a connect");
        let fields = fields_of(&connect, &profile).expect("fields");
        assert_eq!(fields.block.as_deref(), Some(&ConnectBlock::default()));
        let bad = Connect {
            manifest: vec![0xff; 9],
            ..connect
        };
        assert!(fields_of(&bad, &profile).is_err());
    }

    #[test]
    fn the_ticket_context_differs_per_challenge_server_identity_and_binding() {
        let a: SocketAddr = "10.0.0.1:28000".parse().expect("an address");
        let b: SocketAddr = "10.0.0.2:28000".parse().expect("an address");
        let cx = |s, c, i, k| ticket_context(s, c, i, k).encode();
        assert_ne!(cx(a, 1, None, None), cx(a, 2, None, None));
        assert_ne!(cx(a, 1, None, None), cx(b, 1, None, None));
        assert_ne!(cx(a, 1, None, None), cx(a, 1 << 32, None, None));
        assert_ne!(
            cx(a, 1, None, None),
            cx(a, 1, Some(ServerIdentity([9; 32])), None)
        );
        let bound = Some(ChannelBinding([7; 32]));
        assert_ne!(cx(a, 1, None, None), cx(a, 1, None, bound));
        assert_ne!(
            cx(a, 1, None, bound),
            cx(a, 1, None, Some(ChannelBinding([8; 32])))
        );
        // A link without a binding writes exactly the bytes it wrote before bindings existed.
        assert_eq!(cx(a, 1, None, None).len(), 1 + 4 + 2 + 4 + 16);
        assert_eq!(cx(a, 1, None, bound).len(), 1 + 4 + 2 + 4 + 16 + 32);
    }
}
