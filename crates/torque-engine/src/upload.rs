//! Client-to-server uploads: files a client sends that a plugin's sink takes, under limits the server sets and holds every client
//! to ([`NetRegistry::upload`]).
//!
//! ```
//! use torque_engine::upload::{UploadCx, UploadSink, UploadSpec, UntrustedBytes};
//! use torque_engine::{EngineBuilder, Plugin, PluginId};
//!
//! struct Notes;
//!
//! impl UploadSink for Notes {
//!     fn accept(&mut self, _cx: &mut UploadCx<'_>, meta: &[u8]) -> Result<(), String> {
//!         if meta.is_empty() { Err("Give the note a name.".into()) } else { Ok(()) }
//!     }
//!
//!     fn finish(&mut self, cx: &mut UploadCx<'_>, meta: &[u8], bytes: UntrustedBytes<'_>) -> Result<(), String> {
//!         // The bytes are a client's: parse them under caps. Here they are only counted.
//!         let _ = (cx.conn, meta, bytes.len());
//!         Ok(())
//!     }
//! }
//!
//! #[derive(PluginId)]
//! #[plugin(id = "example.notes")]
//! struct NotesPlugin;
//!
//! impl Plugin for NotesPlugin {
//!     fn build(&self, engine: &mut EngineBuilder) {
//!         let spec = UploadSpec { max_bytes: 1 << 20, ..UploadSpec::default() };
//!         engine.net().upload("notes", spec, Notes);
//!     }
//! }
//!
//! let engine = EngineBuilder::new().add_plugins(NotesPlugin).build()?;
//! assert_eq!(engine.net_rpcs()?.len(), 4);
//! assert!(engine.net_channels()?.up_names().contains(&"torque.upload"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! **The flow.** A client calls [`Uploads::begin`] with the kind's name, a short description (bytes the sink reads) and the content
//! ([`Source`]). Its offer (the `torque.upload.begin` RPC) carries the size and the SHA-256; the server checks the size, the
//! player's quota, how many uploads the player has going and how many staged bytes it holds, the permission the kind asks for
//! ([`UploadSpec::permission`]) and the sink's [`accept`](UploadSink::accept), and answers with an id or a reason for the player.
//! The client then sends the hash of each 64 KiB block, and the server *requests runs* (`torque.upload.request`, a notification to the
//! client) of at most four blocks, two at a time, within `rate` and `server_rate`. The client answers each on the client-to-server
//! channel `torque.upload`. Every block is checked against its hash before a byte is kept, and the whole content again before the
//! sink's [`finish`](UploadSink::finish) gets it as [`UntrustedBytes`]. The server ends the upload with `torque.upload.end`.
//! Everything that crosses is hostile input on both sides: [`torque_transfer`] has the rules and their tests.
//!
//! **What a game provides.** A sink per kind, and (for uploads above the memory cap) a staging folder that no script or mount can
//! reach: the app's server makes one of its own under the user's data folder unless the game sets one
//! ([`NetRegistry::upload_staging`]). A game that wants uploads of more than a few hundred KiB a second raises the server's
//! client-to-server channel caps ([`torque_net::chunk::upstream::UpstreamCaps`]) to match its kinds' `rate`: the channel's own
//! caps drop a connection that sends more than they allow.
//!
//! **Scripts** read `$Server::UploadViolations`, the number of clients dropped for breaking the rules, and
//! `$Server::UploadsFinished`.
//!
//! # Maintaining
//!
//! **Module map.** This file: the registration (`NetRegistry::upload`), the sink trait and its context, and the recipes and hooks
//! that run each tick. `upload/messages.rs`: the RPC messages. `upload/links.rs`: the two seams ([`ServerLinks`], [`ClientLinks`])
//! the network host implements (`torque-server`'s `net::uploads`) and the server's half of the upload channel. `upload/server.rs`:
//! [`ServerUploads`] (an extension of the server's script engine). `upload/client.rs`: [`ClientUploads`] and [`Uploads`]. The
//! protocol itself is `torque-transfer`.
//!
//! **A server tick** (`ServerUploads::pump`, a `ServerTick::before` hook, after the RPC pump): take what the upload channel
//! received and how many bytes each connection delivered (so a slow link that is still delivering is not a stalled one), give each message to the transfer server (a violation drops that client and nothing else), poll it (requests go out,
//! ends queue), run at most one sink's `finish` (so a large parse cannot stall the tick), and send the queued ends. A request or
//! an end the connection cannot take now (busy) is kept or retracted and tried the next tick.
//!
//! **Invariants and their tests.** (`cargo test -p torque-engine upload`)
//! - Nothing a client sends can panic the server or take it past a cap (`torque-transfer`'s hostile test; the end-to-end tests in
//!   `torque-example-minimal-engine`).
//! - A permission the kind asks for is checked before the sink hears of the offer (`a_kind_that_asks_for_a_permission_refuses_a_player_without_it`).
//! - A permission is checked again when the content is complete (`a_permission_lost_before_the_upload_finishes_is_checked_again`).
//! - A tick that finds the transfer state in use waits and says so; it loses no message (`a_tick_that_finds_the_transfer_state_in_use_waits_and_loses_nothing`).
//! - The sink never runs inside a network borrow: `accept` in the RPC handler, `finish` in the tick hook.
//! - A registration the engine cannot honor fails the build with the reason (`registration_errors_fail_the_build`).

