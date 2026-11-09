use std::cell::{Cell, RefCell};
use std::rc::Rc;

use torque_ghosts::codec::NetWidths;
use torque_transfer::{BLOCK_BYTES, Quota, encode_data};
use torque_ts_vm::vm::Vm;

use torque_net::chunk::Kind;
use torque_net::chunk::registry::ChunkChannel;
use torque_transfer::MAX_META_BYTES;

use super::server::MAX_REASON;
use super::*;
use crate::permissions::NoPermissions;
use crate::rpc::NoGhosts;
use crate::rpc::RpcCx;
use crate::{BuildError, EngineBuilder, Plugin, PluginId};

const CONN: ConnId = ConnId(7);

/// What a sink was given: the description, the content and the player.
type Finished = (Vec<u8>, Vec<u8>, SubjectKey);

/// What the sinks saw.
#[derive(Default)]
struct Seen {
    accepted: RefCell<Vec<Vec<u8>>>,
    finished: RefCell<Vec<Finished>>,
}

struct Files {
    seen: Rc<Seen>,
    refuse_with: Option<&'static str>,
    fail_with: Option<&'static str>,
}

impl UploadSink for Files {
    fn accept(&mut self, _cx: &mut UploadCx<'_>, meta: &[u8]) -> Result<(), String> {
        self.seen.accepted.borrow_mut().push(meta.to_vec());
        self.refuse_with.map_or(Ok(()), |r| Err(r.to_string()))
    }

    fn finish(
        &mut self,
        cx: &mut UploadCx<'_>,
        meta: &[u8],
        bytes: UntrustedBytes<'_>,
    ) -> Result<(), String> {
        if let Some(reason) = self.fail_with {
            return Err(reason.to_string());
        }
        let content = bytes.to_vec(usize::MAX).map_err(|e| e.to_string())?;
        self.seen
            .finished
            .borrow_mut()
            .push((meta.to_vec(), content, cx.subject.clone()));
        Ok(())
    }
}

fn spec() -> UploadSpec {
    UploadSpec {
        max_bytes: 1 << 20,
        quota: Quota {
            bytes: 8 << 20,
            per: std::time::Duration::from_secs(3_600),
            files: 4,
        },
        rate: 8 << 20,
        server_rate: 8 << 20,
        concurrent: 2,
        server_in_flight: 4 << 20,
        meta_max: 16,
        ..UploadSpec::default()
    }
}

#[derive(PluginId)]
#[plugin(id = "example.files")]
struct FilesPlugin {
    seen: Rc<Seen>,
    permission: Option<&'static str>,
    refuse_with: Option<&'static str>,
    fail_with: Option<&'static str>,
    idle_secs: Option<u64>,
}

impl FilesPlugin {
    fn new(seen: &Rc<Seen>) -> FilesPlugin {
        FilesPlugin {
            seen: Rc::clone(seen),
            permission: None,
            refuse_with: None,
            fail_with: None,
            idle_secs: None,
        }
    }
}

impl Plugin for FilesPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        let spec = UploadSpec {
            permission: self.permission.map(str::to_string),
            idle: self
                .idle_secs
                .map_or(spec().idle, std::time::Duration::from_secs),
            ..spec()
        };
        engine.net().upload(
            "files",
            spec,
            Files {
                seen: Rc::clone(&self.seen),
                refuse_with: self.refuse_with,
                fail_with: self.fail_with,
            },
        );
    }
}

fn engine(plugin: FilesPlugin) -> Engine {
    EngineBuilder::new().add_plugins(plugin).build().unwrap()
}

// ---- registration

