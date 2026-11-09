//! Uploads through the engine's own server and client over the memory transport, in both protocols: a client offers a file, the server
//! pulls it block by block through the RPCs and the upload channel, and a plugin's sink gets it. Refusals reach the client with their
//! reasons, and a client that sends what the server did not ask for loses its connection.

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use torque::app::client::{ClientNet, GameClient};
use torque::app::plugins::set_protocol;
use torque::app::server::Listen;
use torque::engine::permissions::SubjectKey;
use torque::engine::upload::{
    CHANNEL, Quota, StagingConfig, Status, UntrustedBytes, UploadCx, UploadSink, UploadSpec,
    uploads,
};
use torque::engine::{EngineBuilder, Plugin, PluginId, ProtocolKind};
use torque::net::chunk::Kind;
use torque::server::net::channels::{ChannelHub, ClientChannels};
use torque_example_minimal_engine::{builder, client, files, server};

fn addresses(slot: u16) -> (SocketAddr, SocketAddrV4) {
    (
        SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 28_300 + 2 * slot),
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, 28_301 + 2 * slot),
    )
}

/// What the sink was given: the description, the content and the player.
type Finished = (Vec<u8>, Vec<u8>, SubjectKey);

#[derive(Default)]
struct Seen {
    finished: RefCell<Vec<Finished>>,
}

struct Sink(Rc<Seen>);

impl UploadSink for Sink {
    fn finish(
        &mut self,
        cx: &mut UploadCx<'_>,
        meta: &[u8],
        bytes: UntrustedBytes<'_>,
    ) -> Result<(), String> {
        let content = bytes.to_vec(usize::MAX).map_err(|e| e.to_string())?;
        self.0
            .finished
            .borrow_mut()
            .push((meta.to_vec(), content, cx.subject.clone()));
        Ok(())
    }
}

#[derive(PluginId)]
#[plugin(crate = torque::engine)]
struct UploadPlugin {
    seen: Rc<Seen>,
    staging: Option<PathBuf>,
    spec: UploadSpec,
}

impl Plugin for UploadPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine
            .net()
            .upload("files", self.spec.clone(), Sink(Rc::clone(&self.seen)));
        if let Some(folder) = &self.staging {
            engine.net().upload_staging(StagingConfig {
                memory_per_upload: 32 << 10,
                memory_total: 64 << 10,
                folder: Some(folder.clone()),
                ..StagingConfig::default()
            });
        }
    }
}

struct Rig {
    client: GameClient,
    hub: Rc<ChannelHub>,
    channels: Rc<ClientChannels>,
    seen: Rc<Seen>,
}

/// The limits the tests use unless they say otherwise: roomy, so only the link is slow.
fn roomy() -> UploadSpec {
    UploadSpec {
        max_bytes: 2 << 20,
        quota: Quota {
            bytes: 64 << 20,
            per: Duration::from_secs(3_600),
            files: 16,
        },
        rate: 4 << 20,
        server_rate: 16 << 20,
        concurrent: 2,
        total: Duration::from_secs(3_600),
        idle: Duration::from_secs(60),
        ..UploadSpec::default()
    }
}

fn rig(protocol: ProtocolKind, slot: u16, staging: Option<PathBuf>) -> Rig {
    rig_with(protocol, slot, staging, roomy())
}

fn rig_with(protocol: ProtocolKind, slot: u16, staging: Option<PathBuf>, spec: UploadSpec) -> Rig {
    let (server_addr, client_addr) = addresses(slot);
    let seen = Rc::new(Seen::default());
    let files = files().unwrap();
    let mut b = builder(&files).unwrap();
    set_protocol(&mut b, protocol);
    b.add_plugins(UploadPlugin {
        seen: Rc::clone(&seen),
        staging,
        spec,
    });
    let engine = b.build().unwrap();
    let server = server(&engine, Listen::Memory(server_addr)).expect("the server starts");
    let hub = server
        .vm()
        .extension::<ChannelHub>()
        .expect("the server has channels");
    let mut client = client(
        &engine,
        ClientNet::Memory {
            client: client_addr,
            server: server_addr,
        },
    )
    .expect("the client");
    let channels = client
        .vm()
        .extension::<ClientChannels>()
        .expect("the client has channels");
    client.start_hosting(server);
    client
        .call("connectTo", &[&server_addr.to_string()])
        .unwrap();
    Rig {
        client,
        hub,
        channels,
        seen,
    }
}

