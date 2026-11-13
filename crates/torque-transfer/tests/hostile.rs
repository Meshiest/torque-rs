//! A hostile client, and a hostile server. `TORQUE_FUZZ_UPLOAD=100000` runs that many operations: begins with lies in every field,
//! block lists and data that are valid, mutated or random, answers to requests that are right or wrong, cancels, disconnects and
//! jumps in time. After every operation the caps must hold (staging memory and disk, the staged bytes of a kind, the sessions, a
//! player's uploads at once), nothing may panic, and in the end nothing may be left held.

use crate::common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use common::{TempFolder, content};
use torque_net::handshake::ConnId;
use torque_transfer::fuzz::{Rng, cases_from_env, mutate};
use torque_transfer::{
    Accept, BLOCK_BYTES, ClientError, MAX_SESSIONS, Millis, Offer, Out, Quota, StagingConfig,
    Subject, UploadClient, UploadServer, UploadSpec, block_hashes, content_hash, decode,
    encode_block_list, encode_data, run_bytes,
};

const KINDS: [&str; 2] = ["build", "image"];
const MEMORY_TOTAL: usize = 512 << 10;
const DISK_TOTAL: u64 = 6 << 20;
const PER_SUBJECT: u64 = 3 << 20;

fn specs() -> Vec<(String, UploadSpec)> {
    let build = UploadSpec {
        max_bytes: 2 << 20,
        quota: Quota {
            bytes: 6 << 20,
            per: Duration::from_secs(60),
            files: 8,
        },
        rate: 1 << 20,
        concurrent: 2,
        server_in_flight: 4 << 20,
        server_rate: 2 << 20,
        idle: Duration::from_secs(5),
        total: Duration::from_secs(60),
        resume: Duration::from_secs(20),
        meta_max: 8,
        permission: None,
    };
    let image = UploadSpec {
        max_bytes: 300 << 10,
        concurrent: 1,
        server_in_flight: 600 << 10,
        ..build.clone()
    };
    vec![(KINDS[0].to_string(), build), (KINDS[1].to_string(), image)]
}

struct Conn {
    id: ConnId,
    subject: Subject,
    /// Runs the server asked this connection for, not yet answered.
    asked: Vec<(u32, u32, u32)>,
    /// Upload ids this connection was told, and the content each was offered for.
    ids: Vec<(u32, usize)>,
}

struct Hostile {
    server: UploadServer,
    folder: TempFolder,
    rng: Rng,
    now: Millis,
    conns: Vec<Conn>,
    next_conn: u32,
    contents: Vec<Vec<u8>>,
    peak_memory: usize,
    peak_disk: u64,
    sinks: u64,
}

impl Hostile {
    fn new(seed: u64) -> Hostile {
        let folder = TempFolder::new(&format!("hostile-{seed}"));
        let staging = StagingConfig {
            memory_per_upload: 256 << 10,
            memory_total: MEMORY_TOTAL,
            disk_total: DISK_TOTAL,
            per_subject: PER_SUBJECT,
            folder: Some(folder.0.clone()),
        };
        let mut rng = Rng::new(seed);
        let contents = [100, 50_000, BLOCK_BYTES, 200_000, 700_000, 1_500_000]
            .iter()
            .map(|&n| content(n, rng.next_u64()))
            .collect();
        let mut h = Hostile {
            server: UploadServer::new(specs(), staging).unwrap(),
            folder,
            rng,
            now: 0,
            conns: Vec::new(),
            next_conn: 1,
            contents,
            peak_memory: 0,
            peak_disk: 0,
            sinks: 0,
        };
        for _ in 0..5 {
            h.connect();
        }
        h
    }

    fn connect(&mut self) {
        let id = ConnId(self.next_conn);
        self.next_conn += 1;
        let subject = match self.rng.below(4) {
            0 => Subject::Connection(id),
            n => Subject::Identity(vec![n as u8]),
        };
        self.conns.push(Conn {
            id,
            subject,
            asked: Vec::new(),
            ids: Vec::new(),
        });
    }