#[test]
fn the_first_kind_registers_the_surface_and_the_next_kinds_share_it() {
    let seen = Rc::new(Seen::default());
    #[derive(PluginId)]
    #[plugin(id = "example.two")]
    struct Two(Rc<Seen>);
    impl Plugin for Two {
        fn build(&self, engine: &mut EngineBuilder) {
            for name in ["a", "b"] {
                let sink = Files {
                    seen: Rc::clone(&self.0),
                    refuse_with: None,
                    fail_with: None,
                };
                engine.net().upload(name, spec(), sink);
            }
        }
    }
    let engine = EngineBuilder::new().add_plugins(Two(seen)).build().unwrap();
    let rpcs = engine.net_rpcs().unwrap();
    let mut names: Vec<&str> = rpcs.iter().map(|e| e.name.as_str()).collect();
    names.sort_unstable();
    let mut want = [BEGIN_RPC, CANCEL_RPC, REQUEST_RPC, END_RPC];
    want.sort_unstable();
    assert_eq!(names, want);
    assert!(engine.net_channels().unwrap().up_names().contains(&CHANNEL));
    assert_eq!(engine.list::<UploadEntry>().len(), 2);
}

#[test]
fn registration_errors_fail_the_build() {
    let seen = Rc::new(Seen::default());
    let bad = |edit: fn(&mut UploadSpec), name: &str, twice: bool| {
        #[derive(PluginId)]
        #[plugin(id = "example.bad")]
        struct Bad(Rc<Seen>, fn(&mut UploadSpec), String, bool);
        impl Plugin for Bad {
            fn build(&self, engine: &mut EngineBuilder) {
                let mut spec = spec();
                (self.1)(&mut spec);
                let sink = || Files {
                    seen: Rc::clone(&self.0),
                    refuse_with: None,
                    fail_with: None,
                };
                engine.net().upload(&self.2, spec.clone(), sink());
                if self.3 {
                    engine.net().upload(&self.2, spec, sink());
                }
            }
        }
        match EngineBuilder::new()
            .add_plugins(Bad(Rc::clone(&seen), edit, name.to_string(), twice))
            .build()
        {
            Err(BuildError::Rejected { message, .. }) => message,
            other => panic!("expected a rejection, got {:?}", other.map(|_| ())),
        }
    };
    assert!(bad(|_| {}, "", false).contains("1 to 40 bytes"));
    assert!(bad(|_| {}, &"x".repeat(41), false).contains("1 to 40 bytes"));
    assert!(bad(|_| {}, "same", true).contains("another upload has this name"));
    assert!(bad(|s| s.max_bytes = 0, "files", false).contains("max_bytes"));
    assert!(
        bad(
            |s| s.permission = Some("not a permission".into()),
            "files",
            false
        )
        .contains("not one")
    );
}

#[test]
fn a_kinds_permission_is_declared_for_the_plugin() {
    let seen = Rc::new(Seen::default());
    let mut plugin = FilesPlugin::new(&seen);
    plugin.permission = Some("files.upload");
    let engine = engine(plugin);
    assert!(
        engine
            .declared_permissions()
            .iter()
            .any(|d| d.name == "files.upload")
    );
}

// ---- a server and a client in one test, joined by mock links

#[derive(Default)]
struct World {
    to_server: RefCell<UploadInbox>,
    begins: RefCell<Vec<(BeginUpload, BeginDone)>>,
    requests: RefCell<Vec<UploadRequest>>,
    ends: RefCell<Vec<UploadEnd>>,
    dropped: RefCell<Vec<ConnId>>,
    cancels: RefCell<Vec<u32>>,
    busy_requests: Cell<u32>,
    epoch: Cell<u64>,
    /// What the upload channel has delivered, as the server's host counts it.
    received: Cell<u64>,
    /// Whether the player holds the permission when the server looks at the finished upload.
    finish_granted: Cell<bool>,
    /// Requests the client has not got yet (a slow link).
    hold_requests: Cell<bool>,
}

struct ServerSide(Rc<World>);
struct ClientSide(Rc<World>);

struct Perms(bool);

impl PermissionView for Perms {
    fn has_permission(&self, _permission: &str) -> bool {
        self.0
    }
}

impl ServerLinks for ServerSide {
    fn take_inbox(&self) -> UploadInbox {
        std::mem::take(&mut *self.0.to_server.borrow_mut())
    }

    fn received(&self, _conn: ConnId) -> u64 {
        self.0.received.get()
    }

    fn request(&self, _conn: ConnId, request: UploadRequest) -> Result<(), LinkError> {
        if self.0.busy_requests.get() > 0 {
            self.0.busy_requests.set(self.0.busy_requests.get() - 1);
            return Err(LinkError::Busy);
        }
        self.0.requests.borrow_mut().push(request);
        Ok(())
    }