impl Rig {
    /// Steps until the connection takes uploads.
    fn connect(&mut self) {
        for _ in 0..1_000 {
            if self.channels.is_active() && self.begin_ready() {
                return;
            }
            self.client.step(32);
        }
        panic!("never connected: {:?}", self.client.client_log);
    }

    fn begin_ready(&self) -> bool {
        uploads(self.client.vm()).is_some_and(|u| u.ready())
    }

    fn until(&mut self, max: u32, what: &str, done: impl Fn(&Rig) -> bool) -> u32 {
        for step in 0..max {
            if done(self) {
                return step;
            }
            self.client.step(32);
        }
        assert!(done(self), "{what}: {:?}", self.client.client_log);
        max
    }
}

fn content(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(seed).wrapping_add((i >> 8) as u8))
        .collect()
}

fn an_upload_arrives(protocol: ProtocolKind, slot: u16) {
    let mut rig = rig(protocol, slot, None);
    rig.connect();
    let bytes = content(120_000, 7);
    let handle = uploads(rig.client.vm())
        .unwrap()
        .begin("files", b"notes".to_vec(), Box::new(bytes.clone()))
        .unwrap();
    let steps = rig.until(20_000, "the upload ends", |r| {
        !matches!(
            uploads(r.client.vm()).unwrap().status(handle),
            Some(Status::Waiting | Status::Sending { .. })
        )
    });
    assert_eq!(
        uploads(rig.client.vm()).unwrap().status(handle),
        Some(Status::Done),
        "{:?}",
        rig.client.client_log
    );
    eprintln!("{protocol:?}: 120 KB in {steps} steps");
    let finished = rig.seen.finished.borrow();
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].0, b"notes");
    assert_eq!(finished[0].1, bytes);
    assert!(matches!(finished[0].2, SubjectKey::Connection(_)));
    assert_eq!(rig.hub.clients().len(), 1, "the connection stayed");
}

#[test]
fn an_upload_arrives_in_the_tge_protocol() {
    an_upload_arrives(ProtocolKind::Tge, 0);
}

#[test]
fn an_upload_arrives_in_the_engine_protocol() {
    an_upload_arrives(ProtocolKind::Engine, 1);
}

fn a_refusal_reaches_the_client(protocol: ProtocolKind, slot: u16) {
    let mut rig = rig(protocol, slot, None);
    rig.connect();
    let handle = uploads(rig.client.vm())
        .unwrap()
        .begin("files", b"x".to_vec(), Box::new(content((2 << 20) + 1, 3)))
        .unwrap();
    rig.until(2_000, "the refusal", |r| {
        matches!(
            uploads(r.client.vm()).unwrap().status(handle),
            Some(Status::Failed(_))
        )
    });
    assert_eq!(
        uploads(rig.client.vm()).unwrap().status(handle),
        Some(Status::Failed(
            "That is 2.0 MiB and this kind of upload takes at most 2.0 MiB.".to_string()
        ))
    );
    let handle = uploads(rig.client.vm())
        .unwrap()
        .begin("nothing", b"x".to_vec(), Box::new(content(100, 3)))
        .unwrap();
    rig.until(2_000, "the refusal of a kind", |r| {
        matches!(
            uploads(r.client.vm()).unwrap().status(handle),
            Some(Status::Failed(_))
        )
    });
    assert!(rig.seen.finished.borrow().is_empty());
}

#[test]
fn a_refusal_reaches_the_client_in_the_tge_protocol() {
    a_refusal_reaches_the_client(ProtocolKind::Tge, 2);
}

#[test]
fn a_refusal_reaches_the_client_in_the_engine_protocol() {
    a_refusal_reaches_the_client(ProtocolKind::Engine, 3);
}

