use std::time::Duration;

use super::*;
use crate::blocks::{BLOCK_BYTES, Hash, block_hashes, content_hash, run_bytes};
use crate::client::UploadClient;
use crate::message::{encode_block_list, encode_data};
use crate::spec::{Quota, Refusal, UploadSpec};
use crate::staging::StagingConfig;

const KIND: &str = "build";

fn spec() -> UploadSpec {
    UploadSpec {
        max_bytes: 2 << 20,
        quota: Quota {
            bytes: 4 << 20,
            per: Duration::from_secs(3_600),
            files: 3,
        },
        rate: 8 << 20,
        concurrent: 2,
        server_in_flight: 3 << 20,
        server_rate: 64 << 20,
        idle: Duration::from_secs(10),
        total: Duration::from_secs(100),
        resume: Duration::from_secs(60),
        meta_max: 16,
        permission: None,
    }
}

fn server(spec: UploadSpec) -> UploadServer {
    let staging = StagingConfig {
        memory_per_upload: 4 << 20,
        memory_total: 16 << 20,
        ..StagingConfig::default()
    };
    UploadServer::new(vec![(KIND.to_string(), spec)], staging).unwrap()
}

fn data(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(seed).wrapping_add(seed))
        .collect()
}

fn me() -> Subject {
    Subject::Identity(b"me".to_vec())
}

fn offer<'a>(content: &'a [u8], meta: &'a [u8]) -> (Offer<'a>, Hash) {
    let hash = content_hash(content);
    (
        Offer {
            kind: KIND,
            meta,
            size: content.len() as u64,
            hash,
        },
        hash,
    )
}

fn begin(
    server: &mut UploadServer,
    conn: u32,
    who: &Subject,
    content: &[u8],
) -> Result<Accepted, Refusal> {
    server.begin(0, ConnId(conn), who, &offer(content, b"m").0, |_| Ok(()))
}

/// Accepts `content` and sends its block list.
fn accept_with_list(server: &mut UploadServer, conn: u32, who: &Subject, content: &[u8]) -> u32 {
    let accepted = begin(server, conn, who, content).unwrap();
    let list = encode_block_list(accepted.id, &block_hashes(content));
    server.on_message(0, ConnId(conn), &list).unwrap();
    accepted.id
}

fn requests(out: &[Out]) -> Vec<(u32, u32, u32)> {
    out.iter()
        .filter_map(|o| match o {
            Out::Request {
                id, start, count, ..
            } => Some((*id, *start, *count)),
            _ => None,
        })
        .collect()
}

fn run_data(content: &[u8], id: u32, start: u32, count: u32) -> Vec<u8> {
    let (from, to) = run_bytes(content.len() as u64, u64::from(start), u64::from(count));
    encode_data(id, start, &content[from as usize..to as usize])
}