    fn end(&self, _conn: ConnId, end: UploadEnd) -> Result<(), LinkError> {
        self.0.ends.borrow_mut().push(end);
        Ok(())
    }

    fn permissions(&self, _conn: ConnId) -> Box<dyn PermissionView + '_> {
        Box::new(Perms(self.0.finish_granted.get()))
    }

    fn drop_conn(&self, _vm: &mut Vm, conn: ConnId, _reason: &[u8]) {
        self.0.dropped.borrow_mut().push(conn);
    }
}

impl ClientLinks for ClientSide {
    fn ready(&self) -> bool {
        true
    }

    fn epoch(&self) -> u64 {
        self.0.epoch.get()
    }

    fn begin(&self, request: BeginUpload, done: BeginDone) -> Result<(), RpcError> {
        self.0.begins.borrow_mut().push((request, done));
        Ok(())
    }

    fn cancel(&self, id: u32) {
        self.0.cancels.borrow_mut().push(id);
    }

    fn send(&self, message: Vec<u8>) -> Result<(), LinkError> {
        self.0.to_server.borrow_mut().messages.push((CONN, message));
        Ok(())
    }
}

struct Rig {
    engine: Engine,
    world: Rc<World>,
    server_vm: Vm,
    client_vm: Vm,
    tick: u64,
    granted: bool,
}

impl Rig {
    fn new(plugin: FilesPlugin) -> Rig {
        let engine = engine(plugin);
        let world = Rc::new(World::default());
        world.finish_granted.set(true);
        let mut server_vm = Vm::new();
        let mut client_vm = Vm::new();
        for &stage in ServerStage::ALL {
            engine.replay(stage, &mut server_vm).unwrap();
        }
        for &stage in ClientStage::ALL {
            engine.replay(stage, &mut client_vm).unwrap();
        }
        server_vm.set_extension(Rc::new(ServerLinksSlot(Rc::new(ServerSide(Rc::clone(
            &world,
        ))))));
        client_vm.set_extension(Rc::new(ClientLinksSlot(Rc::new(ClientSide(Rc::clone(
            &world,
        ))))));
        Rig {
            engine,
            world,
            server_vm,
            client_vm,
            tick: 0,
            granted: true,
        }
    }

    fn uploads(&self) -> Uploads {
        uploads(&self.client_vm).expect("the client has uploads")
    }

    /// One tick of both sides and what crosses between them.
    fn tick(&mut self) {
        self.tick += 1;
        let begins = std::mem::take(&mut *self.world.begins.borrow_mut());
        for (request, done) in begins {
            let server = self.server_vm.extension::<ServerUploads>().unwrap();
            let perms = Perms(self.granted);
            let reply = {
                let mut cx = RpcCx::new(
                    &mut self.server_vm,
                    Some(CONN),
                    &perms,
                    self.tick,
                    &NoGhosts,
                );
                server.begin(&mut cx, &request)
            };
            done(Ok(reply));
        }
        for hook in self.engine.list::<ClientTick>() {
            hook.run(&mut self.client_vm);
        }
        let info = TickInfo::new(self.tick);
        for hook in self.engine.list::<ServerTick>() {
            hook.run(&mut self.server_vm, &self.engine, info).unwrap();
        }
        let client = self.client_vm.extension::<ClientUploads>().unwrap();
        if !self.world.hold_requests.get() {
            for request in std::mem::take(&mut *self.world.requests.borrow_mut()) {
                client.on_request(&request);
            }
        }
        for end in std::mem::take(&mut *self.world.ends.borrow_mut()) {
            client.on_end(&end);
        }
    }

    fn run_until(&mut self, max: u32, done: impl Fn(&Rig) -> bool) -> bool {
        for _ in 0..max {
            if done(self) {
                return true;
            }
            self.tick();
        }
        done(self)
    }
}

fn content(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(seed).wrapping_add(seed))
        .collect()
}