fn unrequested_data_drops_the_client(protocol: ProtocolKind, slot: u16) {
    let mut rig = rig(protocol, slot, None);
    rig.connect();
    assert_eq!(rig.hub.clients().len(), 1);
    // Data for an upload that does not exist: an honest client sends only what it was asked for.
    // A data message (tag 2) for upload 99, from block 0.
    let message = vec![2, 99, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3];
    rig.channels.send(CHANNEL, Kind::Snapshot, message).unwrap();
    rig.until(2_000, "the drop", |r| r.hub.clients().is_empty());
    assert!(rig.seen.finished.borrow().is_empty());
    assert_eq!(
        rig.client.global("$Server::UploadViolations"),
        "",
        "that global is the server's"
    );
}

#[test]
fn unrequested_data_drops_the_client_in_the_tge_protocol() {
    unrequested_data_drops_the_client(ProtocolKind::Tge, 4);
}

#[test]
fn unrequested_data_drops_the_client_in_the_engine_protocol() {
    unrequested_data_drops_the_client(ProtocolKind::Engine, 5);
}

fn a_large_upload_is_staged_on_disk(protocol: ProtocolKind, slot: u16) {
    let folder = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("uploads-{slot}"));
    let _ = std::fs::remove_dir_all(&folder);
    let mut rig = rig(protocol, slot, Some(folder.clone()));
    rig.connect();
    let bytes = content(120_000, 5);
    let handle = uploads(rig.client.vm())
        .unwrap()
        .begin("files", b"big".to_vec(), Box::new(bytes.clone()))
        .unwrap();
    // While it comes, the staging folder holds one file and nothing a client chose.
    let mut saw_file = false;
    for _ in 0..40_000 {
        if uploads(rig.client.vm()).unwrap().status(handle) == Some(Status::Done) {
            break;
        }
        saw_file |= std::fs::read_dir(&folder).is_ok_and(|d| d.count() == 1);
        rig.client.step(32);
    }
    assert_eq!(
        uploads(rig.client.vm()).unwrap().status(handle),
        Some(Status::Done),
        "{:?}",
        rig.client.client_log
    );
    assert!(saw_file, "the upload was staged in the folder");
    assert_eq!(rig.seen.finished.borrow()[0].1, bytes);
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_large_upload_is_staged_on_disk_in_the_engine_protocol() {
    a_large_upload_is_staged_on_disk(ProtocolKind::Engine, 6);
}

#[test]
fn a_large_upload_is_staged_on_disk_in_the_tge_protocol() {
    a_large_upload_is_staged_on_disk(ProtocolKind::Tge, 7);
}

fn a_1_mib_upload_completes_at_default_prefs_and_default_limits(protocol: ProtocolKind, slot: u16) {
    // The limits are the defaults (30 s quiet time, 15 minutes in all, 128 KiB a second) and so are the client's packet prefs:
    // Torque's protocol then carries a few KiB a second, and a run of 256 KiB takes longer than the quiet time. The upload goes on
    // because the connection keeps delivering.
    let mut rig = rig_with(protocol, slot, None, UploadSpec::default());
    rig.connect();
    let bytes = content(1 << 20, 11);
    let handle = uploads(rig.client.vm())
        .unwrap()
        .begin("files", b"big".to_vec(), Box::new(bytes.clone()))
        .unwrap();
    let steps = rig.until(60_000, "the upload ends", |r| {
        !matches!(
            uploads(r.client.vm()).unwrap().status(handle),
            Some(Status::Waiting | Status::Sending { .. })
        )
    });
    assert_eq!(
        uploads(rig.client.vm()).unwrap().status(handle),
        Some(Status::Done),
        "{:?}",
        rig.client.client_log
    );
    eprintln!(
        "{protocol:?}: 1 MiB in {steps} steps ({} s)",
        steps * 32 / 1_000
    );
    assert_eq!(rig.seen.finished.borrow()[0].1, bytes);
    assert_eq!(rig.hub.clients().len(), 1, "the connection stayed");
}

#[test]
fn a_1_mib_upload_completes_at_defaults_in_the_tge_protocol() {
    a_1_mib_upload_completes_at_default_prefs_and_default_limits(ProtocolKind::Tge, 8);
}

#[test]
fn a_1_mib_upload_completes_at_defaults_in_the_engine_protocol() {
    a_1_mib_upload_completes_at_default_prefs_and_default_limits(ProtocolKind::Engine, 9);
}
