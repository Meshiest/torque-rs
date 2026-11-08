use std::marker::PhantomData;
use std::net::SocketAddr;

use torque_net::connect::{AuthContext, ConnectGate, SharedSecret};
use torque_net::handshake::ConnectFields;

use super::*;
use crate::{BuildError, Plugin, PluginId, PluginType};

/// A channel with nothing to say.
#[derive(Default)]
struct Nothing;

impl torque_net::chunk::registry::ChunkChannel for Nothing {}

/// A test plugin's name: each one is a type, as plugins are.
trait Name: 'static {
    const ID: &'static str;
}

macro_rules! names {
    ($($ty:ident = $id:literal),* $(,)?) => {
        $(
            struct $ty;
            impl Name for $ty {
                const ID: &'static str = $id;
            }
        )*
    };
}

names!(
    Rope = "torque.rope",
    Arena = "example.arena",
    Local = "local.only",
    AuthOne = "auth.one",
    AuthTwo = "auth.two",
    ProtocolOne = "protocol.one",
    ProtocolTwo = "protocol.two",
    Auth = "auth.test",
);

/// A plugin with a network version and a body.
struct Net<N: Name> {
    version: Option<u32>,
    body: fn(&mut EngineBuilder),
    name: PhantomData<N>,
}

impl<N: Name> Plugin for Net<N> {
    fn net_version(&self) -> Option<u32> {
        self.version
    }

    fn build(&self, engine: &mut EngineBuilder) {
        (self.body)(engine);
    }
}

impl<N: Name> PluginType for Net<N> {
    const ID: PluginId = PluginId::new(N::ID);
}

fn plugin<N: Name>(version: Option<u32>, body: fn(&mut EngineBuilder)) -> Net<N> {
    Net {
        version,
        body,
        name: PhantomData,
    }
}

fn nothing(_: &mut EngineBuilder) {}

fn from() -> SocketAddr {
    SocketAddr::from(([10, 0, 0, 1], 4000))
}

fn cx() -> AuthContext {
    AuthContext {
        server: SocketAddr::from(([192, 0, 2, 1], 28_000)),
        sequence: 3,
        challenge: [9, 8, 7, 6],
    }
}

fn fields(block: Option<ConnectBlock>) -> ConnectFields {
    let mut f = ConnectFields::standard(b"", Vec::new());
    f.block = block.map(Box::new);
    f
}

#[test]
fn an_engine_that_says_nothing_sends_no_block_and_judges_no_one() {
    let engine = EngineBuilder::new()
        .add_plugins(plugin::<Local>(None, nothing))
        .build()
        .unwrap();
    let profile = HandshakeProfile::default();
    assert!(!engine.is_networked());
    assert_eq!(engine.connect_block(&cx()), Ok(None));
    assert!(engine.connect_gate(&profile).is_none());
    assert!(engine.auth().is_none());
    assert!(engine.net_manifest(&profile).is_bare());
}

#[test]
fn the_manifest_lists_the_networked_plugins_and_the_names_registered() {
    let engine = EngineBuilder::new()
        .add_plugins(plugin::<Rope>(Some(2), |e| {
            e.net().ghost_class("Rope").datablock_class("RopeData");
        }))
        .add_plugins(plugin::<Arena>(Some(1), |e| {
            e.net()
                .ghost_class("Arena")
                .ghost_class("Rope")
                .channel::<Nothing>("terrain", 100);
        }))
        .add_plugins(plugin::<Local>(None, nothing))
        .build()
        .unwrap();
    let manifest = engine.net_manifest(&HandshakeProfile::default());
    assert_eq!((manifest.protocol_min, manifest.protocol_current), (12, 12));
    let plugins: Vec<_> = manifest
        .plugins
        .iter()
        .map(|p| (p.id.as_str(), p.version))
        .collect();
    assert_eq!(plugins, [("example.arena", 1), ("torque.rope", 2)]);
    assert_eq!(manifest.ghosts.names().unwrap(), ["Arena", "Rope"]);
    assert_eq!(manifest.datablocks.names().unwrap(), ["RopeData"]);
    assert_eq!(manifest.channels.names().unwrap(), ["terrain"]);
    assert!(manifest.mods.is_empty());
}

#[test]
fn a_server_and_a_client_of_the_same_engine_agree() {
    let build = || {
        EngineBuilder::new()
            .add_plugins(plugin::<Rope>(Some(2), |e| {
                e.net().ghost_class("Rope");
            }))
            .build()
            .unwrap()
    };
    let profile = HandshakeProfile::default();
    let gate = build().connect_gate(&profile).expect("a gate");
    let block = build().connect_block(&cx()).unwrap();
    assert_eq!(gate.check(from(), &cx(), &fields(block)), Ok(None));
}

#[test]
fn a_server_refuses_a_client_built_with_another_plugin_version_or_none() {
    let server = EngineBuilder::new()
        .add_plugins(plugin::<Rope>(Some(2), nothing))
        .build()
        .unwrap();
    let gate = server.connect_gate(&HandshakeProfile::default()).unwrap();
    let older = EngineBuilder::new()
        .add_plugins(plugin::<Rope>(Some(1), nothing))
        .build()
        .unwrap();
    assert_eq!(
        gate.check(from(), &cx(), &fields(older.connect_block(&cx()).unwrap())),
        Err(b"server needs plugin torque.rope 2, you have 1".to_vec())
    );
    let plain = EngineBuilder::new().build().unwrap();
    assert_eq!(plain.connect_block(&cx()), Ok(None));
    assert_eq!(
        gate.check(from(), &cx(), &fields(None)),
        Err(b"server needs plugin torque.rope 2, you do not have it".to_vec())
    );
}