#[test]
fn an_upload_goes_from_the_client_to_the_sink_and_both_are_told() {
    let seen = Rc::new(Seen::default());
    let mut rig = Rig::new(FilesPlugin::new(&seen));
    let bytes = content(300_000, 3);
    let handle = rig
        .uploads()
        .begin("files", b"notes".to_vec(), Box::new(bytes.clone()))
        .unwrap();
    assert_eq!(rig.uploads().status(handle), Some(Status::Waiting));
    assert!(rig.run_until(200, |r| r.uploads().status(handle) == Some(Status::Done)));
    let finished = seen.finished.borrow();
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0].0, b"notes");
    assert_eq!(finished[0].1, bytes);
    // The player is the connection: no identity was given.
    assert_eq!(finished[0].2, SubjectKey::Connection(CONN.0));
    assert_eq!(seen.accepted.borrow().as_slice(), [b"notes".to_vec()]);
    assert_eq!(
        rig.server_vm.global("$Server::UploadsFinished").as_deref(),
        Some(&b"1"[..])
    );
}

#[test]
fn the_sinks_reasons_reach_the_player() {
    let seen = Rc::new(Seen::default());
    let mut plugin = FilesPlugin::new(&seen);
    plugin.refuse_with = Some("That name is taken.");
    let mut rig = Rig::new(plugin);
    let handle = rig
        .uploads()
        .begin("files", b"x".to_vec(), Box::new(content(1000, 1)))
        .unwrap();
    rig.tick();
    assert_eq!(
        rig.uploads().status(handle),
        Some(Status::Failed("That name is taken.".to_string()))
    );
    let mut plugin = FilesPlugin::new(&seen);
    plugin.fail_with = Some("Not a build.");
    let mut rig = Rig::new(plugin);
    let handle = rig
        .uploads()
        .begin("files", b"x".to_vec(), Box::new(content(1000, 1)))
        .unwrap();
    assert!(rig.run_until(100, |r| matches!(
        r.uploads().status(handle),
        Some(Status::Failed(_))
    )));
    assert_eq!(
        rig.uploads().status(handle),
        Some(Status::Failed("Not a build.".to_string()))
    );
}

#[test]
fn a_kind_that_asks_for_a_permission_refuses_a_player_without_it() {
    let seen = Rc::new(Seen::default());
    let mut plugin = FilesPlugin::new(&seen);
    plugin.permission = Some("files.upload");
    let mut rig = Rig::new(plugin);
    rig.granted = false;
    let handle = rig
        .uploads()
        .begin("files", b"x".to_vec(), Box::new(content(1000, 1)))
        .unwrap();
    rig.tick();
    assert_eq!(
        rig.uploads().status(handle),
        Some(Status::Failed(
            "You are not allowed to upload that.".to_string()
        ))
    );
    // The sink never heard of it.
    assert!(seen.accepted.borrow().is_empty());
    rig.granted = true;
    let handle = rig
        .uploads()
        .begin("files", b"x".to_vec(), Box::new(content(1000, 1)))
        .unwrap();
    assert!(rig.run_until(100, |r| r.uploads().status(handle) == Some(Status::Done)));
}

#[test]
fn a_kind_the_server_does_not_have_is_refused_with_a_reason() {
    let seen = Rc::new(Seen::default());
    let mut rig = Rig::new(FilesPlugin::new(&seen));
    let handle = rig
        .uploads()
        .begin("other", b"x".to_vec(), Box::new(content(1000, 1)))
        .unwrap();
    rig.tick();
    assert_eq!(
        rig.uploads().status(handle),
        Some(Status::Failed(
            "The server does not take that kind of upload.".to_string()
        ))
    );
}

#[test]
fn a_client_that_sends_what_was_not_asked_for_is_dropped_and_counted() {
    let seen = Rc::new(Seen::default());
    let mut rig = Rig::new(FilesPlugin::new(&seen));
    let bytes = content(2 * BLOCK_BYTES, 3);
    let handle = rig
        .uploads()
        .begin("files", b"x".to_vec(), Box::new(bytes.clone()))
        .unwrap();
    rig.tick();
    let Some(Status::Sending { .. }) = rig.uploads().status(handle) else {
        panic!("{:?}", rig.uploads().status(handle));
    };
    // A message for an upload that exists, a run nobody asked for: the connection is dropped.
    rig.world
        .to_server
        .borrow_mut()
        .messages
        .push((CONN, encode_data(1, 1, &bytes[BLOCK_BYTES..])));
    rig.tick();
    assert_eq!(rig.world.dropped.borrow().as_slice(), [CONN]);
    assert_eq!(
        rig.server_vm.global("$Server::UploadViolations").as_deref(),
        Some(&b"1"[..])
    );
}