    fn conn_index(&mut self) -> usize {
        self.rng.below(self.conns.len())
    }

    fn disconnect(&mut self, index: usize) {
        let conn = self.conns.swap_remove(index);
        self.server.disconnected(self.now, conn.id);
        self.connect();
    }

    fn pick_content(&mut self) -> usize {
        self.rng.below(self.contents.len())
    }

    fn some_id(&mut self, index: usize) -> u32 {
        let known = &self.conns[index].ids;
        if !known.is_empty() && self.rng.chance(85) {
            known[self.rng.below(known.len())].0
        } else {
            self.rng.next_u64() as u32
        }
    }

    fn begin(&mut self) {
        let index = self.conn_index();
        let which = self.pick_content();
        let bytes = &self.contents[which];
        let kind = if self.rng.chance(95) {
            KINDS[self.rng.below(2)]
        } else {
            "nonsense"
        };
        let mut size = bytes.len() as u64;
        let mut hash = content_hash(bytes);
        match self.rng.below(10) {
            0 => size = 0,
            1 => size = u64::MAX,
            2 => size = size.wrapping_add(self.rng.below(3) as u64),
            3 => hash = [self.rng.next_u64() as u8; 32],
            _ => {}
        }
        let meta = self.rng.some_bytes(12);
        let offer = Offer {
            kind,
            meta: &meta,
            size,
            hash,
        };
        let reject = self.rng.chance(5);
        let conn = &self.conns[index];
        let result = self
            .server
            .begin(self.now, conn.id, &conn.subject, &offer, |_| {
                if reject {
                    Err("no".to_string())
                } else {
                    Ok(())
                }
            });
        if let Ok(accepted) = result {
            self.conns[index].ids.push((accepted.id, which));
            // The client usually follows with the block list.
            if self.rng.chance(80) {
                let list = encode_block_list(accepted.id, &block_hashes(&self.contents[which]));
                self.send(index, &list);
            }
        }
    }

    fn send(&mut self, index: usize, message: &[u8]) {
        let id = self.conns[index].id;
        match self.server.on_message(self.now, id, message) {
            Ok(None) => {}
            Ok(Some(out)) => self.apply(out),
            Err(_) => self.disconnect(index),
        }
    }

    fn block_list(&mut self) {
        let index = self.conn_index();
        let id = self.some_id(index);
        let which = self.pick_content();
        let mut list = encode_block_list(id, &block_hashes(&self.contents[which]));
        match self.rng.below(4) {
            0 => list = mutate(&list, &mut self.rng),
            1 => list.truncate(self.rng.below(list.len())),
            _ => {}
        }
        self.send(index, &list);
    }

    fn data(&mut self) {
        let index = self.conn_index();
        let message = if let Some(&(id, start, count)) = self.conns[index].asked.first() {
            self.conns[index].asked.remove(0);
            let which = self.content_of(index, id);
            let bytes = &self.contents[which];
            let (from, to) = run_bytes(bytes.len() as u64, u64::from(start), u64::from(count));
            let honest = encode_data(id, start, &bytes[from as usize..to as usize]);
            match self.rng.below(20) {
                0 => mutate(&honest, &mut self.rng),
                1 => encode_data(id, start + 1, &bytes[from as usize..to as usize]),
                2 => encode_data(
                    id,
                    start,
                    &bytes[from as usize..(to as usize).saturating_sub(1).max(1)],
                ),
                3 => encode_data(
                    id.wrapping_add(1),
                    start,
                    &bytes[from as usize..to as usize],
                ),
                _ => honest,
            }
        } else {
            let id = self.some_id(index);
            let n = 1 + self.rng.below(2 * BLOCK_BYTES);
            let bytes = self.rng.bytes(n);
            encode_data(id, self.rng.below(8) as u32, &bytes)
        };
        self.send(index, &message);
    }

    /// The content upload `id` of connection `index` was offered for.
    fn content_of(&mut self, index: usize, id: u32) -> usize {
        match self.conns[index].ids.iter().find(|(known, _)| *known == id) {
            Some(&(_, which)) => which,
            None => self.pick_content(),
        }
    }