#[test]
fn the_auth_provider_gives_tickets_and_verifies_them() {
    let engine = EngineBuilder::new()
        .add_plugins(plugin::<Auth>(None, |e| {
            e.net().auth(SharedSecret::new([5; 16], "ada"));
        }))
        .build()
        .unwrap();
    assert!(
        engine.is_networked(),
        "an auth provider alone says something"
    );
    let block = engine.connect_block(&cx()).unwrap().expect("a block");
    assert!(block.ticket.is_some());
    let gate = engine.connect_gate(&HandshakeProfile::default()).unwrap();
    assert_eq!(
        gate.check(from(), &cx(), &fields(Some(block.clone()))),
        Ok(Some(b"ada".to_vec()))
    );
    // The same block on another server or another connection is refused.
    let refused = Err(b"your sign-in was not accepted".to_vec());
    let elsewhere = AuthContext {
        server: SocketAddr::from(([192, 0, 2, 2], 28_000)),
        ..cx()
    };
    assert_eq!(
        gate.check(from(), &elsewhere, &fields(Some(block.clone()))),
        refused
    );
    let later = AuthContext {
        challenge: [9, 8, 7, 5],
        ..cx()
    };
    assert_eq!(gate.check(from(), &later, &fields(Some(block))), refused);
    // A ticket signed with another secret.
    let forged = ConnectBlock {
        manifest: Some(engine.net_manifest(&HandshakeProfile::default())),
        ticket: Some(SharedSecret::new([6; 16], "ada").ticket(&cx()).unwrap()),
        ..Default::default()
    };
    assert_eq!(gate.check(from(), &cx(), &fields(Some(forged))), refused);
}

#[test]
fn an_auth_provider_that_has_no_ticket_stops_the_connect_with_its_reason() {
    let engine = EngineBuilder::new()
        .add_plugins(plugin::<Auth>(None, |e| {
            e.net().auth(SharedSecret::new([5; 16], ""));
        }))
        .build()
        .unwrap();
    assert_eq!(
        engine.connect_block(&cx()),
        Err("not signed in".to_string())
    );
}

#[test]
fn odd_or_large_name_tables_still_make_a_block() {
    let engine = EngineBuilder::new()
        .add_plugins(plugin::<Rope>(Some(1), |e| {
            e.net().ghost_class("Ro\u{202e}pe");
        }))
        .build()
        .unwrap();
    // A name with invisible characters is counted in its table's digest but never listed, so the block still writes.
    assert!(engine.connect_block(&cx()).unwrap().is_some());
    let engine = EngineBuilder::new()
        .add_plugins(plugin::<Rope>(Some(1), |e| {
            for i in 0..400 {
                e.net()
                    .channel::<Nothing>(format!("channel-with-a-rather-long-name-{i:04}"), 100);
            }
        }))
        .build()
        .unwrap();
    assert!(
        engine.connect_block(&cx()).unwrap().is_some(),
        "a big table goes as a count and digest"
    );
}

#[test]
fn a_second_auth_provider_fails_the_build() {
    let error = EngineBuilder::new()
        .add_plugins(plugin::<AuthOne>(None, |e| {
            e.net().auth(SharedSecret::new([1; 16], "a"));
        }))
        .add_plugins(plugin::<AuthTwo>(None, |e| {
            e.net().auth(SharedSecret::new([2; 16], "b"));
        }))
        .build()
        .unwrap_err();
    assert!(matches!(error, BuildError::DuplicateSlot { .. }), "{error}");
}

#[test]
fn an_engine_speaks_torques_protocol_unless_a_plugin_says_otherwise() {
    assert_eq!(
        EngineBuilder::new().build().unwrap().protocol(),
        ProtocolKind::Tge
    );
    let engine = EngineBuilder::new()
        .add_plugins(plugin::<ProtocolOne>(None, |e| {
            e.net().protocol(ProtocolKind::Engine);
        }))
        .build()
        .unwrap();
    assert_eq!(engine.protocol(), ProtocolKind::Engine);
    // Setting the protocol says nothing on the wire by itself.
    assert!(!engine.is_networked());
}

#[test]
fn a_second_protocol_fails_the_build_and_names_both_plugins() {
    let error = EngineBuilder::new()
        .add_plugins(plugin::<ProtocolOne>(None, |e| {
            e.net().protocol(ProtocolKind::Tge);
        }))
        .add_plugins(plugin::<ProtocolTwo>(None, |e| {
            e.net().protocol(ProtocolKind::Engine);
        }))
        .build()
        .unwrap_err();
    assert!(matches!(error, BuildError::DuplicateSlot { .. }), "{error}");
    let text = error.to_string();
    assert!(
        text.contains("protocol.one") && text.contains("protocol.two"),
        "{text}"
    );
}

#[test]
fn the_protocols_have_stable_labels() {
    for kind in [ProtocolKind::Tge, ProtocolKind::Engine] {
        assert_eq!(ProtocolKind::from_label(kind.label()), Some(kind));
    }
    assert_eq!(ProtocolKind::Tge.label(), "tge");
    assert_eq!(ProtocolKind::Engine.label(), "engine");
    assert_eq!(ProtocolKind::from_label("udp"), None);
    assert_eq!(ProtocolKind::from_label("Tge"), None);
    assert_eq!(ProtocolKind::default(), ProtocolKind::Tge);
}