mod client;
mod links;
mod messages;
mod server;

use std::cell::RefCell;
use std::rc::Rc;

use torque_net::handshake::ConnId;
pub use torque_transfer::{
    ClientError, Counters, FileSource, Handle, Quota, Refusal, Source, StagingConfig, Status,
    UntrustedBytes, UploadSpec, Violation,
};
use torque_ts_vm::vm::Vm;

pub use client::{ClientUploads, UploadError, Uploads, uploads};
use links::channel_limits;
pub use links::{
    BeginDone, ClientLinks, ClientLinksSlot, LinkError, ServerLinks, ServerLinksSlot,
    UploadChannel, UploadInbox,
};
pub use messages::{BeginReply, BeginUpload, CancelUpload, UploadEnd, UploadRequest};
pub use server::ServerUploads;

use crate::net::NetRegistry;
use crate::permissions::{DeclaredPermission, PermissionView, SubjectKey};
use crate::rpc::{Allow, RpcError};
use crate::{ClientStage, ClientTick, Engine, PluginId, ServerStage, ServerTick, TickInfo};

/// The RPC a client offers an upload with.
pub const BEGIN_RPC: &str = "torque.upload.begin";
/// The RPC a client cancels an upload with.
pub const CANCEL_RPC: &str = "torque.upload.cancel";
/// The notification a server asks for a run of blocks with.
pub const REQUEST_RPC: &str = "torque.upload.request";
/// The notification a server ends an upload with.
pub const END_RPC: &str = "torque.upload.end";
/// The client-to-server channel the block lists and the data go on.
pub const CHANNEL: &str = "torque.upload";

/// What a sink is told about the call it runs in.
pub struct UploadCx<'a> {
    /// The server's script engine.
    pub vm: &'a mut Vm,
    /// The connection the upload is from.
    pub conn: ConnId,
    /// The player: the auth identity, else the connection. Key what the sink stores by this, never by anything the client says.
    pub subject: &'a SubjectKey,
    /// The tick.
    pub tick: u64,
    perms: &'a dyn PermissionView,
}

impl UploadCx<'_> {
    /// Whether the player holds `permission` now.
    pub fn has_permission(&self, permission: &str) -> bool {
        self.perms.has_permission(permission)
    }
}

/// What a plugin does with one kind of upload.
pub trait UploadSink: 'static {
    /// Judges an offer before anything is held for it: the permission and naming rules of the kind. `meta` is the description
    /// the client sent (at most the spec's `meta_max` bytes; hostile). `Err` refuses with a reason the player reads.
    ///
    /// # Errors
    /// The reason to refuse.
    fn accept(&mut self, cx: &mut UploadCx<'_>, meta: &[u8]) -> Result<(), String> {
        let _ = (cx, meta);
        Ok(())
    }

    /// Takes the finished content. Its hash is the one the client declared, which says only that these are the bytes the client
    /// meant to send: parse them as untrusted input, with caps, and store them where the kind keeps them. The engine never
    /// interprets them. `Err` ends the upload with the reason.
    ///
    /// # Errors
    /// The reason the content was not taken.
    fn finish(
        &mut self,
        cx: &mut UploadCx<'_>,
        meta: &[u8],
        bytes: UntrustedBytes<'_>,
    ) -> Result<(), String>;
}

/// A kind of upload a plugin registered ([`NetRegistry::upload`]).
#[derive(Clone)]
pub struct UploadEntry {
    /// The kind's name.
    pub name: String,
    /// The plugin that registered it.
    pub plugin: PluginId,
    /// Its limits.
    pub spec: UploadSpec,
    sink: Rc<RefCell<dyn UploadSink>>,
}

/// Where the staging folder is, as a game said (`NetRegistry::upload_staging`).
#[derive(Clone)]
struct StagingEntry(StagingConfig);

/// The staging a host gives servers whose game set none: an extension of the server's script engine, set before the engine's
/// recipes run (the app's server makes a folder of its own under the user's data folder).
pub struct HostStaging(pub StagingConfig);