#[test]
fn garbage_on_the_channel_drops_the_connection_and_a_flood_does_too() {
    let seen = Rc::new(Seen::default());
    let mut rig = Rig::new(FilesPlugin::new(&seen));
    rig.world
        .to_server
        .borrow_mut()
        .messages
        .push((CONN, vec![9, 9, 9]));
    rig.tick();
    assert_eq!(rig.world.dropped.borrow().as_slice(), [CONN]);
    rig.world.to_server.borrow_mut().flooders.push(ConnId(8));
    rig.tick();
    assert_eq!(rig.world.dropped.borrow().as_slice(), [CONN, ConnId(8)]);
}

#[test]
fn a_connection_that_cannot_take_a_request_now_is_asked_again_next_tick() {
    let seen = Rc::new(Seen::default());
    let mut rig = Rig::new(FilesPlugin::new(&seen));
    rig.world.busy_requests.set(3);
    let handle = rig
        .uploads()
        .begin("files", b"x".to_vec(), Box::new(content(600_000, 2)))
        .unwrap();
    assert!(rig.run_until(300, |r| r.uploads().status(handle) == Some(Status::Done)));
    assert_eq!(rig.world.busy_requests.get(), 0);
}

#[test]
fn only_one_sink_runs_in_a_tick() {
    let seen = Rc::new(Seen::default());
    let mut rig = Rig::new(FilesPlugin::new(&seen));
    let uploads = rig.uploads();
    let a = uploads
        .begin("files", b"a".to_vec(), Box::new(content(1000, 1)))
        .unwrap();
    let b = uploads
        .begin("files", b"b".to_vec(), Box::new(content(1000, 2)))
        .unwrap();
    // Both finish their transfer in the same tick; the sinks run in consecutive ticks.
    let mut counts = Vec::new();
    for _ in 0..40 {
        rig.tick();
        counts.push(seen.finished.borrow().len());
    }
    assert_eq!(*counts.last().unwrap(), 2);
    assert!(counts.windows(2).all(|w| w[1] - w[0] <= 1), "{counts:?}");
    assert_eq!(uploads.status(a), Some(Status::Done));
    assert_eq!(uploads.status(b), Some(Status::Done));
}

#[test]
fn a_new_connection_ends_the_uploads_of_the_old_one() {
    let seen = Rc::new(Seen::default());
    let mut rig = Rig::new(FilesPlugin::new(&seen));
    // The connection takes no request, so the upload stays where it is.
    rig.world.busy_requests.set(1_000);
    let handle = rig
        .uploads()
        .begin(
            "files",
            b"x".to_vec(),
            Box::new(content(5 * BLOCK_BYTES, 2)),
        )
        .unwrap();
    rig.tick();
    rig.tick();
    let status = rig.uploads().status(handle);
    assert!(matches!(status, Some(Status::Sending { .. })), "{status:?}");
    rig.world.epoch.set(1);
    rig.tick();
    assert_eq!(
        rig.uploads().status(handle),
        Some(Status::Failed("The connection ended.".to_string()))
    );
}

#[test]
fn a_cancel_reaches_the_server_and_frees_what_it_held() {
    let seen = Rc::new(Seen::default());
    let mut rig = Rig::new(FilesPlugin::new(&seen));
    let uploads = rig.uploads();
    let handle = uploads
        .begin(
            "files",
            b"x".to_vec(),
            Box::new(content(5 * BLOCK_BYTES, 2)),
        )
        .unwrap();
    rig.tick();
    uploads.cancel(handle);
    assert_eq!(
        uploads.status(handle),
        Some(Status::Failed("Cancelled.".to_string()))
    );
    rig.tick();
    assert_eq!(rig.world.cancels.borrow().as_slice(), [1]);
}

