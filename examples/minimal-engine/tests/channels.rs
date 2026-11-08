//! A bulk channel through the engine's own server and client over the memory transport: the channel is turned on by the
//! handshake (the client's manifest lists it, the server's gate matches it, the accept says so), the data a plugin's channel
//! syncs arrives while the gem's ghost updates keep coming, and a second connection gets it again.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::rc::Rc;

use torque::app::client::ClientNet;
use torque::app::server::Listen;
use torque::engine::{Plugin, PluginId};
use torque::net::chunk::{ChannelLimits, Kind};
use torque::server::net::channels::{ChannelHub, ChunkChannel, ClientChannels};
use torque_example_minimal_engine::{builder, client, files, ghosts, server};

const SERVER: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_000);
const CLIENT: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_001);

/// What both sides of the channel share in this one process: the server's data, and what the client got.
#[derive(Clone, Default)]
struct Terrain {
    server: Rc<RefCell<Vec<u8>>>,
    client: Rc<RefCell<Vec<u8>>>,
    messages: Rc<RefCell<usize>>,
}

impl ChunkChannel for Terrain {
    fn snapshot(&mut self, _client: torque::net::handshake::ConnId) -> Option<Vec<u8>> {
        Some(self.server.borrow().clone())
    }

    fn receive(&mut self, kind: Kind, data: &[u8]) {
        assert_eq!(kind, Kind::Snapshot);
        *self.messages.borrow_mut() += 1;
        *self.client.borrow_mut() = data.to_vec();
    }
}

#[derive(PluginId)]
#[plugin(crate = torque::engine)]
struct TerrainPlugin(Terrain);

impl Plugin for TerrainPlugin {
    fn build(&self, engine: &mut torque::engine::EngineBuilder) {
        let terrain = self.0.clone();
        engine.net().channel_with(
            "terrain",
            ChannelLimits::new(100).with_max_message(1 << 20),
            move || terrain.clone(),
        );
    }
}

fn gem_positions(client: &torque::app::client::GameClient, into: &mut BTreeSet<[i32; 3]>) {
    if let Some((_, at)) = ghosts(client).into_iter().find(|(c, _)| *c == "Item") {
        into.insert(at.map(|f| (f * 1000.0) as i32));
    }
}

#[test]
fn the_handshake_turns_a_plugin_channel_on_and_each_connection_gets_the_data() {
    let terrain = Terrain::default();
    let data: Vec<u8> = (0..30_000u32).map(|i| (i * 31 % 251) as u8).collect();
    *terrain.server.borrow_mut() = data.clone();

    let files = files().unwrap();
    let mut b = builder(&files).unwrap();
    b.add_plugins(TerrainPlugin(terrain.clone()));
    let engine = b.build().unwrap();
    assert_eq!(engine.net_channels().unwrap().names(), ["terrain"]);
    let server = server(&engine, Listen::Memory(SERVER)).expect("the server starts");
    let hub = server
        .vm()
        .extension::<ChannelHub>()
        .expect("the engine has channels");
    let mut client = client(
        &engine,
        ClientNet::Memory {
            client: CLIENT,
            server: SERVER,
        },
    )
    .expect("the client");
    let channels = client
        .vm()
        .extension::<ClientChannels>()
        .expect("the client has channels");
    client.start_hosting(server);
    let address = SERVER.to_string();
    client.call("connectTo", &[&address]).unwrap();
    assert!(!channels.is_active(), "off until the server's accept");

    // Nothing is turned on by hand: the accept does it, on both sides.
    let mut steps = 0;
    let mut gem_at = BTreeSet::new();
    while *terrain.client.borrow() != data && steps < 5_000 {
        client.step(32);
        steps += 1;
        gem_positions(&client, &mut gem_at);
    }
    assert!(
        *terrain.client.borrow() == data,
        "the data arrived: {} of {} bytes, log {:?}",
        terrain.client.borrow().len(),
        data.len(),
        client.client_log
    );
    assert!(channels.is_active());
    assert_eq!(hub.clients().len(), 1);
    assert_eq!(channels.held_bytes(), 0);
    // The gem's ghost kept updating while it came.
    for _ in 0..300 {
        client.step(32);
        gem_positions(&client, &mut gem_at);
    }
    assert!(gem_at.len() > 50, "{} gem positions", gem_at.len());
    assert_eq!(client.ghost_count(), 3);
    assert_eq!(*terrain.messages.borrow(), 1);
    eprintln!("30 KB in {steps} steps");

    // The same client connects again: its channels start over and it gets the data again.
    client.call("disconnect", &[]).unwrap();
    for _ in 0..200 {
        client.step(32);
    }
    assert!(
        hub.clients().is_empty(),
        "the server dropped the connection"
    );
    terrain.client.borrow_mut().clear();
    client.call("connectTo", &[&address]).unwrap();
    assert!(!channels.is_active(), "a new connection starts over");
    let mut steps = 0;
    while *terrain.client.borrow() != data && steps < 5_000 {
        client.step(32);
        steps += 1;
    }
    assert!(
        *terrain.client.borrow() == data,
        "the second connection's data arrived: {} of {} bytes, log {:?}",
        terrain.client.borrow().len(),
        data.len(),
        client.client_log
    );
    assert_eq!(*terrain.messages.borrow(), 2);
    assert!(channels.is_active());
    assert_eq!(client.ghost_count(), 3);
}

