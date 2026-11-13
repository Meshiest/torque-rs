# Uploads

A plugin can take **files from clients**: a saved build, a picture, a log. The engine does the part that is the same for every kind
of file and hard to get right (limits, hashing, resuming, hostile input) and hands the plugin the finished bytes. What the bytes mean,
where they are kept and who may send them are the plugin's.

Read [Networked plugins](networked-plugins.md) first: an upload is two of its pieces together, RPCs and a bulk channel.

## Registering a kind

```rust,ignore
use std::time::Duration;
use torque::engine::upload::{Quota, UntrustedBytes, UploadCx, UploadSink, UploadSpec};

struct Builds;

impl UploadSink for Builds {
    // Called when a client offers an upload, before anything is held for it.
    fn accept(&mut self, cx: &mut UploadCx<'_>, meta: &[u8]) -> Result<(), String> {
        if meta.is_empty() { Err("Give the build a name.".into()) } else { Ok(()) }
    }

    // Called once every block is in and the whole content matched its hash.
    fn finish(&mut self, cx: &mut UploadCx<'_>, meta: &[u8], bytes: UntrustedBytes<'_>) -> Result<(), String> {
        // Parse `bytes` as untrusted input; store it under `cx.subject`.
        Ok(())
    }
}

// In a plugin's build:
engine.net().upload("sandbox.build", UploadSpec {
    max_bytes: 16 << 20,
    quota: Quota { bytes: 64 << 20, per: Duration::from_secs(86_400), files: 32 },
    permission: Some("sandbox.upload".into()),
    ..UploadSpec::default()
}, Builds);
```

The first kind registers four RPCs (`torque.upload.begin`, `.cancel`, `.request`, `.end`) and a client-to-server channel
(`torque.upload`); a server and a client of the game must both have the plugin, and the manifest says so.

| Limit | Default | Meaning |
|---|---|---|
| `max_bytes` | 16 MiB | one upload |
| `quota` | 64 MiB, 32 uploads a day | accepted per player (the auth identity, else the connection) |
| `concurrent` | 1 | uploads one player may have, going or waiting to resume |
| `rate` | 128 KiB/s | requested from one connection |
| `server_rate` | 4 MiB/s | requested from all players together |
| `server_in_flight` | 64 MiB | declared bytes of the kind's uploads that hold their room (see below) |
| `idle`, `total`, `resume` | 30 s, 15 min, 5 min | quiet time, the whole upload, how long a cut-off upload waits |
| `meta_max` | 512 B | the description a client sends (at most 768) |
| `permission` | none | a permission the player needs (declared for the plugin, checked before the sink hears of the offer) |

## What happens

1. The client calls `uploads(vm).begin(kind, meta, source)`. The content is read once to hash it; the offer carries its size and
   SHA-256.
2. The server checks the size, the quota, the player's uploads going, the staged bytes, the permission and the sink's `accept`.
   Each refusal is a sentence for the player ("That would go over your limit of 64.0 MiB every 24 hours; you have 3.0 MiB left.").
3. The client sends the hash of each 64 KiB block. The server then **asks for runs** of up to four blocks, two at a time, as
   fast as `rate` and `server_rate` allow. The client sends only what it was asked for.
4. Each block is checked against its hash before a byte of it is kept, and the whole content once more before `finish`.

Because the server asks, it decides the pace and the memory it uses: a client cannot push more than was requested.

An upload that waits for the client is alive while its connection delivers: each byte buys the time a link of 1 KiB a second needs for
it, so a slow link that keeps delivering is never taken for stalled, and a trickle is. Heard of in `idle`, an upload gives up its room
against `server_in_flight` and goes last in line for new runs, and it is ended `idle` later. If its data comes in the meantime it takes
its room back, unless another player took it. An upload waiting for a resume gives up its room after `idle` too (its staging stays, held
by the staging's own caps). When an upload ends, its data still on the way is ignored, not taken for a breach. An upload that has a
connection is not resumed (the player begins it again once the server has seen that connection end), and a client does not begin the
same content twice at once.

## Hostile clients

Everything a client sends is hostile input. Data nobody asked for, a block whose hash does not match, a run of the wrong size
or a message that is not well formed drops the connection (and counts in `$Server::UploadViolations`). Staging never holds more
than its caps: memory up to 1 MiB for an upload, 8 MiB in all, a folder beyond that
(a game's own `engine.net().upload_staging(StagingConfig { folder: Some(..), .. })`, in which the engine deletes only files named like its own,
`upload-<16 hex digits>.part`; the app's server otherwise makes a folder for each run under the user's data folder,
`torque-rs/uploads/.staging/<run>`, holds a lock on its `run.lock` while it lives, caps it at 512 MiB, and removes the folders of runs
whose lock nobody holds when it starts, never one that is alive; it says on the console when it has no folder), 64 MiB for one player. File names in the folder are random and never come from the client; the folder is
emptied when the server starts and no script or mount may reach it. With no folder (the web), only uploads that fit in memory are taken.

A sink gets `UntrustedBytes`, a read-only view that is never loaded whole unless the sink asks for it with a cap. The engine never
interprets the bytes. Every sink ships a fuzz test that runs `torque_transfer::fuzz::fuzz_sink` (behind torque-transfer's `testing` feature, which the sink's dev-dependency turns on) over a valid sample
(`TORQUE_FUZZ_UPLOAD_<NAME>=100000`), and parses a large upload in steps, not in one tick: the sink's `finish` runs once a tick at most.

The permission a kind asks for is checked again when the content is complete, so an upload that was accepted is not taken from a player
who lost it on the way.

A server never takes code from a client: the engine ships no sink that executes an upload, and a game that wants one needs its
own security review.

## Resuming

If a connection ends while blocks are missing, the server keeps what it has for `resume`, still counted against the player's
quota and limits. A `begin` of the same content by the same player (an auth identity; a player without one resumes only within the
same connection) is accepted with the blocks the server holds, and only the rest is requested again. A resume does not move the
upload's deadline, an upload resumes at most eight times, and when a new upload would pass a player's limits the oldest of their
waiting uploads is evicted for it.

## How fast it goes

The channel is the engine's own and it shares the connection's packets with everything else, so the time an upload takes is the link's.
In the engine's protocol the client sends as fast as the link allows. In Torque's protocol the client's packet size and rate
(`$pref::Net::PacketSize`, the packet rate) decide, and with their defaults an upload runs at a few KiB a second: raise them for a
game whose players upload large files. A server that takes more than a few hundred KiB a second per client raises its
client-to-server channel caps (`ChannelHub::set_upstream_caps`) to match `rate`.

## Maintaining

The rules and their tests are in `crates/torque-transfer` (`lib.rs`'s Maintaining section) and `torque-engine`'s `upload` module.
`torque-transfer` is the state machines, with no sockets or clock: `UploadServer` (policy, scheduling, staging, resume) and
`UploadClient`. `torque-engine`'s `upload` is the registration, the RPC messages and the tick hooks; `torque-server`'s
`net::uploads` is the host's side of the two seams (`ServerLinks`, `ClientLinks`). The mods download uses the same block size and hashes.

Checks: `cargo test -p torque-transfer` (the 16 MiB upload with 5% loss, resume at half, every refusal, the counted-allocator peak),
`TORQUE_FUZZ_UPLOAD=100000 cargo test -p torque-transfer --test main hostile::`, `cargo test -p torque-engine upload`,
`cargo test -p torque-example-minimal-engine --test main uploads::` (both protocols), and the wire goldens and `just trace-hash`, which a game with no
upload kind must leave unchanged.
