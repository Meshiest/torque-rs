//! Uploads over a link that loses packets: a 16 MiB upload arrives with the right hash, a resume after a disconnect asks only for
//! what is missing, and a client that breaks the rules loses its connection.

use crate::common;

use std::time::Duration;

use common::{Sim, TempFolder, content, sha256};
use torque_transfer::{
    BLOCK_BYTES, MAX_RUN_BLOCKS, Quota, Refusal, StagingConfig, Status, UploadSpec, Violation,
    block_count, encode_data,
};

fn spec(max_bytes: u64) -> UploadSpec {
    UploadSpec {
        max_bytes,
        quota: Quota {
            bytes: 1 << 30,
            per: Duration::from_secs(86_400),
            files: 100,
        },
        rate: 8 << 20,
        concurrent: 2,
        server_in_flight: 64 << 20,
        server_rate: 64 << 20,
        idle: Duration::from_secs(30),
        total: Duration::from_secs(3_600),
        resume: Duration::from_secs(300),
        meta_max: 64,
        permission: None,
    }
}

fn staging(folder: &TempFolder) -> StagingConfig {
    StagingConfig {
        memory_per_upload: 1 << 20,
        memory_total: 2 << 20,
        disk_total: 256 << 20,
        per_subject: 1 << 30,
        folder: Some(folder.0.clone()),
    }
}

#[test]
fn a_16_mib_upload_with_5_percent_loss_arrives_with_the_right_hash() {
    let folder = TempFolder::new("link-16mib");
    let bytes = content(16 << 20, 1);
    let want = sha256(&bytes);
    let mut sim = Sim::new(spec(16 << 20), staging(&folder), 5, 42);
    sim.begin(bytes).expect("accepted");
    assert!(
        sim.run_until(20_000, |s| !s.finished.is_empty() && !s.ended.is_empty()),
        "never finished: {:?}",
        sim.status()
    );
    assert!(sim.violations.is_empty(), "{:?}", sim.violations);
    let (_, len, got) = sim.finished[0];
    assert_eq!(len, 16 << 20);
    assert_eq!(got, want);
    assert_eq!(sim.status(), Some(&Status::Done));
    // The server held it on disk, and let go of everything.
    assert_eq!(sim.server.staged_disk(), 0);
    assert_eq!(sim.server.staged_memory(), 0);
    assert_eq!(sim.server.sessions(), 0);
    assert_eq!(std::fs::read_dir(&folder.0).unwrap().count(), 0);
    // The client sent each block about once; the link's loss is the chunk layer's to repair, not another request.
    assert_eq!(sim.data_sent, 16 << 20);
}

#[test]
fn a_small_upload_stages_in_memory_and_arrives() {
    let folder = TempFolder::new("link-small");
    let bytes = content(700_000, 2);
    let want = sha256(&bytes);
    let mut sim = Sim::new(spec(16 << 20), staging(&folder), 5, 7);
    sim.begin(bytes).expect("accepted");
    assert!(sim.run_until(5_000, |s| !s.finished.is_empty()));
    assert_eq!(sim.finished[0].2, want);
    assert_eq!(std::fs::read_dir(&folder.0).unwrap().count(), 0);
}

#[test]
fn a_resume_after_a_disconnect_at_half_asks_for_at_most_the_missing_blocks_and_one_run() {
    let folder = TempFolder::new("link-resume");
    let size = 16 << 20;
    let bytes = content(size, 3);
    let want = sha256(&bytes);
    let blocks = block_count(size as u64) as u32;
    let mut sim = Sim::new(spec(size as u64), staging(&folder), 5, 9);
    let first = sim.begin(bytes.clone()).expect("accepted");
    assert!(!first.resumed);
    assert!(sim.run_until(20_000, |s| s.data_received >= (size / 2) as u64));
    let sent_before = sim.data_sent;
    sim.reconnect();
    // The server keeps what it has.
    assert_eq!(sim.server.sessions(), 1);
    sim.data_sent = 0;
    let again = sim.begin(bytes).expect("accepted again");
    assert!(again.resumed, "{again:?}");
    assert!(again.has_list);
    assert!(
        again.have >= blocks / 2 - 2 * MAX_RUN_BLOCKS,
        "the server holds {} of {blocks}",
        again.have
    );
    assert!(sim.run_until(20_000, |s| !s.finished.is_empty() && !s.ended.is_empty()));
    assert_eq!(sim.finished[0].2, want);
    let missing = u64::from(blocks - again.have);
    let bound = (missing + u64::from(MAX_RUN_BLOCKS)) * BLOCK_BYTES as u64;
    assert!(
        sim.data_sent <= bound,
        "sent {} after the resume, at most {bound} allowed ({missing} blocks missing)",
        sim.data_sent
    );
    // The two connections together sent less than twice the content; the resume did not start over.
    assert!(sent_before + sim.data_sent < (size as u64) * 3 / 2 + bound);
}