#[test]
fn a_message_longer_than_the_offer_can_hold_is_refused_before_it_is_sent() {
    let seen = Rc::new(Seen::default());
    let rig = Rig::new(FilesPlugin::new(&seen));
    let e = rig
        .uploads()
        .begin(
            "files",
            vec![0; MAX_META_BYTES + 1],
            Box::new(content(10, 1)),
        )
        .unwrap_err();
    assert!(matches!(e, UploadError::Client(ClientError::BadRequest(_))));
    let e = rig
        .uploads()
        .begin("files", vec![], Box::new(Vec::<u8>::new()))
        .unwrap_err();
    assert_eq!(e, UploadError::Client(ClientError::BadSize));
}

#[test]
fn the_messages_fit_their_rpcs() {
    // The begin offer with the longest kind name and the longest description is one RPC message of at most 1,024 bytes.
    let seen = Rc::new(Seen::default());
    let engine = engine(FilesPlugin::new(&seen));
    let table = engine.net_rpcs().unwrap();
    let entry = table.get(table.id(BEGIN_RPC).unwrap()).unwrap();
    assert_eq!(entry.max_bytes, 1_024);
    let mut link = crate::rpc::RpcLink::new(
        table.clone(),
        crate::rpc::Side::Client,
        None,
        NetWidths::engine_default(),
        Rc::new(NullCarrier),
        0,
    );
    let begin = BeginUpload {
        kind: "k".repeat(40),
        meta: vec![255; MAX_META_BYTES],
        size_low: u32::MAX,
        size_high: u32::MAX,
        hash: [255; 32],
    };
    let rpc = link.rpc::<BeginUpload, BeginReply>(BEGIN_RPC).unwrap();
    assert!(link.call(&rpc, begin, |_| {}).is_ok());
}

struct NullCarrier;

impl crate::rpc::RpcCarrier for NullCarrier {
    fn unacked(&self) -> usize {
        0
    }

    fn send(
        &self,
        _batch: Vec<u8>,
        _delivery: crate::rpc::Delivery,
    ) -> Result<(), crate::rpc::CarrierFull> {
        Ok(())
    }
}

#[test]
fn reasons_are_cut_on_a_character_boundary() {
    let long = "é".repeat(150);
    let cut = short(&long);
    assert!(cut.len() <= MAX_REASON);
    assert!(cut.chars().all(|c| c == 'é'));
    assert_eq!(short("short"), "short");
}

#[test]
fn the_permission_view_names_who_is_calling() {
    assert_eq!(NoPermissions.subject(), None);
}

#[test]
fn one_client_filling_the_inbox_does_not_get_another_dropped() {
    let mut channel = UploadChannel::default();
    let message = vec![0u8; 256 << 10];
    let (a, b, c) = (ConnId(1), ConnId(2), ConnId(3));
    // Two clients each hold exactly their share, which together is more than any one connection may hold; a third then sends one.
    for _ in 0..(INBOX_PER_CONNECTION / message.len()) {
        channel.receive_from(a, Kind::Snapshot, &message);
        channel.receive_from(b, Kind::Snapshot, &message);
    }
    channel.receive_from(c, Kind::Snapshot, &[1, 2, 3]);
    let inbox = channel.take();
    assert!(inbox.flooders.is_empty(), "{:?}", inbox.flooders);
    assert_eq!(
        inbox.messages.iter().filter(|(from, _)| *from == c).count(),
        1
    );
    // A client over its own share is the one dropped, and what it sent after is not kept; the others go on.
    for _ in 0..=(INBOX_PER_CONNECTION / message.len()) {
        channel.receive_from(a, Kind::Snapshot, &message);
    }
    channel.receive_from(b, Kind::Snapshot, &[9]);
    let inbox = channel.take();
    assert_eq!(inbox.flooders, [a]);
    assert!(
        inbox
            .messages
            .iter()
            .any(|(from, bytes)| *from == b && bytes == &[9])
    );
    // The shares are counted again each tick.
    channel.receive_from(a, Kind::Snapshot, &message);
    assert!(channel.take().flooders.is_empty());
}