    fn noise(&mut self) {
        let index = self.conn_index();
        let n = self.rng.below(64);
        let bytes = self.rng.bytes(n);
        let message = if self.rng.chance(50) {
            let id = self.some_id(index);
            let mut m = vec![1 + self.rng.below(3) as u8];
            m.extend_from_slice(&id.to_le_bytes());
            m.extend(bytes);
            m
        } else {
            bytes
        };
        let _ = decode(&message);
        self.send(index, &message);
    }

    fn cancel(&mut self) {
        let index = self.conn_index();
        let id = self.some_id(index);
        let conn = self.conns[index].id;
        self.server.cancel(self.now, conn, id);
    }

    fn poll(&mut self, advance: u64) {
        self.now += advance;
        let outs = self.server.poll(self.now);
        for out in outs {
            self.apply(out);
        }
    }

    fn apply(&mut self, out: Out) {
        match out {
            Out::Request {
                conn,
                id,
                start,
                count,
            } => {
                if let Some(c) = self.conns.iter_mut().find(|c| c.id == conn) {
                    c.asked.push((id, start, count));
                }
            }
            Out::Finish { id } => {
                if let Some(view) = self.server.content(id) {
                    let _ = view.to_vec(usize::MAX);
                }
                self.sinks += 1;
                let result = if self.rng.chance(80) {
                    Ok(())
                } else {
                    Err("bad".to_string())
                };
                self.server.complete(id, result);
            }
            Out::Drop { conn } => {
                if let Some(index) = self.conns.iter().position(|c| c.id == conn) {
                    self.disconnect(index);
                }
            }
            _ => {}
        }
    }

    fn step(&mut self) {
        match self.rng.below(100) {
            0..=19 => self.begin(),
            20..=26 => self.block_list(),
            27..=64 => self.data(),
            65..=67 => self.noise(),
            68..=69 => self.cancel(),
            70..=71 => {
                let index = self.conn_index();
                let (conn, bytes) = (self.conns[index].id, self.rng.below(5_000) as u64);
                self.server.delivered(self.now, conn, bytes);
            }
            72..=74 => {
                let index = self.conn_index();
                self.disconnect(index);
            }
            75..=94 => {
                let advance = self.rng.below(400) as u64;
                self.poll(advance);
            }
            _ => {
                let advance = self.rng.below(30_000) as u64;
                self.poll(advance);
            }
        }
    }

    /// What must hold after every operation.
    fn check(&mut self) {
        let memory = self.server.staged_memory();
        let disk = self.server.staged_disk();
        self.peak_memory = self.peak_memory.max(memory);
        self.peak_disk = self.peak_disk.max(disk);
        assert!(memory <= MEMORY_TOTAL, "staging memory {memory}");
        assert!(disk <= DISK_TOTAL, "staging disk {disk}");
        for (name, spec) in specs() {
            assert!(
                self.server.in_flight(&name) <= spec.server_in_flight,
                "{name} in flight"
            );
            for conn in &self.conns {
                let staged = self.server.staged_for(&conn.subject);
                assert!(staged <= PER_SUBJECT, "a player holds {staged} bytes");
                assert!(
                    self.server.active(&conn.subject, &name) <= spec.concurrent as usize,
                    "{name} at once"
                );
            }
        }
        assert!(self.server.sessions() <= MAX_SESSIONS);
    }

    /// Everything ends: nothing may be held, and the staging folder is empty.
    fn finish(&mut self) {
        while let Some(conn) = self.conns.pop() {
            self.server.disconnected(self.now, conn.id);
        }
        self.now += 10_000_000;
        self.poll(0);
        self.poll(100);
        assert_eq!(self.server.sessions(), 0, "sessions left");
        assert_eq!(self.server.staged_memory(), 0);
        assert_eq!(self.server.staged_disk(), 0);
        for (name, _) in specs() {
            assert_eq!(self.server.in_flight(&name), 0);
        }
        assert_eq!(
            std::fs::read_dir(&self.folder.0).unwrap().count(),
            0,
            "staging files left"
        );
    }
}

