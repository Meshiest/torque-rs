//! The server side of ghosting over a toy world: 64 managers under Torque's policy, each tick marking `dirty` objects of every
//! client's 1,024 ghosts, writing one packet per client and acknowledging it. Reported apart: `set_mask` (the dirty list's
//! collapse), `write_section` and `on_notify`. The toy source costs a map lookup a call, so these are lower bounds for a host
//! whose source does more per object.

use std::collections::BTreeSet;
use std::hint::black_box;
use std::time::{Duration, Instant};
use torque_testkit::Rng;

use criterion::{Criterion, criterion_group, criterion_main};
use torque_net::bits::BitWriter;
use torque_net::events::ConnectionMessage;
use torque_net::ghost::{
    CamInfo, GhostConfig, GhostManager, NoScripts, ObjId, ObjectSource, SectionBudget, net_flags,
};

const CLIENTS: usize = 64;
const OBJECTS: u32 = 1_024;
const PACKET_BYTES: usize = 1_000;

struct Toy {
    objects: BTreeSet<ObjId>,
    ids: Vec<ObjId>,
    always: Vec<ObjId>,
}

impl ObjectSource for Toy {
    fn exists(&self, obj: ObjId) -> bool {
        self.objects.contains(&obj)
    }

    fn class_id(&self, obj: ObjId) -> Option<u32> {
        self.objects.contains(&obj).then_some(5)
    }

    fn net_flags(&self, obj: ObjId) -> u32 {
        if self.objects.contains(&obj) {
            net_flags::GHOSTABLE
        } else {
            0
        }
    }

    fn pack_update(
        &mut self,
        _obj: ObjId,
        mask: u32,
        w: &mut BitWriter,
        _ghost_of: &dyn Fn(ObjId) -> Option<u32>,
    ) -> u32 {
        for b in 0..8 {
            let set = mask & (1 << b) != 0;
            w.write_flag(set);
            if set {
                w.write_bits(7, 8);
            }
        }
        0
    }

    fn update_priority(&self, _obj: ObjId, _cam: &CamInfo, _mask: u32, skip: u8) -> f32 {
        f32::from(skip) * 0.1
    }

    fn camera_info(&self, _scope: ObjId) -> Option<CamInfo> {
        Some(CamInfo::default())
    }

    fn sky_visible_distance(&self) -> Option<f32> {
        None
    }

    fn mount_of(&self, _scope: ObjId) -> Option<ObjId> {
        None
    }

    fn scope_query(
        &self,
        _scope: ObjId,
        _cam: &CamInfo,
        _radius: f32,
        visit: &mut dyn FnMut(ObjId) -> bool,
    ) {
        for &id in &self.ids {
            if !visit(id) {
                return;
            }
        }
    }

    fn scope_distance(&self, _obj: ObjId, _scope: ObjId) -> Option<f32> {
        None
    }

    fn ghost_always(&self) -> &[ObjId] {
        &self.always
    }
}

/// Every phase's time over the ticks run.
#[derive(Default, Clone, Copy)]
struct Phases {
    set_mask: Duration,
    write: Duration,
    notify: Duration,
}

struct World {
    src: Toy,
    managers: Vec<GhostManager>,
    seq: u32,
    now: u32,
    rng: Rng,
    phases: Phases,
    records: usize,
}

impl World {
    /// 64 clients that have every ghost, acknowledged.
    fn settled() -> World {
        let mut src = Toy {
            objects: BTreeSet::new(),
            ids: Vec::new(),
            always: Vec::new(),
        };
        for id in 1..=OBJECTS + 1 {
            src.objects.insert(id);
            src.ids.push(id);
        }
        let managers = (0..CLIENTS)
            .map(|_| {
                let mut m = GhostManager::new(GhostConfig::standard()).expect("a manager");
                m.set_scope_object(Some(OBJECTS + 1));
                m.activate(&src).expect("activates");
                let seq = m.sequence();
                m.on_connection_message(
                    &ConnectionMessage {
                        seq,
                        message: 1,
                        count: 0,
                    },
                    &mut NoScripts,
                )
                .expect("ready");
                m
            })
            .collect();
        let mut world = World {
            src,
            managers,
            seq: 1,
            now: 0,
            rng: Rng::new(12_345),
            phases: Phases::default(),
            records: 0,
        };
        for _ in 0..200 {
            if world.packets() == 0 {
                break;
            }
        }
        world
    }

    /// One packet to every client, acknowledged; the records written.
    fn packets(&mut self) -> usize {
        let mut records = 0;
        for m in &mut self.managers {
            let t0 = Instant::now();
            let mut w = BitWriter::with_limit(PACKET_BYTES * 8);
            w.enable_string_buffer();
            let mut scan = |_: u64| true;
            let mut recs = |_: u64| true;
            let stats = m.write_section(
                &mut w,
                PACKET_BYTES * 8,
                self.seq,
                self.now,
                &mut self.src,
                SectionBudget {
                    scan: &mut scan,
                    records: &mut recs,
                },
            );
            black_box(&w);
            let t1 = Instant::now();
            m.on_notify(self.seq, true);
            self.phases.write += t1 - t0;
            self.phases.notify += t1.elapsed();
            records += stats.records;
            self.seq += 1;
        }
        records
    }

    /// One tick: `dirty` objects marked on every client (all of them when `dirty` covers the world), then the packets.
    fn tick(&mut self, dirty: u32) {
        let t = Instant::now();
        for k in 0..dirty {
            let obj = if dirty >= OBJECTS {
                k + 1
            } else {
                1 + (self.rng.next_u64() % u64::from(OBJECTS)) as u32
            };
            for m in &mut self.managers {
                m.set_mask(obj, 0x0F);
            }
        }
        self.phases.set_mask += t.elapsed();
        self.now += 32;
        self.records += self.packets();
    }
}

fn cases(c: &mut Criterion) {
    let mut group = c.benchmark_group("ghost_section");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(2));
    for (name, dirty) in [("static", 0u32), ("dirty_100", 100), ("dirty_all", OBJECTS)] {
        // One iteration is a tick of 64 clients; the bench's time is the phase named.
        type Pick = fn(&Phases) -> Duration;
        let phases: [(&str, Pick); 3] = [
            ("tick_set_mask", |p| p.set_mask),
            ("tick_write_section", |p| p.write),
            ("tick_on_notify", |p| p.notify),
        ];
        for (phase, pick) in phases {
            group.bench_function(format!("{name}/{phase}"), |b| {
                let mut world = World::settled();
                b.iter_custom(|iters| {
                    let before = pick(&world.phases);
                    for _ in 0..iters {
                        world.tick(dirty);
                    }
                    pick(&world.phases) - before
                });
            });
        }
    }
    group.finish();
}

criterion_group!(benches, cases);
criterion_main!(benches);