/// Runs a whole honest transfer of `content` and returns the finish.
fn transfer(server: &mut UploadServer, conn: u32, id: u32, content: &[u8], now: &mut Millis) {
    for _ in 0..10_000 {
        *now += 32;
        for out in server.poll(*now) {
            match out {
                Out::Request {
                    id, start, count, ..
                } => {
                    let message = run_data(content, id, start, count);
                    server.on_message(*now, ConnId(conn), &message).unwrap();
                }
                Out::Finish { id: done } => {
                    assert_eq!(done, id);
                    return;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
    panic!("never finished");
}

#[test]
fn an_honest_upload_reaches_the_sink_with_its_bytes() {
    let mut s = server(spec());
    let content = data(300_000, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    let mut now = 0;
    transfer(&mut s, 1, id, &content, &mut now);
    let view = s.content(id).unwrap();
    assert_eq!(view.to_vec(usize::MAX).unwrap(), content);
    assert_eq!(s.meta(id), Some(&b"m"[..]));
    assert_eq!(s.kind_of(id), Some(KIND));
    let end = s.complete(id, Ok(())).unwrap();
    assert_eq!(
        end,
        Out::End {
            conn: ConnId(1),
            id,
            result: Ok(())
        }
    );
    assert_eq!(s.sessions(), 0);
    assert_eq!(s.in_flight(KIND), 0);
    assert_eq!(s.staged_memory(), 0);
    assert_eq!(s.counters().completed, 1);
}

#[test]
fn the_sink_is_not_given_a_content_before_every_block_is_in() {
    let mut s = server(spec());
    let content = data(300_000, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    assert!(s.content(id).is_none());
    s.poll(1);
    assert!(s.content(id).is_none());
}

// ---- refusals, each with its message

#[test]
fn a_content_over_the_limit_is_refused_with_both_sizes() {
    let mut s = server(spec());
    let big = vec![0u8; (2 << 20) + 1];
    let e = begin(&mut s, 1, &me(), &big).unwrap_err();
    assert_eq!(
        e.to_string(),
        "That is 2.0 MiB and this kind of upload takes at most 2.0 MiB."
    );
    assert!(matches!(e, Refusal::TooLarge { .. }));
}

#[test]
fn an_empty_content_and_a_long_description_and_an_unknown_kind_are_refused() {
    let mut s = server(spec());
    let mut o = offer(b"x", b"m").0;
    o.size = 0;
    let e = s.begin(0, ConnId(1), &me(), &o, |_| Ok(())).unwrap_err();
    assert_eq!(e.to_string(), "There is nothing to upload.");
    let long = [0u8; 17];
    let o = offer(b"x", &long).0;
    let e = s.begin(0, ConnId(1), &me(), &o, |_| Ok(())).unwrap_err();
    assert_eq!(
        e.to_string(),
        "The description is 17 bytes and at most 16 are taken."
    );
    let mut o = offer(b"x", b"m").0;
    o.kind = "other";
    let e = s.begin(0, ConnId(1), &me(), &o, |_| Ok(())).unwrap_err();
    assert_eq!(
        e.to_string(),
        "The server does not take that kind of upload."
    );
}

#[test]
fn the_quota_refuses_bytes_with_what_is_left() {
    let mut cfg = spec();
    cfg.quota.bytes = 3 << 20;
    cfg.concurrent = 3;
    cfg.server_in_flight = 8 << 20;
    let mut s = server(cfg);
    begin(&mut s, 1, &me(), &data(2 << 20, 1)).unwrap();
    let e = begin(&mut s, 1, &me(), &data((1 << 20) + 1, 2)).unwrap_err();
    assert_eq!(
        e.to_string(),
        "That would go over your limit of 3.0 MiB every 60 minutes; you have 1.0 MiB left."
    );
    // Another player has a quota of their own.
    begin(
        &mut s,
        2,
        &Subject::Identity(b"you".to_vec()),
        &data(2 << 20, 3),
    )
    .unwrap();
}

#[test]
fn the_quota_refuses_a_fourth_upload_in_its_window_and_takes_one_after() {
    let mut cfg = spec();
    cfg.concurrent = 3;
    cfg.server_in_flight = 3 << 20;
    let mut s = server(cfg);
    for seed in 1..=3 {
        let id = begin(&mut s, 1, &me(), &data(1000, seed)).unwrap().id;
        s.cancel(0, ConnId(1), id);
    }
    let e = begin(&mut s, 1, &me(), &data(1000, 9)).unwrap_err();
    assert_eq!(
        e.to_string(),
        "You can send at most 3 uploads every 60 minutes."
    );
    // An hour later the window has passed.
    let content = data(1000, 9);
    let o = offer(&content, b"m").0;
    assert!(s.begin(3_600_001, ConnId(1), &me(), &o, |_| Ok(())).is_ok());
}

#[test]
fn concurrency_refuses_an_upload_over_the_players_limit() {
    let mut cfg = spec();
    cfg.concurrent = 1;
    let mut s = server(cfg);
    begin(&mut s, 1, &me(), &data(1000, 1)).unwrap();
    let e = begin(&mut s, 1, &me(), &data(1000, 2)).unwrap_err();
    assert_eq!(
        e.to_string(),
        "You already have 1 upload going; wait for it to finish."
    );
    // Another player is not held by it, and the limit is per kind and player, not per connection.
    begin(
        &mut s,
        2,
        &Subject::Identity(b"you".to_vec()),
        &data(1000, 2),
    )
    .unwrap();
    let e = begin(&mut s, 3, &me(), &data(1000, 4)).unwrap_err();
    assert!(matches!(e, Refusal::Concurrent { max: 1 }));
}

#[test]
fn server_in_flight_refuses_what_would_pass_the_servers_staged_bytes() {
    let mut cfg = spec();
    cfg.server_in_flight = 2 << 20;
    cfg.max_bytes = 2 << 20;
    cfg.quota.bytes = 64 << 20;
    let mut s = server(cfg);
    let a = Subject::Identity(b"a".to_vec());
    let b = Subject::Identity(b"b".to_vec());
    begin(&mut s, 1, &a, &data(1 << 20, 1)).unwrap();
    begin(&mut s, 2, &b, &data(1 << 20, 2)).unwrap();
    let e = begin(&mut s, 3, &Subject::Identity(b"c".to_vec()), &data(1000, 3)).unwrap_err();
    assert_eq!(e, Refusal::ServerFull);
    assert_eq!(
        e.to_string(),
        "The server is taking too many uploads right now; try again later."
    );
    assert_eq!(s.in_flight(KIND), 2 << 20);
}

#[test]
fn the_kinds_own_rules_refuse_with_their_reason() {
    let mut s = server(spec());
    let o = offer(b"abc", b"m").0;
    let e = s
        .begin(0, ConnId(1), &me(), &o, |_| {
            Err("That name is taken.".to_string())
        })
        .unwrap_err();
    assert_eq!(e.to_string(), "That name is taken.");
}

#[test]
fn a_refused_offer_holds_nothing() {
    let mut s = server(spec());
    let o = offer(b"abc", b"m").0;
    let _ = s.begin(0, ConnId(1), &me(), &o, |_| Err("no".to_string()));
    let _ = begin(&mut s, 1, &me(), &vec![0u8; 3 << 20]);
    assert_eq!(s.sessions(), 0);
    assert_eq!(s.in_flight(KIND), 0);
    assert_eq!(s.staged_memory(), 0);
    assert_eq!(s.counters().refused, 2);
    // A refusal by the kind's rules left no quota entry either: three uploads still fit.
    for seed in 1..=3 {
        let id = begin(&mut s, 1, &me(), &data(1000, seed)).unwrap().id;
        s.cancel(0, ConnId(1), id);
    }
}

// ---- hostile messages

#[test]
fn unrequested_data_is_a_violation() {
    let mut s = server(spec());
    let content = data(1000, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    // Nothing was requested yet.
    let v = s
        .on_message(0, ConnId(1), &run_data(&content, id, 0, 1))
        .unwrap_err();
    assert_eq!(v, Violation::Unrequested);
    assert_eq!(s.counters().violations, 1);
}

#[test]
fn data_for_a_block_that_was_not_the_one_requested_is_a_violation() {
    let mut s = server(spec());
    let content = data(BLOCK_BYTES * 6, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    let asked = requests(&s.poll(1));
    assert_eq!(asked, vec![(id, 0, 4), (id, 4, 2)]);
    // Blocks 1..2 are inside a run but are not a run.
    let v = s
        .on_message(2, ConnId(1), &run_data(&content, id, 1, 1))
        .unwrap_err();
    assert_eq!(v, Violation::Unrequested);
}

#[test]
fn a_run_of_the_wrong_size_is_a_violation() {
    let mut s = server(spec());
    let content = data(BLOCK_BYTES * 3, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    assert_eq!(requests(&s.poll(1)), vec![(id, 0, 3)]);
    let v = s
        .on_message(2, ConnId(1), &run_data(&content, id, 0, 2))
        .unwrap_err();
    assert_eq!(v, Violation::WrongLength);
}

#[test]
fn a_bad_block_hash_is_a_violation_and_nothing_is_kept() {
    let mut s = server(spec());
    let content = data(BLOCK_BYTES * 2, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    assert_eq!(requests(&s.poll(1)), vec![(id, 0, 2)]);
    let mut bad = content.clone();
    bad[BLOCK_BYTES + 5] ^= 1;
    let v = s
        .on_message(2, ConnId(1), &run_data(&bad, id, 0, 2))
        .unwrap_err();
    assert_eq!(v, Violation::BadBlockHash);
    // Not even the good first block was kept, and the run is still outstanding, not satisfied.
    s.disconnected(3, ConnId(1));
    s.begin(4, ConnId(2), &me(), &offer(&content, b"m").0, |_| Ok(()))
        .map(|a| assert_eq!(a.have, 0))
        .unwrap();
}

#[test]
fn data_of_an_upload_the_connection_does_not_have_is_a_violation() {
    let mut s = server(spec());
    let content = data(1000, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    s.poll(1);
    // Another connection cannot feed it, and an id that never existed is not tolerated.
    let message = run_data(&content, id, 0, 1);
    assert_eq!(
        s.on_message(2, ConnId(2), &message).unwrap_err(),
        Violation::UnknownUpload
    );
    let message = run_data(&content, id + 100, 0, 1);
    assert_eq!(
        s.on_message(2, ConnId(1), &message).unwrap_err(),
        Violation::UnknownUpload
    );
}

#[test]
fn malformed_messages_are_violations() {
    let mut s = server(spec());
    for bytes in [&[][..], &[9, 0, 0, 0, 0][..], &[2, 1][..]] {
        assert!(matches!(
            s.on_message(0, ConnId(1), bytes),
            Err(Violation::Malformed(_))
        ));
    }
    assert_eq!(s.counters().violations, 3);
}

#[test]
fn a_second_block_list_or_one_of_the_wrong_length_is_a_violation() {
    let content = data(BLOCK_BYTES * 2, 3);
    let mut s = server(spec());
    let accepted = begin(&mut s, 1, &me(), &content).unwrap();
    let short = encode_block_list(accepted.id, &block_hashes(&content)[..1]);
    assert_eq!(
        s.on_message(0, ConnId(1), &short).unwrap_err(),
        Violation::BadBlockList
    );
    // Each is its own offence: a fresh server and upload for the second.
    let mut s = server(spec());
    let accepted = begin(&mut s, 1, &me(), &content).unwrap();
    let list = encode_block_list(accepted.id, &block_hashes(&content));
    s.on_message(0, ConnId(1), &list).unwrap();
    assert_eq!(
        s.on_message(0, ConnId(1), &list).unwrap_err(),
        Violation::BadBlockList
    );
}

#[test]
fn a_content_that_does_not_match_its_hash_drops_the_connection() {
    let mut s = server(spec());
    let truth = data(1000, 3);
    let lie = data(1000, 5);
    // The client declares the hash of one content and sends the blocks (and their list) of another.
    let o = offer(&truth, b"m").0;
    let accepted = s.begin(0, ConnId(1), &me(), &o, |_| Ok(())).unwrap();
    s.on_message(
        0,
        ConnId(1),
        &encode_block_list(accepted.id, &block_hashes(&lie)),
    )
    .unwrap();
    let mut dropped = false;
    for step in 1..50 {
        for out in s.poll(step * 32) {
            match out {
                Out::Request {
                    id, start, count, ..
                } => {
                    s.on_message(step * 32, ConnId(1), &run_data(&lie, id, start, count))
                        .unwrap();
                }
                Out::Drop { conn } => {
                    assert_eq!(conn, ConnId(1));
                    dropped = true;
                }
                Out::Finish { .. } => panic!("a content with the wrong hash reached the sink"),
                _ => {}
            }
        }
    }
    assert!(dropped);
    assert_eq!(s.sessions(), 0);
    assert_eq!(s.staged_memory(), 0);
}

#[test]
fn late_data_of_a_closed_upload_is_tolerated_once_per_request() {
    let mut s = server(spec());
    let content = data(BLOCK_BYTES * 12, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    assert_eq!(requests(&s.poll(1)), vec![(id, 0, 4), (id, 4, 4)]);
    s.cancel(2, ConnId(1), id);
    // The two runs already on their way arrive; a third nobody asked for is a violation.
    assert_eq!(
        s.on_message(3, ConnId(1), &run_data(&content, id, 0, 4)),
        Ok(None)
    );
    assert_eq!(
        s.on_message(3, ConnId(1), &run_data(&content, id, 4, 4)),
        Ok(None)
    );
    assert_eq!(
        s.on_message(3, ConnId(1), &run_data(&content, id, 8, 4))
            .unwrap_err(),
        Violation::UnknownUpload
    );
}

// ---- pacing and time

#[test]
fn requests_come_two_runs_at_a_time_within_the_rate() {
    let mut cfg = spec();
    cfg.rate = 256 << 10;
    cfg.max_bytes = 2 << 20;
    let mut s = server(cfg);
    let content = data(2 << 20, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    // The bucket holds two runs (512 KiB) at the start and answers nothing more until time passes.
    let first = requests(&s.poll(0));
    assert_eq!(first, vec![(id, 0, 4), (id, 4, 4)]);
    assert!(requests(&s.poll(10)).is_empty());
    // Sending the data does not free the pace: only time does. 256 KiB a second: one run takes a second.
    s.on_message(20, ConnId(1), &run_data(&content, id, 0, 4))
        .unwrap();
    assert!(requests(&s.poll(500)).is_empty());
    assert_eq!(requests(&s.poll(1_100)), vec![(id, 8, 4)]);
}

#[test]
fn the_server_rate_is_shared_by_every_connection() {
    let mut cfg = spec();
    cfg.rate = 8 << 20;
    cfg.server_rate = 256 << 10;
    cfg.concurrent = 1;
    cfg.server_in_flight = 8 << 20;
    let mut s = server(cfg);
    let a = accept_with_list(
        &mut s,
        1,
        &Subject::Identity(b"a".to_vec()),
        &data(2 << 20, 1),
    );
    let b = accept_with_list(
        &mut s,
        2,
        &Subject::Identity(b"b".to_vec()),
        &data(2 << 20, 2),
    );
    let granted = requests(&s.poll(0));
    // The kind's bucket holds two runs: they go round robin, one to each.
    assert_eq!(granted.len(), 2);
    let ids: Vec<u32> = granted.iter().map(|g| g.0).collect();
    assert!(ids.contains(&a) && ids.contains(&b));
}

#[test]
fn a_run_refused_by_its_connections_rate_spends_none_of_the_servers() {
    // A connection's bucket holds two runs, the kind's four.
    let mut cfg = spec();
    cfg.rate = 256 << 10;
    cfg.server_rate = 1 << 20;
    cfg.server_in_flight = 8 << 20;
    let mut s = server(cfg);
    let a = Subject::Identity(b"a".to_vec());
    let first = accept_with_list(&mut s, 1, &a, &data(2 << 20, 1));
    let second = accept_with_list(&mut s, 1, &a, &data(2 << 20, 2));
    let other = accept_with_list(
        &mut s,
        2,
        &Subject::Identity(b"b".to_vec()),
        &data(2 << 20, 3),
    );
    let granted = requests(&s.poll(0));
    let of = |id: u32| granted.iter().filter(|g| g.0 == id).count();
    // Connection 1 gets its two runs; the runs its own bucket refused leave the other connection the kind's other two.
    assert_eq!(of(first) + of(second), 2);
    assert_eq!(of(other), 2);
}

#[test]
fn an_upload_that_goes_quiet_is_ended_with_a_reason_and_its_data_on_the_way_is_not_a_violation() {
    let mut s = server(spec());
    let content = data(BLOCK_BYTES, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    assert_eq!(requests(&s.poll(0)), vec![(id, 0, 1)]);
    // Quiet for the idle time (10 s) it gives up its room; a further idle time later it ends.
    assert!(s.poll(9_000).is_empty());
    assert_eq!(s.in_flight(KIND), content.len() as u64);
    assert!(s.poll(10_000).is_empty());
    assert_eq!(s.in_flight(KIND), 0);
    assert!(s.poll(19_900).is_empty());
    let out = s.poll(20_000);
    assert_eq!(
        out,
        vec![Out::End {
            conn: ConnId(1),
            id,
            result: Err("The upload stalled.".to_string())
        }]
    );
    assert_eq!(s.sessions(), 0);
    assert_eq!(s.staged_memory(), 0);
    // The run it asked for was already on its way: it arrives after the end and is ignored, once, not a reason to drop the client.
    assert_eq!(
        s.on_message(20_100, ConnId(1), &run_data(&content, id, 0, 1)),
        Ok(None)
    );
    assert_eq!(s.counters().violations, 0);
    assert_eq!(
        s.on_message(20_100, ConnId(1), &run_data(&content, id, 0, 1))
            .unwrap_err(),
        Violation::UnknownUpload
    );
}

#[test]
fn every_way_an_upload_ends_leaves_room_for_the_data_on_its_way() {
    // Too long, and the server unable to store: the same tomb as a cancel.
    let mut cfg = spec();
    cfg.total = Duration::from_secs(5);
    let mut s = server(cfg);
    let content = data(BLOCK_BYTES * 4, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    assert_eq!(requests(&s.poll(0)).len(), 1);
    let out = s.poll(6_000);
    assert!(
        matches!(&out[..], [Out::End { result: Err(why), .. }] if why == "The upload took too long.")
    );
    assert_eq!(
        s.on_message(6_100, ConnId(1), &run_data(&content, id, 0, 4)),
        Ok(None)
    );
}

#[test]
fn an_upload_that_takes_too_long_is_ended() {
    let mut cfg = spec();
    cfg.rate = 1;
    cfg.idle = Duration::from_secs(1_000);
    let mut s = server(cfg);
    let content = data(BLOCK_BYTES * 2, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    let _ = s.poll(0);
    let out = s.poll(100_000);
    assert_eq!(
        out,
        vec![Out::End {
            conn: ConnId(1),
            id,
            result: Err("The upload took too long.".to_string())
        }]
    );
}

#[test]
fn waiting_for_the_servers_own_pace_is_not_idleness() {
    let mut cfg = spec();
    cfg.rate = 64 << 10;
    cfg.max_bytes = 2 << 20;
    cfg.idle = Duration::from_secs(5);
    cfg.total = Duration::from_secs(10_000);
    let mut s = server(cfg);
    let content = data(2 << 20, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    let mut now = 0;
    transfer(&mut s, 1, id, &content, &mut now);
    assert!(s.content(id).is_some());
}

// ---- resume

#[test]
fn a_disconnected_upload_resumes_with_only_the_missing_blocks() {
    let mut s = server(spec());
    let content = data(BLOCK_BYTES * 8, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    let asked = requests(&s.poll(0));
    assert_eq!(asked, vec![(id, 0, 4), (id, 4, 4)]);
    s.on_message(1, ConnId(1), &run_data(&content, id, 0, 4))
        .unwrap();
    s.disconnected(2, ConnId(1));
    // It waits, holding its bytes and quota.
    assert_eq!(s.sessions(), 1);
    assert_eq!(s.in_flight(KIND), content.len() as u64);
    assert_eq!(s.active(&me(), KIND), 0);
    let accepted = begin(&mut s, 2, &me(), &content).unwrap();
    assert!(accepted.resumed);
    assert!(accepted.has_list);
    assert_eq!(accepted.have, 4);
    assert_eq!(accepted.id, id);
    // Only blocks 4..8 are asked for, once.
    assert_eq!(requests(&s.poll(100)), vec![(id, 4, 4)]);
    assert_eq!(s.counters().resumed, 1);
}

#[test]
fn a_player_with_no_identity_does_not_resume_after_a_disconnect() {
    let mut s = server(spec());
    let content = data(BLOCK_BYTES * 2, 3);
    let who = Subject::Connection(ConnId(1));
    let id = accept_with_list(&mut s, 1, &who, &content);
    s.poll(0);
    s.disconnected(1, ConnId(1));
    assert_eq!(s.sessions(), 0);
    assert_eq!(s.staged_memory(), 0);
    let _ = id;
    let again = begin(&mut s, 2, &Subject::Connection(ConnId(2)), &content).unwrap();
    assert!(!again.resumed);
}

#[test]
fn an_upload_that_has_a_connection_is_not_resumed() {
    let mut s = server(spec());
    let content = data(BLOCK_BYTES * 8, 3);
    let who = Subject::Connection(ConnId(1));
    let id = accept_with_list(&mut s, 1, &who, &content);
    let asked = requests(&s.poll(0));
    assert_eq!(asked, vec![(id, 0, 4), (id, 4, 4)]);
    // The same connection, and another one of the same player: neither takes the upload under its feet.
    let e = begin(&mut s, 1, &who, &content).unwrap_err();
    assert!(
        matches!(e, Refusal::Invalid(why) if why.contains("already going")),
        "{e:?}"
    );
    let e = begin(&mut s, 2, &me(), &content);
    assert!(
        e.is_ok(),
        "another player's offer of the same bytes is its own upload"
    );
    // What the connection has in flight still arrives for what was asked.
    assert_eq!(
        s.on_message(1, ConnId(1), &run_data(&content, id, 0, 4)),
        Ok(None)
    );
    assert_eq!(
        s.on_message(1, ConnId(1), &run_data(&content, id, 4, 4)),
        Ok(None)
    );
    assert_eq!(s.counters().violations, 0);
}

#[test]
fn an_identity_does_not_take_an_upload_from_its_own_live_connection() {
    let mut s = server(spec());
    let content = data(BLOCK_BYTES * 8, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    let asked = requests(&s.poll(0));
    assert_eq!(asked.len(), 2);
    let e = begin(&mut s, 2, &me(), &content).unwrap_err();
    assert!(matches!(e, Refusal::Invalid(_)));
    // The first connection's answers are not a violation, so the honest player is not kicked.
    assert_eq!(
        s.on_message(1, ConnId(1), &run_data(&content, id, 0, 4)),
        Ok(None)
    );
    // Once the server sees that connection end, the upload resumes on the other.
    s.disconnected(2, ConnId(1));
    let again = begin(&mut s, 2, &me(), &content).unwrap();
    assert!(again.resumed && again.have == 4);
}

#[test]
fn a_waiting_upload_expires_after_the_resume_time() {
    let mut s = server(spec());
    let content = data(BLOCK_BYTES, 3);
    accept_with_list(&mut s, 1, &me(), &content);
    s.disconnected(0, ConnId(1));
    assert!(s.poll(59_000).is_empty());
    assert_eq!(s.sessions(), 1);
    assert!(s.poll(60_001).is_empty());
    assert_eq!(s.sessions(), 0);
    assert_eq!(s.staged_memory(), 0);
    assert_eq!(s.in_flight(KIND), 0);
}

#[test]
fn a_resume_counts_once_against_the_quota() {
    let mut cfg = spec();
    cfg.quota.files = 1;
    let mut s = server(cfg);
    let content = data(BLOCK_BYTES, 3);
    accept_with_list(&mut s, 1, &me(), &content);
    s.disconnected(1, ConnId(1));
    assert!(begin(&mut s, 2, &me(), &content).unwrap().resumed);
    let e = begin(&mut s, 2, &me(), &data(BLOCK_BYTES, 4)).unwrap_err();
    assert!(matches!(e, Refusal::QuotaFiles { .. }));
}

#[test]
fn a_resume_of_another_size_is_refused() {
    let mut s = server(spec());
    let content = data(BLOCK_BYTES, 3);
    accept_with_list(&mut s, 1, &me(), &content);
    s.disconnected(1, ConnId(1));
    let mut o = offer(&content, b"m").0;
    o.size += 1;
    let e = s.begin(2, ConnId(2), &me(), &o, |_| Ok(())).unwrap_err();
    assert!(matches!(e, Refusal::Invalid(_)));
}

// ---- staging and cleanup

#[test]
fn cancel_releases_everything() {
    let mut s = server(spec());
    let content = data(2000, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    assert_eq!(s.staged_memory(), 2000);
    s.cancel(0, ConnId(1), id);
    assert_eq!(
        (s.sessions(), s.staged_memory(), s.in_flight(KIND)),
        (0, 0, 0)
    );
    // Another connection cannot cancel what it does not own.
    let id = accept_with_list(&mut s, 1, &me(), &content);
    s.cancel(0, ConnId(2), id);
    assert_eq!(s.sessions(), 1);
}

#[test]
fn staging_memory_never_passes_its_cap() {
    let mut cfg = spec();
    cfg.concurrent = 16;
    cfg.quota.files = 100;
    cfg.quota.bytes = 1 << 30;
    cfg.server_in_flight = 1 << 30;
    let staging = StagingConfig {
        memory_per_upload: 1 << 20,
        memory_total: 3 << 20,
        ..StagingConfig::default()
    };
    let mut s = UploadServer::new(vec![(KIND.to_string(), cfg)], staging).unwrap();
    let mut refused = 0;
    for seed in 0..10u8 {
        if begin(&mut s, 1, &me(), &data(1 << 20, seed + 1)).is_err() {
            refused += 1;
        }
        assert!(s.staged_memory() <= 3 << 20);
    }
    // The fourth megabyte did not fit: with no folder to stage in, the rest are refused as busy.
    assert_eq!(refused, 7);
}

#[test]
fn the_sink_failing_ends_the_upload_with_its_reason() {
    let mut s = server(spec());
    let content = data(1000, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    let mut now = 0;
    transfer(&mut s, 1, id, &content, &mut now);
    let end = s.complete(id, Err("Not a build.".to_string())).unwrap();
    assert_eq!(
        end,
        Out::End {
            conn: ConnId(1),
            id,
            result: Err("Not a build.".to_string())
        }
    );
    assert_eq!(s.counters().aborted, 1);
    assert!(s.complete(id, Ok(())).is_none());
}

#[test]
fn a_request_the_host_could_not_send_is_asked_for_again() {
    let mut s = server(spec());
    let content = data(BLOCK_BYTES * 2, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    assert_eq!(requests(&s.poll(0)), vec![(id, 0, 2)]);
    s.retract(id, 0, 2);
    assert_eq!(requests(&s.poll(1)), vec![(id, 0, 2)]);
}

#[test]
fn a_spec_that_is_wrong_or_a_name_that_repeats_is_refused_when_the_server_is_made() {
    let mut bad = spec();
    bad.max_bytes = 0;
    let e = UploadServer::new(vec![("x".into(), bad)], StagingConfig::default()).err();
    assert!(e.unwrap().contains("max_bytes"));
    let e = UploadServer::new(
        vec![("x".into(), spec()), ("x".into(), spec())],
        StagingConfig::default(),
    )
    .err();
    assert!(e.unwrap().contains("two uploads"));
    assert!(UploadServer::new(vec![(String::new(), spec())], StagingConfig::default()).is_err());
}

#[test]
fn the_client_and_server_agree_on_what_an_accept_says() {
    let mut s = server(spec());
    let mut c = UploadClient::new();
    let begin = c
        .begin(KIND, b"m".to_vec(), Box::new(data(100, 3)))
        .unwrap();
    let offer = Offer {
        kind: &begin.kind,
        meta: &begin.meta,
        size: begin.size,
        hash: begin.hash,
    };
    let accepted = s.begin(0, ConnId(1), &me(), &offer, |_| Ok(())).unwrap();
    let messages = c.accepted(begin.handle, accepted.into()).unwrap();
    assert_eq!(messages.len(), 1);
    s.on_message(0, ConnId(1), &messages[0]).unwrap();
}

// ---- caps that waiting uploads must not escape

fn begin_at(
    server: &mut UploadServer,
    now: Millis,
    conn: u32,
    who: &Subject,
    content: &[u8],
) -> Result<Accepted, Refusal> {
    server.begin(now, ConnId(conn), who, &offer(content, b"m").0, |_| Ok(()))
}

#[test]
fn disconnecting_and_resuming_in_a_loop_cannot_keep_an_upload_alive_past_its_deadline() {
    let mut cfg = spec();
    cfg.total = Duration::from_secs(100);
    cfg.resume = Duration::from_secs(60);
    cfg.quota.files = 100;
    let mut s = server(cfg);
    let content = data(BLOCK_BYTES, 3);
    let list = |id| encode_block_list(id, &block_hashes(&content));
    let id = begin_at(&mut s, 0, 1, &me(), &content).unwrap().id;
    s.on_message(0, ConnId(1), &list(id)).unwrap();
    let (mut now, mut conn) = (0, 1);
    // Every 20 s the connection ends and the same upload is offered again; the deadline stays where the first accept put it.
    for _ in 0..4 {
        s.disconnected(now, ConnId(conn));
        now += 20_000;
        conn += 1;
        let again = begin_at(&mut s, now, conn, &me(), &content).unwrap();
        assert!(again.resumed && again.id == id, "{now}");
        assert!(again.has_list);
        s.poll(now);
    }
    assert_eq!(now, 80_000);
    // At 100 s the upload ends, whoever is attached to it.
    let out = s.poll(100_000);
    assert!(
        out.iter().any(
            |o| matches!(o, Out::End { result: Err(why), .. } if why == "The upload took too long.")
        ),
        "{out:?}"
    );
    assert_eq!(s.sessions(), 0);
    // An offer after the deadline starts afresh, before any poll has noticed it.
    let fresh = begin_at(&mut s, 100_000, conn, &me(), &content).unwrap();
    assert!(!fresh.resumed);
    s.disconnected(150_000, ConnId(conn));
    let late = begin_at(&mut s, 200_000, conn + 1, &me(), &content).unwrap();
    assert!(!late.resumed, "{fresh:?} {late:?}");
    assert_eq!(s.sessions(), 1);
}

#[test]
fn an_upload_may_be_resumed_only_so_many_times() {
    let mut cfg = spec();
    cfg.total = Duration::from_secs(1_000_000);
    cfg.resume = Duration::from_secs(1_000_000);
    cfg.idle = Duration::from_secs(1_000_000);
    cfg.quota.files = 100;
    let mut s = server(cfg);
    let content = data(BLOCK_BYTES, 3);
    begin_at(&mut s, 0, 1, &me(), &content).unwrap();
    let mut conn = 1;
    for k in 0..MAX_RESUMES {
        s.disconnected(u64::from(k), ConnId(conn));
        conn += 1;
        assert!(
            begin_at(&mut s, u64::from(k), conn, &me(), &content)
                .unwrap()
                .resumed
        );
    }
    s.disconnected(100, ConnId(conn));
    conn += 1;
    let e = begin_at(&mut s, 100, conn, &me(), &content).unwrap_err();
    assert!(
        matches!(e, Refusal::Invalid(why) if why.contains("too many times")),
        "{e:?}"
    );
    // It is gone: the next offer starts a new upload.
    assert_eq!(s.sessions(), 0);
    assert!(
        !begin_at(&mut s, 101, conn, &me(), &content)
            .unwrap()
            .resumed
    );
}

#[test]
fn waiting_uploads_count_toward_the_players_limit_and_the_oldest_gives_way() {
    let mut cfg = spec();
    cfg.concurrent = 2;
    cfg.quota.files = 100;
    let mut s = server(cfg);
    let (a, b, c) = (data(1000, 1), data(1000, 2), data(1000, 3));
    begin_at(&mut s, 0, 1, &me(), &a).unwrap();
    begin_at(&mut s, 1, 1, &me(), &b).unwrap();
    s.disconnected(10, ConnId(1));
    assert_eq!((s.sessions(), s.active(&me(), KIND)), (2, 0));
    // Two are waiting, so a third would be a third upload of the player's: the oldest waiting one is evicted for it.
    begin_at(&mut s, 20, 2, &me(), &c).unwrap();
    assert_eq!(s.sessions(), 2);
    assert_eq!(s.staged_memory(), 2000);
    assert_eq!(s.in_flight(KIND), 2000);
    // `a` was the one evicted: it starts afresh (and evicts `b`, now the oldest waiting one); `c` is not touched.
    assert!(!begin_at(&mut s, 30, 3, &me(), &a).unwrap().resumed);
    assert_eq!(s.sessions(), 2);
    // `c` is still there: once its connection ends, offering it again resumes it.
    s.disconnected(35, ConnId(2));
    assert!(begin_at(&mut s, 36, 4, &me(), &c).unwrap().resumed);
    // The limit still counts connected uploads first: two attached ones refuse a third.
    let e = begin_at(&mut s, 40, 3, &me(), &b).unwrap_err();
    assert!(matches!(e, Refusal::Concurrent { max: 2 }), "{e:?}");
}

#[test]
fn staged_bytes_per_player_stay_capped_and_waiting_uploads_give_way_for_them() {
    let mut cfg = spec();
    cfg.concurrent = 16;
    cfg.quota.files = 100;
    let staging = StagingConfig {
        memory_per_upload: 4 << 20,
        memory_total: 16 << 20,
        per_subject: 2_500,
        ..StagingConfig::default()
    };
    let mut s = UploadServer::new(vec![(KIND.to_string(), cfg)], staging).unwrap();
    let held = |s: &UploadServer| s.staged_memory();
    begin_at(&mut s, 0, 1, &me(), &data(1000, 1)).unwrap();
    begin_at(&mut s, 1, 1, &me(), &data(1000, 2)).unwrap();
    let e = begin_at(&mut s, 2, 1, &me(), &data(1000, 3)).unwrap_err();
    assert!(
        matches!(e, Refusal::HoldingTooMuch { limit: 2_500 }),
        "{e:?}"
    );
    assert!(e.to_string().contains("waiting on the server"));
    assert_eq!(held(&s), 2000);
    // Another player's bytes are not this player's.
    let you = Subject::Identity(b"you".to_vec());
    begin_at(&mut s, 3, 2, &you, &data(1000, 4)).unwrap();
    // Once the player's connection ends, what waits gives way, the oldest first, and the cap holds.
    s.disconnected(10, ConnId(1));
    begin_at(&mut s, 11, 3, &me(), &data(1000, 5)).unwrap();
    assert!(held(&s) <= 2_000 + 1_000);
    let mine: u64 = [2_000u64, 1_000].iter().sum();
    assert!(mine <= 3_000);
    assert_eq!(s.sessions(), 3);
    // A single upload above the cap can never fit, however much waits.
    let e = begin_at(&mut s, 12, 3, &me(), &data(2_501, 6)).unwrap_err();
    assert!(matches!(e, Refusal::HoldingTooMuch { .. }), "{e:?}");
    assert_eq!(s.sessions(), 3);
}

#[test]
fn bytes_that_keep_arriving_keep_a_slow_upload_alive_whatever_the_specs_rate_is() {
    // The spec's rate is 8 MiB a second, the link delivers 2 KiB a second: each byte buys the time a 1 KiB/s link needs for it, so
    // the upload lives as long as the link delivers at least that, and a 64 KiB run (half a minute) is not a stall.
    let mut s = server(spec());
    let content = data(BLOCK_BYTES, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    assert_eq!(requests(&s.poll(0)).len(), 1);
    let mut now = 0;
    for _ in 0..40 {
        now += 1_000;
        s.delivered(now, ConnId(1), 2_048);
        assert!(s.poll(now).is_empty(), "ended at {now}");
        assert_eq!(
            s.in_flight(KIND),
            content.len() as u64,
            "gave up its room at {now}"
        );
    }
    // The run arrives at last.
    assert_eq!(
        s.on_message(now, ConnId(1), &run_data(&content, id, 0, 1)),
        Ok(None)
    );
}

#[test]
fn a_trickle_does_not_keep_an_upload_alive() {
    let mut s = server(spec());
    let content = data(BLOCK_BYTES, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    assert_eq!(requests(&s.poll(0)).len(), 1);
    let mut ended = None;
    for step in 1..=60 {
        let now = step * 1_000;
        // A byte a second is a thousandth of the slowest link there is.
        s.delivered(now, ConnId(1), 1);
        if s.poll(now).iter().any(|o| matches!(o, Out::End { .. })) {
            ended = Some(now);
            break;
        }
    }
    let at = ended.expect("a trickle is a stall");
    assert!((19_000..=21_000).contains(&at), "ended at {at}");
    assert_eq!(s.sessions(), 0);
    let _ = id;
}

#[test]
fn what_a_connection_delivers_does_not_wake_an_upload_that_waits_for_nothing() {
    // An upload with nothing asked for (waiting on the pace) is not helped by, or hurt by, the bytes of its connection.
    let mut cfg = spec();
    cfg.rate = 8 << 10;
    let mut s = server(cfg);
    let content = data(BLOCK_BYTES * 8, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    for out in s.poll(0) {
        if let Out::Request {
            id, start, count, ..
        } = out
        {
            s.on_message(0, ConnId(1), &run_data(&content, id, start, count))
                .unwrap();
        }
    }
    s.delivered(1_000, ConnId(1), 1 << 20);
    assert_eq!(s.in_flight(KIND), content.len() as u64);
    let _ = id;
}

#[test]
fn an_upload_past_its_idle_time_gives_up_its_room_and_takes_it_back_only_if_it_is_free() {
    let mut cfg = spec();
    cfg.rate = 1 << 10;
    cfg.max_bytes = 2 << 20;
    cfg.server_in_flight = 2 << 20;
    cfg.concurrent = 2;
    cfg.quota.files = 10;
    cfg.quota.bytes = 64 << 20;
    let a_content = data(BLOCK_BYTES * 4, 3);
    let who_b = Subject::Identity(b"b".to_vec());
    // Quiet for 10 s, it no longer counts against the room, and nobody takes it: its data brings it back.
    let mut s = server(cfg.clone());
    let a = accept_with_list(&mut s, 1, &me(), &a_content);
    assert_eq!(requests(&s.poll(0)), vec![(a, 0, 4)]);
    assert_eq!(s.in_flight(KIND), a_content.len() as u64);
    assert!(s.poll(12_000).is_empty());
    assert_eq!(s.in_flight(KIND), 0, "the room is given up");
    assert_eq!(s.sessions(), 1, "the upload itself goes on");
    let back = s.on_message(13_000, ConnId(1), &run_data(&a_content, a, 0, 4));
    assert_eq!(back, Ok(None));
    assert_eq!(s.in_flight(KIND), a_content.len() as u64);
    // The same, but another player took the room meanwhile: the late data ends the upload instead of passing the cap.
    let mut s = server(cfg);
    let a = accept_with_list(&mut s, 1, &me(), &a_content);
    s.poll(0);
    assert!(s.poll(12_000).is_empty());
    begin_at(&mut s, 12_000, 2, &who_b, &data(2 << 20, 9)).unwrap();
    let out = s
        .on_message(13_000, ConnId(1), &run_data(&a_content, a, 0, 4))
        .unwrap();
    assert_eq!(
        out,
        Some(Out::End {
            conn: ConnId(1),
            id: a,
            result: Err("The server needed the room for other uploads.".to_string())
        })
    );
    assert!(s.in_flight(KIND) <= 2 << 20);
    assert_eq!(s.sessions(), 1);
}

#[test]
fn an_upload_that_went_quiet_is_offered_runs_after_the_others() {
    let mut cfg = spec();
    cfg.rate = 1 << 10;
    cfg.concurrent = 2;
    let mut s = server(cfg);
    let a = accept_with_list(&mut s, 1, &me(), &data(BLOCK_BYTES * 12, 1));
    s.poll(0);
    let b = accept_with_list(
        &mut s,
        2,
        &Subject::Identity(b"b".to_vec()),
        &data(BLOCK_BYTES * 12, 2),
    );
    let before = s.grant_order();
    // Round robin from the last one served: `b` is first, then `a`.
    assert_eq!(before, vec![b, a]);
    // Once `a` has been quiet past `idle` it goes last, even where it would come first.
    s.cursor = b;
    assert_eq!(s.grant_order(), vec![a, b]);
    s.poll(10_500);
    assert_eq!(s.grant_order(), vec![b, a]);
}

#[test]
fn an_upload_waiting_only_on_the_servers_pace_is_never_quiet() {
    let mut cfg = spec();
    cfg.rate = 8 << 10;
    cfg.idle = Duration::from_secs(5);
    cfg.total = Duration::from_secs(1_000_000);
    cfg.max_bytes = 2 << 20;
    let mut s = server(cfg);
    let content = data(2 << 20, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    let mut now = 0;
    // Two runs go out and arrive at once; the next must wait half a minute for the rate, with nothing asked for meanwhile.
    for out in s.poll(0) {
        if let Out::Request {
            id, start, count, ..
        } = out
        {
            s.on_message(now, ConnId(1), &run_data(&content, id, start, count))
                .unwrap();
        }
    }
    for _ in 0..100 {
        now += 1_000;
        let outs = s.poll(now);
        assert!(
            outs.iter().all(|o| !matches!(o, Out::End { .. })),
            "ended at {now}"
        );
        // The client answers as soon as it is asked: it is never the one that is quiet.
        for out in outs {
            if let Out::Request {
                id, start, count, ..
            } = out
            {
                s.on_message(now, ConnId(1), &run_data(&content, id, start, count))
                    .unwrap();
            }
        }
        assert_eq!(
            s.in_flight(KIND),
            content.len() as u64,
            "gave up its room at {now}"
        );
    }
    let _ = id;
}

#[test]
fn a_client_that_breaks_the_rules_loses_every_upload_it_had_and_cannot_resume_them() {
    let mut cfg = spec();
    cfg.concurrent = 3;
    cfg.quota.files = 10;
    let mut s = server(cfg);
    let (a, b) = (data(BLOCK_BYTES * 2, 1), data(BLOCK_BYTES * 2, 2));
    let id_a = accept_with_list(&mut s, 1, &me(), &a);
    accept_with_list(&mut s, 1, &me(), &b);
    assert_eq!(s.sessions(), 2);
    // It sends data nobody asked for (on top of the one upload it was asked about).
    let v = s
        .on_message(0, ConnId(1), &run_data(&a, id_a, 0, 1))
        .unwrap_err();
    assert_eq!(v, Violation::Unrequested);
    // Both uploads are gone with the connection, and what they held is released: nothing waits for a resume.
    assert_eq!(s.sessions(), 0);
    assert_eq!(s.in_flight(KIND), 0);
    assert_eq!(s.staged_memory(), 0);
    s.disconnected(1, ConnId(1));
    assert!(!begin_at(&mut s, 2, 2, &me(), &a).unwrap().resumed);
}

#[test]
fn a_content_that_does_not_match_its_hash_also_ends_the_clients_other_uploads() {
    let mut cfg = spec();
    cfg.concurrent = 2;
    let mut s = server(cfg);
    let truth = data(1000, 3);
    let lie = data(1000, 5);
    let other = data(BLOCK_BYTES, 9);
    accept_with_list(&mut s, 1, &me(), &other);
    let o = offer(&truth, b"m").0;
    let accepted = s.begin(0, ConnId(1), &me(), &o, |_| Ok(())).unwrap();
    s.on_message(
        0,
        ConnId(1),
        &encode_block_list(accepted.id, &block_hashes(&lie)),
    )
    .unwrap();
    let mut dropped = false;
    for step in 1..50 {
        for out in s.poll(step * 32) {
            match out {
                Out::Request {
                    id, start, count, ..
                } if id == accepted.id => {
                    s.on_message(step * 32, ConnId(1), &run_data(&lie, id, start, count))
                        .unwrap();
                }
                Out::Drop { .. } => dropped = true,
                _ => {}
            }
        }
    }
    assert!(dropped);
    assert_eq!(s.sessions(), 0, "the other upload did not stay behind");
}

#[test]
fn players_with_no_identity_leave_no_records_when_their_connection_ends() {
    let mut cfg = spec();
    cfg.quota.files = 5;
    let mut s = server(cfg);
    for conn in 1..=2_000u32 {
        let who = Subject::Connection(ConnId(conn));
        begin_at(
            &mut s,
            u64::from(conn),
            conn,
            &who,
            &data(300, (conn % 200) as u8 + 1),
        )
        .unwrap();
        s.disconnected(u64::from(conn), ConnId(conn));
        assert!(
            s.players() <= 1,
            "{} players after {conn} connections",
            s.players()
        );
    }
    assert_eq!(s.players(), 0);
    assert_eq!(s.sessions(), 0);
    // A player with an identity keeps its records for the window, and then they go.
    begin_at(&mut s, 0, 5_000, &me(), &data(300, 7)).unwrap();
    s.disconnected(1, ConnId(5_000));
    assert_eq!(s.players(), 1);
    s.poll(3_600_000 + 100_000);
    assert_eq!(s.players(), 0);
}

#[test]
fn a_request_the_host_could_not_send_is_neither_a_sign_of_life_nor_a_cost() {
    let mut cfg = spec();
    cfg.rate = 256 << 10;
    cfg.server_rate = 256 << 10;
    let mut s = server(cfg);
    let content = data(BLOCK_BYTES * 12, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    let before = s.sessions[&id].alive_until;
    // Two runs are granted at 9 s, the second poll's tokens being the first's refunded; the host cannot send either.
    let first = requests(&s.poll(9_000));
    assert_eq!(first, vec![(id, 0, 4), (id, 4, 4)]);
    assert_ne!(s.sessions[&id].alive_until, before);
    for (id, start, count) in first {
        s.retract(id, start, count);
    }
    assert_eq!(s.sessions[&id].alive_until, before, "the clock went back");
    // The tokens came back too: the same two runs are asked for again at once.
    assert_eq!(requests(&s.poll(9_001)).len(), 2);
}

#[test]
fn an_upload_waiting_for_a_resume_gives_up_its_room_and_needs_it_back_to_resume() {
    let mut cfg = spec();
    cfg.max_bytes = 2 << 20;
    cfg.server_in_flight = 2 << 20;
    cfg.quota.files = 10;
    cfg.quota.bytes = 64 << 20;
    cfg.resume = Duration::from_secs(60);
    let a = data(1 << 20, 3);
    let big = data(2 << 20, 9);
    let mut s = server(cfg);
    begin_at(&mut s, 0, 1, &me(), &a).unwrap();
    s.disconnected(0, ConnId(1));
    assert_eq!(s.in_flight(KIND), a.len() as u64);
    // Waiting for the idle time (10 s), it stops counting against the kind's room, though its staging is still held.
    s.poll(9_000);
    assert_eq!(s.in_flight(KIND), a.len() as u64);
    s.poll(10_000);
    assert_eq!(s.in_flight(KIND), 0);
    assert_eq!(s.staged_memory(), a.len());
    // Another player takes the room: the waiting upload can no longer resume until there is room.
    let you = Subject::Identity(b"you".to_vec());
    begin_at(&mut s, 11_000, 2, &you, &big).unwrap();
    let e = begin_at(&mut s, 12_000, 3, &me(), &a).unwrap_err();
    assert_eq!(e, Refusal::ServerFull);
    assert!(s.in_flight("build") <= 2 << 20);
    s.disconnected(13_000, ConnId(2));
    s.poll(23_000);
    // `you` waits too and gave up its room: now the first resumes and takes its room back.
    let again = begin_at(&mut s, 23_001, 3, &me(), &a).unwrap();
    assert!(again.resumed);
    assert_eq!(s.in_flight(KIND), a.len() as u64);
}

#[test]
fn the_kinds_own_rules_are_asked_before_anything_is_evicted_or_staged() {
    let mut cfg = spec();
    cfg.concurrent = 1;
    cfg.quota.files = 10;
    let mut s = server(cfg);
    let (a, b) = (data(1000, 1), data(1000, 2));
    begin_at(&mut s, 0, 1, &me(), &a).unwrap();
    s.disconnected(1, ConnId(1));
    assert_eq!((s.sessions(), s.staged_memory()), (1, 1000));
    // The new offer would have evicted the waiting upload; the rules refuse it, and the waiting upload stays.
    let o = offer(&b, b"m").0;
    let e = s
        .begin(2, ConnId(2), &me(), &o, |_| Err("No.".to_string()))
        .unwrap_err();
    assert_eq!(e, Refusal::Rejected("No.".to_string()));
    assert_eq!((s.sessions(), s.staged_memory()), (1, 1000));
    assert!(begin_at(&mut s, 3, 3, &me(), &a).unwrap().resumed);
}

#[test]
fn what_a_connection_delivers_is_capped_at_what_its_uploads_wait_for() {
    let mut s = server(spec());
    // A hundred bytes are all the upload waits for; a megabyte delivered does not buy more than those hundred.
    let content = data(100, 3);
    let id = accept_with_list(&mut s, 1, &me(), &content);
    assert_eq!(requests(&s.poll(0)).len(), 1);
    let before = s.sessions[&id].alive_until;
    s.delivered(1_000, ConnId(1), 1 << 20);
    let gained = s.sessions[&id].alive_until - before;
    assert!(gained <= 100 * 1_000 / 1_024, "gained {gained} ms");
}

#[test]
fn what_a_connection_delivers_is_shared_among_its_waiting_uploads() {
    let mut cfg = spec();
    cfg.concurrent = 2;
    let mut s = server(cfg);
    let (a, b) = (data(BLOCK_BYTES, 1), data(BLOCK_BYTES, 2));
    let id_a = accept_with_list(&mut s, 1, &me(), &a);
    let id_b = accept_with_list(&mut s, 1, &me(), &b);
    assert_eq!(requests(&s.poll(0)).len(), 2);
    let before = (s.sessions[&id_a].alive_until, s.sessions[&id_b].alive_until);
    // 2 KiB is two seconds of a 1 KiB/s link, whether one upload gets them or two: here a second each.
    s.delivered(1_000, ConnId(1), 2_048);
    assert_eq!(s.sessions[&id_a].alive_until - before.0, 1_000);
    assert_eq!(s.sessions[&id_b].alive_until - before.1, 1_000);
    // An upload of another connection is not credited with them.
    let you = Subject::Identity(b"you".to_vec());
    let other = accept_with_list(&mut s, 2, &you, &data(BLOCK_BYTES, 5));
    s.poll(1_000);
    let was = s.sessions[&other].alive_until;
    s.delivered(1_500, ConnId(1), 2_048);
    assert_eq!(s.sessions[&other].alive_until, was);
}