#[test]
fn a_server_without_channels_never_turns_a_clients_channels_on() {
    let terrain = Terrain::default();
    let files = files().unwrap();
    // The server's engine has no channel and no gate; the client's lists one in its manifest.
    let plain = builder(&files).unwrap().build().unwrap();
    let mut b = builder(&files).unwrap();
    b.add_plugins(TerrainPlugin(terrain.clone()));
    let with = b.build().unwrap();
    let server = server(&plain, Listen::Memory(SERVER)).expect("the server starts");
    assert!(server.vm().extension::<ChannelHub>().is_none());
    let mut client = client(
        &with,
        ClientNet::Memory {
            client: CLIENT,
            server: SERVER,
        },
    )
    .expect("the client");
    let channels = client.vm().extension::<ClientChannels>().unwrap();
    client.start_hosting(server);
    client.call("connectTo", &[&SERVER.to_string()]).unwrap();
    for _ in 0..600 {
        client.step(32);
        if client.ghost_count() == 3 {
            break;
        }
    }
    // The connection works as a Torque one does, and nothing is read as channels.
    assert_eq!(client.ghost_count(), 3, "{:?}", client.client_log);
    assert!(!channels.is_active());
    assert_eq!(*terrain.messages.borrow(), 0);
}

/// What the server's logic of a client-to-server channel got.
#[derive(Clone, Default)]
struct Uploads {
    got: Rc<RefCell<Vec<Vec<u8>>>>,
}

impl ChunkChannel for Uploads {
    fn receive_from(&mut self, _from: torque::net::handshake::ConnId, _kind: Kind, data: &[u8]) {
        self.got.borrow_mut().push(data.to_vec());
    }
}

#[derive(PluginId)]
#[plugin(crate = torque::engine)]
struct UploadPlugin(Uploads);

impl Plugin for UploadPlugin {
    fn build(&self, engine: &mut torque::engine::EngineBuilder) {
        let uploads = self.0.clone();
        engine.net().channel_to_server_with(
            "uploads",
            ChannelLimits::new(200).with_max_message(256 << 10),
            move || uploads.clone(),
        );
    }
}

#[test]
fn a_plugin_receives_what_a_client_sends_on_its_client_to_server_channel() {
    let uploads = Uploads::default();
    let files = files().unwrap();
    let mut b = builder(&files).unwrap();
    b.add_plugins(UploadPlugin(uploads.clone()));
    let engine = b.build().unwrap();
    let registry = engine.net_channels().unwrap();
    assert!(registry.names().is_empty());
    assert_eq!(registry.up_names(), ["uploads"]);
    let server = server(&engine, Listen::Memory(SERVER)).expect("the server starts");
    let mut client = client(
        &engine,
        ClientNet::Memory {
            client: CLIENT,
            server: SERVER,
        },
    )
    .expect("the client");
    let channels = client
        .vm()
        .extension::<ClientChannels>()
        .expect("the client has channels");
    client.start_hosting(server);
    client.call("connectTo", &[&SERVER.to_string()]).unwrap();
    // Nothing can be sent before the accept turns the channels on.
    assert!(channels.send("uploads", Kind::Delta, vec![1]).is_err());
    let mut steps = 0;
    while !channels.is_active() && steps < 600 {
        client.step(32);
        steps += 1;
    }
    assert!(channels.is_active(), "{:?}", client.client_log);
    let first: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let second = vec![42u8; 300];
    channels
        .send("uploads", Kind::Delta, first.clone())
        .unwrap();
    channels
        .send("uploads", Kind::Delta, second.clone())
        .unwrap();
    assert!(channels.send("nope", Kind::Delta, vec![1]).is_err());
    assert!(
        channels
            .send("uploads", Kind::Delta, vec![0; (256 << 10) + 1])
            .is_err(),
        "a message over the channel's cap is refused locally"
    );
    let mut steps = 0;
    while uploads.got.borrow().len() < 2 && steps < 5_000 {
        client.step(32);
        steps += 1;
    }
    assert_eq!(
        uploads.got.borrow().as_slice(),
        &[first, second],
        "whole and in order after {steps} steps: {:?}",
        client.client_log
    );
    // The moves and ghosts went on beside it.
    for _ in 0..300 {
        client.step(32);
    }
    assert_eq!(client.ghost_count(), 3);
    assert!(channels.upstream_idle());
}