#[test]
fn a_permission_lost_before_the_upload_finishes_is_checked_again() {
    let seen = Rc::new(Seen::default());
    let mut plugin = FilesPlugin::new(&seen);
    plugin.permission = Some("files.upload");
    let mut rig = Rig::new(plugin);
    rig.world.hold_requests.set(true);
    let handle = rig
        .uploads()
        .begin(
            "files",
            b"x".to_vec(),
            Box::new(content(BLOCK_BYTES * 2, 4)),
        )
        .unwrap();
    rig.tick();
    rig.tick();
    // The offer was accepted with the permission; before the content is in, it is taken away.
    assert!(matches!(
        rig.uploads().status(handle),
        Some(Status::Sending { .. })
    ));
    rig.world.finish_granted.set(false);
    rig.world.hold_requests.set(false);
    assert!(rig.run_until(100, |r| matches!(
        r.uploads().status(handle),
        Some(Status::Failed(_))
    )));
    assert_eq!(
        rig.uploads().status(handle),
        Some(Status::Failed(
            "You are not allowed to upload that.".to_string()
        ))
    );
    assert!(seen.finished.borrow().is_empty(), "the sink never took it");
}

#[test]
fn a_slow_connection_that_keeps_delivering_is_not_ended_and_one_that_stops_is() {
    let seen = Rc::new(Seen::default());
    let mut plugin = FilesPlugin::new(&seen);
    plugin.idle_secs = Some(1);
    let mut rig = Rig::new(plugin);
    rig.world.hold_requests.set(true);
    let handle = rig
        .uploads()
        .begin(
            "files",
            b"x".to_vec(),
            Box::new(content(BLOCK_BYTES * 2, 4)),
        )
        .unwrap();
    // Ten seconds, ten times the idle time, on a link that delivers 3 KiB a second: still going.
    for _ in 0..300 {
        rig.world.received.set(rig.world.received.get() + 100);
        rig.tick();
    }
    assert!(
        matches!(rig.uploads().status(handle), Some(Status::Sending { .. })),
        "{:?}",
        rig.uploads().status(handle)
    );
    // The run it asked for comes in at last.
    rig.world.hold_requests.set(false);
    assert!(rig.run_until(100, |r| r.uploads().status(handle) == Some(Status::Done)));
    // A connection that stops delivering is ended after the idle time.
    let mut rig = Rig::new({
        let mut plugin = FilesPlugin::new(&seen);
        plugin.idle_secs = Some(1);
        plugin
    });
    rig.world.hold_requests.set(true);
    let handle = rig
        .uploads()
        .begin(
            "files",
            b"y".to_vec(),
            Box::new(content(BLOCK_BYTES * 2, 5)),
        )
        .unwrap();
    assert!(rig.run_until(300, |r| matches!(
        r.uploads().status(handle),
        Some(Status::Failed(_))
    )));
    assert_eq!(
        rig.uploads().status(handle),
        Some(Status::Failed("The upload stalled.".to_string()))
    );
}

#[test]
fn a_tick_that_finds_the_transfer_state_in_use_waits_and_loses_nothing() {
    let seen = Rc::new(Seen::default());
    let mut rig = Rig::new(FilesPlugin::new(&seen));
    let handle = rig
        .uploads()
        .begin(
            "files",
            b"x".to_vec(),
            Box::new(content(BLOCK_BYTES * 2, 4)),
        )
        .unwrap();
    rig.tick();
    let uploads = rig.server_vm.extension::<ServerUploads>().unwrap();
    {
        // A sink that ran script holds the state while a tick comes: the tick's work waits.
        let _held = uploads.state_for_test().borrow_mut();
        rig.tick();
        rig.tick();
        assert!(matches!(
            rig.uploads().status(handle),
            Some(Status::Sending { .. })
        ));
    }
    assert!(rig.run_until(100, |r| r.uploads().status(handle) == Some(Status::Done)));
    assert_eq!(seen.finished.borrow().len(), 1);
}

/// A client's packets read by the server's own reader of the upload channel, as the network host reads them. Channel 0 is another
/// channel of the connection and channel 1 the upload channel.
struct Link {
    tx: torque_net::chunk::ChunkSender,
    rx: torque_net::chunk::upstream::UpstreamReader,
    now: u32,
}