#[test]
fn a_hostile_client_gets_no_panic_and_staging_stays_inside_its_caps() {
    let operations = cases_from_env("TORQUE_FUZZ_UPLOAD", 4_000);
    let mut panics = 0;
    for seed in 1..=4u64 {
        let mut h = Hostile::new(seed);
        for op in 0..operations / 4 {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                h.step();
                h.check();
                if op % 200 == 0 {
                    let files = std::fs::read_dir(&h.folder.0).unwrap().count();
                    assert!(
                        files <= h.server.sessions(),
                        "{files} staging files for {} sessions",
                        h.server.sessions()
                    );
                }
            }));
            if outcome.is_err() {
                panics += 1;
                eprintln!("seed {seed} operation {op} panicked");
                break;
            }
        }
        h.finish();
        // The run reached what it is meant to: uploads that finished, and the caps' edges.
        assert!(
            h.sinks > 0 || operations < 1_000,
            "no upload ever finished (seed {seed})"
        );
        println!(
            "seed {seed}: {} uploads reached the sink, {} accepted, {} refused, {} resumed, {} violations, peak memory {} / {MEMORY_TOTAL}, peak disk {} / {DISK_TOTAL}",
            h.sinks,
            h.server.counters().accepted,
            h.server.counters().refused,
            h.server.counters().resumed,
            h.server.counters().violations,
            h.peak_memory,
            h.peak_disk
        );
    }
    assert_eq!(panics, 0);
}

#[test]
fn a_hostile_server_gets_no_panic_from_the_client() {
    let operations = cases_from_env("TORQUE_FUZZ_UPLOAD", 4_000);
    let mut rng = Rng::new(77);
    let mut client = UploadClient::new();
    let mut handles = Vec::new();
    let mut panics = 0;
    for op in 0..operations {
        let outcome = catch_unwind(AssertUnwindSafe(|| match rng.below(8) {
            0 => {
                let n = rng.below(3 * BLOCK_BYTES);
                let kind = if rng.chance(90) { "build" } else { "" };
                let source = Box::new(rng.bytes(n));
                if let Ok(begin) = client.begin(kind, rng.some_bytes(40), source) {
                    handles.push(begin.handle);
                }
            }
            1 | 2 => {
                if let Some(&handle) = handles.get(rng.below(handles.len().max(1))) {
                    let accept = Accept {
                        id: rng.below(6) as u32,
                        has_list: rng.chance(50),
                        have: rng.next_u64() as u32,
                    };
                    let _ = client.accepted(handle, accept);
                }
            }
            3..=5 => {
                let edge = [0, 1, 4, 5, u32::MAX, 1 << 31];
                let pick = |rng: &mut Rng| {
                    if rng.chance(50) {
                        edge[rng.below(edge.len())]
                    } else {
                        rng.below(8) as u32
                    }
                };
                let (id, start, count) = (rng.below(6) as u32, pick(&mut rng), pick(&mut rng));
                match client.request(id, start, count) {
                    Ok(_)
                    | Err(
                        ClientError::Unknown | ClientError::Protocol(_) | ClientError::Source(_),
                    ) => {}
                    Err(other) => panic!("unexpected {other:?}"),
                }
            }
            6 => {
                let id = rng.below(6) as u32;
                client.ended(
                    id,
                    if rng.chance(50) {
                        Ok(())
                    } else {
                        Err("x".into())
                    },
                );
            }
            _ => {
                if rng.chance(20) {
                    client.disconnected();
                }
                if let Some(&handle) = handles.get(rng.below(handles.len().max(1))) {
                    client.refused(handle, "no".to_string());
                    let _ = client.status(handle);
                }
            }
        }));
        if outcome.is_err() {
            panics += 1;
            eprintln!("operation {op} panicked");
            break;
        }
    }
    assert_eq!(panics, 0);
}