impl NetRegistry<'_> {
    /// Registers a kind of upload: files a client sends the server, taken by `sink` under `spec`'s limits ([module docs](crate::upload)).
    /// The first kind registers the upload's RPCs and its client-to-server channel, so a server and a client of the game must both
    /// have the plugin. A name used twice, a name or a spec the transfer layer refuses, or a permission that is not a permission
    /// fails the build with the reason.
    pub fn upload(
        &mut self,
        name: &str,
        spec: UploadSpec,
        sink: impl UploadSink + 'static,
    ) -> &mut Self {
        let builder = self.builder();
        let plugin = builder.current_plugin();
        let problem = if name.is_empty() || name.len() > torque_transfer::MAX_KIND_NAME {
            Some("the name is 1 to 40 bytes".to_string())
        } else if builder.list::<UploadEntry>().iter().any(|e| e.name == name) {
            Some("another upload has this name".to_string())
        } else if let Err(e) = spec.validate() {
            Some(e.to_string())
        } else if let Some(p) = &spec.permission
            && let Err(e) = torque_permissions::Permission::parse(p)
        {
            Some(format!("the permission `{p}` is not one ({e})"))
        } else {
            None
        };
        if let Some(problem) = problem {
            builder.reject(format!("upload `{name}`: {problem}"));
            return self;
        }
        if builder.list::<UploadEntry>().is_empty() {
            register_surface(self);
        }
        let builder = self.builder();
        if let Some(permission) = &spec.permission
            && !builder
                .list::<DeclaredPermission>()
                .iter()
                .any(|d| d.name == *permission)
        {
            let description = format!("Upload {name}");
            builder.push(DeclaredPermission {
                plugin,
                name: permission.clone(),
                description,
            });
        }
        builder.push(UploadEntry {
            name: name.to_string(),
            plugin,
            spec,
            sink: Rc::new(RefCell::new(sink)),
        });
        self
    }

    /// Where uploads too large for memory are staged, and how much staging may hold ([`StagingConfig`]). The folder is the
    /// server's own: nothing a client sends is part of a file name in it, it is emptied when the server starts, and no script or
    /// mount may reach it. With none, only uploads that fit in memory are taken.
    pub fn upload_staging(&mut self, config: StagingConfig) -> &mut Self {
        self.builder().push(StagingEntry(config));
        self
    }
}

/// The RPCs, the channel, the recipes that make each side's state and the tick hooks, once for all kinds.
fn register_surface(net: &mut NetRegistry<'_>) {
    net.rpc::<BeginUpload, BeginReply>(BEGIN_RPC)
        .to_server()
        .rate(2, 4)
        .max_bytes(1_024)
        .allow(Allow::Anyone)
        .server(|cx, req| {
            let uploads = cx
                .vm
                .extension::<ServerUploads>()
                .ok_or(RpcError::Refused)?;
            Ok(uploads.begin(cx, &req))
        });
    net.rpc::<CancelUpload, ()>(CANCEL_RPC)
        .to_server()
        .rate(8, 16)
        .max_bytes(64)
        .server(|cx, cancel| {
            if let (Some(uploads), Some(conn)) = (cx.vm.extension::<ServerUploads>(), cx.caller()) {
                let tick = cx.tick();
                uploads.cancel(cx.vm, tick, conn, cancel.id);
            }
            Ok(())
        });
    net.rpc::<UploadRequest, ()>(REQUEST_RPC)
        .to_client()
        .rate(64, 128)
        .max_bytes(64)
        .client(|cx, request| {
            if let Some(uploads) = cx.vm.extension::<ClientUploads>() {
                uploads.on_request(&request);
            }
            Ok(())
        });
    net.rpc::<UploadEnd, ()>(END_RPC)
        .to_client()
        .rate(16, 32)
        .max_bytes(300)
        .client(|cx, end| {
            if let Some(uploads) = cx.vm.extension::<ClientUploads>() {
                uploads.on_end(&end);
            }
            Ok(())
        });
    net.channel_to_server_with(CHANNEL, channel_limits(), UploadChannel::default);
    let builder = net.builder();
    builder.server(ServerStage::Net, |vm, engine| {
        let host = vm.extension::<HostStaging>().map(|h| h.0.clone());
        let uploads = ServerUploads::new(engine, host)?;
        for warning in uploads.startup_warnings() {
            vm.warn(warning);
        }
        vm.set_extension(Rc::new(uploads));
        Ok(())
    });
    builder.push(ServerTick::before(
        |vm: &mut Vm, _engine: &Engine, tick: TickInfo| {
            let (Some(uploads), Some(links)) = (
                vm.extension::<ServerUploads>(),
                vm.extension::<ServerLinksSlot>(),
            ) else {
                return Ok(());
            };
            uploads.pump(vm, links.0.as_ref(), tick);
            Ok(())
        },
    ));
    builder.client(ClientStage::Classes, |vm, _| {
        vm.set_extension(Rc::new(ClientUploads::default()));
        Ok(())
    });
    builder.push(ClientTick::new(|vm: &mut Vm| {
        if let (Some(uploads), Some(links)) = (
            vm.extension::<ClientUploads>(),
            vm.extension::<ClientLinksSlot>(),
        ) {
            uploads.pump(links.0.as_ref());
        }
    }));
}

#[cfg(test)]
use links::INBOX_PER_CONNECTION;
#[cfg(test)]
use server::short;

#[cfg(test)]
#[path = "upload_tests.rs"]
mod tests;