#[test]
fn an_anonymous_player_cannot_resume_on_another_connection() {
    let folder = TempFolder::new("link-anon");
    let bytes = content(2 << 20, 4);
    let mut sim = Sim::new(spec(16 << 20), staging(&folder), 0, 3);
    sim.subject = torque_transfer::Subject::Connection(sim.conn);
    sim.begin(bytes.clone()).expect("accepted");
    sim.run_until(300, |s| s.data_received > 0);
    sim.reconnect();
    sim.subject = torque_transfer::Subject::Connection(sim.conn);
    let again = sim.begin(bytes).expect("accepted");
    assert!(!again.resumed);
    assert_eq!(again.have, 0);
}

#[test]
fn the_refusals_reach_the_player_with_their_messages() {
    let folder = TempFolder::new("link-refusals");
    let mut cfg = spec(1 << 20);
    cfg.concurrent = 1;
    cfg.quota.bytes = 3 << 20;
    cfg.server_in_flight = 2 << 20;
    let mut sim = Sim::new(cfg, staging(&folder), 0, 5);
    let e = sim.begin(content((1 << 20) + 1, 1)).unwrap_err();
    assert_eq!(
        e.to_string(),
        "That is 1.0 MiB and this kind of upload takes at most 1.0 MiB."
    );
    sim.begin(content(1 << 20, 2)).unwrap();
    let e = sim.begin(content(1000, 3)).unwrap_err();
    assert!(matches!(e, Refusal::Concurrent { max: 1 }), "{e:?}");
    assert_eq!(
        e.to_string(),
        "You already have 1 upload going; wait for it to finish."
    );
    assert!(matches!(sim.status(), Some(Status::Failed(_))));
}

#[test]
fn unrequested_data_drops_the_connection() {
    let folder = TempFolder::new("link-unrequested");
    let bytes = content(3 * BLOCK_BYTES, 6);
    let mut sim = Sim::new(spec(16 << 20), staging(&folder), 0, 1);
    let id = sim.begin(bytes.clone()).unwrap().id;
    assert!(sim.run_until(100, |s| !s.requests.is_empty()));
    // The server asked for blocks 0 to 2 as one run; a run that starts at block 1 is not what it asked for.
    assert_eq!(sim.requests, vec![(id, 0, 3)]);
    let message = encode_data(id, 1, &bytes[BLOCK_BYTES..2 * BLOCK_BYTES]);
    assert_eq!(sim.inject(&message), Err(Violation::Unrequested));
    assert!(sim.dropped);
    assert_eq!(sim.server.counters().violations, 1);
    // Nothing was kept, and the upload ended with the connection: it does not wait for a resume.
    assert_eq!(sim.server.sessions(), 0);
}

#[test]
fn a_bad_block_drops_the_connection() {
    let folder = TempFolder::new("link-bad-block");
    let bytes = content(3 * BLOCK_BYTES, 7);
    let mut sim = Sim::new(spec(16 << 20), staging(&folder), 0, 2);
    let id = sim.begin(bytes.clone()).unwrap().id;
    assert!(sim.run_until(100, |s| !s.requests.is_empty()));
    let mut bad = bytes.clone();
    bad[2 * BLOCK_BYTES + 10] ^= 0xff;
    assert_eq!(
        sim.inject(&encode_data(id, 0, &bad)),
        Err(Violation::BadBlockHash)
    );
    assert!(sim.dropped);
    // The honest answer, arriving after the drop, finds no connection to take it.
    sim.step();
    sim.step();
    sim.step();
    assert!(sim.finished.is_empty());
}