impl Link {
    const UPLOAD: usize = 1;

    /// A sender with a large message queued on the other channel, on the upload channel, or both.
    fn new(other: bool, upload: bool) -> Link {
        use torque_net::chunk::upstream::{UpstreamCaps, UpstreamServer};
        let limits = [
            torque_net::chunk::ChannelLimits::new(1_100).with_max_message(1 << 20),
            torque_net::chunk::ChannelLimits::new(1_100).with_max_message(1 << 20),
        ];
        let server = UpstreamServer::new(UpstreamCaps {
            bytes_per_second: 64 << 20,
            byte_burst: 64 << 20,
            ..UpstreamCaps::default()
        });
        let mut tx = torque_net::chunk::ChunkSender::new(&limits);
        if other {
            tx.enqueue(0, Kind::Snapshot, vec![3u8; 500_000]).unwrap();
        }
        if upload {
            tx.enqueue(Self::UPLOAD, Kind::Snapshot, vec![9u8; 500_000])
                .unwrap();
        }
        Link {
            tx,
            rx: server.connection(&limits),
            now: 0,
        }
    }

    /// The bytes of one packet of the sender, not yet read.
    fn packet(&mut self, seq: u32) -> Vec<u8> {
        let mut w = torque_net::bits::BitWriter::new();
        self.tx.write(&mut w, seq, 1_100 * 8);
        self.tx.on_notify(seq, true);
        w.into_bytes()
    }

    fn read(&mut self, packet: &[u8]) {
        self.now += 32;
        let mut out = Vec::new();
        self.rx
            .read(
                &mut torque_net::bits::BitReader::new(packet),
                self.now,
                &mut out,
            )
            .expect("the reader takes it");
    }

    /// What the host counts for the upload channel.
    fn upload_bytes(&self) -> u64 {
        self.rx.fresh_bytes_of(Self::UPLOAD)
    }
}

/// Runs a connection whose upload waits for its runs while `feed` gives the reader a packet each tick: the tick at which the upload
/// failed, if it did, and its status then.
fn run_stalled(link: &mut Link, mut feed: impl FnMut(&mut Link, u32)) -> (Option<u32>, Status) {
    let seen = Rc::new(Seen::default());
    let mut plugin = FilesPlugin::new(&seen);
    plugin.idle_secs = Some(1);
    let mut rig = Rig::new(plugin);
    rig.world.hold_requests.set(true);
    let handle = rig
        .uploads()
        .begin(
            "files",
            b"x".to_vec(),
            Box::new(content(BLOCK_BYTES * 2, 4)),
        )
        .unwrap();
    for tick in 1..=300u32 {
        feed(link, tick);
        rig.world.received.set(link.upload_bytes());
        rig.tick();
        if let Some(status @ Status::Failed(_)) = rig.uploads().status(handle) {
            return (Some(tick), status);
        }
    }
    (None, rig.uploads().status(handle).expect("the upload"))
}

#[test]
fn only_new_bytes_of_the_upload_channel_keep_a_stalled_upload_alive() {
    // New chunks of the upload channel: alive for ten seconds on an idle time of one.
    let mut link = Link::new(false, true);
    let (ended, status) = run_stalled(&mut link, |link, tick| {
        let packet = link.packet(tick);
        link.read(&packet);
    });
    assert_eq!(ended, None, "{status:?}");
    assert!(matches!(status, Status::Sending { .. }));

    // The same packet over and over: as many bytes on the link, none of them new.
    let mut link = Link::new(false, true);
    let first = link.packet(1);
    let (ended, status) = run_stalled(&mut link, |link, _| link.read(&first));
    assert!(ended.is_some_and(|tick| tick < 150), "{ended:?}");
    assert_eq!(status, Status::Failed("The upload stalled.".to_string()));

    // Plenty of new bytes, on another channel of the connection: not the upload's.
    let mut link = Link::new(true, false);
    let (ended, status) = run_stalled(&mut link, |link, tick| {
        let packet = link.packet(tick);
        link.read(&packet);
    });
    assert!(ended.is_some_and(|tick| tick < 150), "{ended:?}");
    assert_eq!(status, Status::Failed("The upload stalled.".to_string()));
}
