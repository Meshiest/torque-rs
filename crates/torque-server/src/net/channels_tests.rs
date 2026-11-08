use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use torque_engine::{EngineBuilder, Plugin, PluginId};
use torque_net::bits::BitWriter;
use torque_net::chunk::ChannelLimits;

struct Nothing;

impl ChunkChannel for Nothing {}

impl Default for Nothing {
    fn default() -> Nothing {
        Nothing
    }
}

#[derive(PluginId)]
#[plugin(id = "test.channels")]
struct Channels(&'static [(&'static str, usize)]);

impl Plugin for Channels {
    fn build(&self, engine: &mut EngineBuilder) {
        let mut net = engine.net();
        for (name, budget) in self.0 {
            net.channel::<Nothing>(*name, *budget);
        }
    }
}

fn engine(channels: &'static [(&'static str, usize)]) -> torque_engine::Engine {
    EngineBuilder::new()
        .add_plugins(Channels(channels))
        .build()
        .expect("the engine builds")
}

#[test]
fn an_engines_channels_are_sorted_by_name_so_both_ends_agree_on_the_ids() {
    let registry = engine(&[("voxels", 200), ("terrain", 300)])
        .net_channels()
        .unwrap();
    assert_eq!(registry.names(), ["terrain", "voxels"]);
    assert_eq!(registry.id("voxels"), Some(1));
    assert_eq!(registry.limits()[0].budget, 300);
    // No channel: nothing registered, so no layer is installed and no packet changes.
    assert!(engine(&[]).net_channels().unwrap().is_empty());
}

#[test]
fn a_duplicate_name_or_a_bad_budget_fails_when_the_registry_is_made() {
    let twice = engine(&[("terrain", 200), ("terrain", 300)]);
    assert_eq!(
        twice.net_channels().unwrap_err(),
        ChannelError::Duplicate("terrain".into())
    );
    let big = engine(&[("terrain", 1_000_000)]);
    assert!(matches!(
        big.net_channels(),
        Err(ChannelError::Budget { .. })
    ));
}

#[test]
fn a_client_reads_nothing_until_it_is_active() {
    let registry = engine(&[("terrain", 200)]).net_channels().unwrap();
    let channels = ClientChannels::new(&registry);
    // A section that is not a section: ignored while off, an error once on.
    let bytes = [0xffu8; 8];
    assert!(channels.read(&mut BitReader::new(&bytes)).is_ok());
    channels.set_active(true);
    assert!(channels.read(&mut BitReader::new(&bytes)).is_err());
    assert!(channels.read(&mut BitReader::new(&[0u8])).is_ok());
}

#[test]
fn an_engine_peer_gets_channels_only_when_the_game_says_so() {
    let registry = engine(&[("terrain", 200)]).net_channels().unwrap();
    let hub = ChannelHub::new(registry);
    let mut layer = ChannelLayer::new(Box::new(Quiet), hub.clone());
    let (a, b) = (ConnId(1), ConnId(2));
    for conn in [a, b] {
        layer.on_accept(conn, 10 + conn.0, 14);
    }
    layer.mark_peer(a);
    assert!(
        !hub.is_enabled(a),
        "off until the game says its peers are its own"
    );
    hub.set_enable_peers(true);
    layer.mark_peer(b);
    assert!(hub.is_enabled(b) && !hub.is_enabled(a));
    // A connection that never connected cannot be enabled, and a dropped one is gone.
    hub.enable(ConnId(9));
    assert!(!hub.is_enabled(ConnId(9)));
    layer.on_drop(b, &mut Vm::new());
    assert!(!hub.is_enabled(b));
    assert_eq!(hub.connected(), vec![a]);
}

/// A channel whose `receive` calls back into the client's channels, as game code may.
struct Reentrant(
    Rc<RefCell<Option<Rc<ClientChannels>>>>,
    Rc<RefCell<Vec<String>>>,
);

impl ChunkChannel for Reentrant {
    fn receive(&mut self, kind: Kind, data: &[u8]) {
        let channels = self.0.borrow().clone().expect("the test sets it");
        self.1.borrow_mut().push(format!(
            "{kind:?} {} active {} held {}",
            data.len(),
            channels.is_active(),
            channels.held_bytes()
        ));
        channels.set_active(true);
    }
}

fn one_message(registry: &ChannelRegistry, data: &[u8]) -> Vec<u8> {
    let mut sender = ChunkSender::new(&registry.limits());
    sender.enqueue(0, Kind::Snapshot, data.to_vec()).unwrap();
    let mut w = BitWriter::new();
    sender.write(&mut w, 1, 8_000);
    w.into_bytes()
}

#[test]
fn a_channels_receive_may_call_back_into_the_clients_channels() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let slot = Rc::new(RefCell::new(None));
    let registry =
        ChannelRegistry::new(&[ChannelEntry::new("terrain", ChannelLimits::new(200), {
            let (slot, seen) = (Rc::clone(&slot), Rc::clone(&seen));
            move || Reentrant(Rc::clone(&slot), Rc::clone(&seen))
        })])
        .unwrap();
    let channels = Rc::new(ClientChannels::new(&registry));
    *slot.borrow_mut() = Some(Rc::clone(&channels));
    channels.set_active(true);
    let bytes = one_message(&registry, &[7; 100]);
    channels.read(&mut BitReader::new(&bytes)).unwrap();
    assert_eq!(*seen.borrow(), ["Snapshot 100 active true held 0"]);
}

#[test]
fn a_new_connection_starts_the_channels_over() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let slot = Rc::new(RefCell::new(None));
    let registry =
        ChannelRegistry::new(&[ChannelEntry::new("terrain", ChannelLimits::new(200), {
            let (slot, seen) = (Rc::clone(&slot), Rc::clone(&seen));
            move || Reentrant(Rc::clone(&slot), Rc::clone(&seen))
        })])
        .unwrap();
    let channels = Rc::new(ClientChannels::new(&registry));
    *slot.borrow_mut() = Some(Rc::clone(&channels));
    channels.set_active(true);
    // The first connection's first message is number 0, and a second one's section starts at 0 again: after a reset it is a new
    // message, not one already delivered. A half-received message of the first connection does not leak into the second.
    let first = one_message(&registry, &[1; 50]);
    channels.read(&mut BitReader::new(&first)).unwrap();
    assert_eq!(seen.borrow().len(), 1);
    channels.read(&mut BitReader::new(&first)).unwrap();
    assert_eq!(
        seen.borrow().len(),
        1,
        "the same message again is a duplicate"
    );
    channels.reset();
    assert!(!channels.is_active());
    channels.set_active(true);
    channels.read(&mut BitReader::new(&first)).unwrap();
    assert_eq!(
        seen.borrow().len(),
        2,
        "after a reset it is a new connection's first message"
    );
}
