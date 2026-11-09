//! A hostile server's per-ghost blocks against a client's world: links to unknown, deleted, reused and own ghost indices,
//! cycles, chains past the caps, out-of-range and non-finite pose codes, raw bytes through the block decoder and the record
//! codec, deletes, resets and the clock running past the wait. Nothing may panic, the client's tree must keep its rules, no
//! more links may wait than the cap, and every pose the client works out is a number or none. `TORQUE_FUZZ_HIERARCHY_NET`
//! sets the number of operations (default 2,000; the full run is 100,000).

use std::collections::BTreeMap;

use torque_ghost_world::ClientWorld;
use torque_ghost_world::links::MAX_WAITING;
use torque_ghost_world::set::SentTable;
use torque_ghost_world::tge::{TgeClientSet, TgeCodec, TgeGhost};
use torque_ghosts::block::{BlockSections, GhostBlock, LinkField, PoseField};
use torque_ghosts::shapes::StaticShape;
use torque_ghosts::{F32, NetWidths, TgeClass, TgeRecord, Xf};
use torque_net::bits::BitReader;
use torque_net::ghost::{ClassForm, GhostChange, GhostCodec};
use torque_net::huffman::Table as StringTable;
use torque_net::testrng::TestRng;

fn count() -> u64 {
    std::env::var("TORQUE_FUZZ_HIERARCHY_NET")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000)
}

/// Ghost indices the fuzz uses: few enough that links meet, plus far ones the client never holds.
const INDICES: u32 = 96;

struct Table(BTreeMap<u32, TgeGhost>);

impl SentTable<TgeClientSet> for Table {
    type Ghost = TgeGhost;

    fn ghost(&self, index: u32) -> Option<&TgeGhost> {
        self.0.get(&index)
    }
}

fn wild(rng: &mut TestRng) -> f32 {
    match rng.below(10) {
        0 => f32::NAN,
        1 => f32::INFINITY,
        2 => -1e30,
        3 => f32::from_bits(rng.next_u64() as u32),
        _ => (rng.below(2_000) as f32 - 1_000.0) * 0.01,
    }
}

/// A record and whether its pose is sane (finite, a unit rotation, a scale near 1); one in ten is wild.
fn record(rng: &mut TestRng) -> (TgeRecord, bool) {
    let sane = rng.below(10) != 0;
    let mut f = |range: f32| {
        F32::new(if sane {
            (rng.below(2_001) as f32 / 1_000.0 - 1.0) * range
        } else {
            wild(rng)
        })
    };
    let xf = Xf {
        position: [f(1_000.0), f(1_000.0), f(1_000.0)],
        quat: [f(0.5), f(0.5), f(0.5)],
        negative_w: false,
    };
    let scale = [1.0 + f(0.5).get(), 1.0 + f(0.5).get(), 1.0 + f(0.5).get()].map(F32::new);
    let record = TgeRecord::StaticShape(StaticShape {
        transform: Some((xf, scale)),
        ..StaticShape::default()
    });
    (record, sane)
}

fn index(rng: &mut TestRng) -> u32 {
    match rng.below(20) {
        0 => rng.next_u64() as u32,
        1 => INDICES + rng.below(8) as u32,
        _ => rng.below(u64::from(INDICES)) as u32,
    }
}

fn block(rng: &mut TestRng) -> GhostBlock {
    let link = match rng.below(5) {
        0 => None,
        1 => Some(LinkField::Root),
        2 => Some(LinkField::Waiting),
        _ => Some(LinkField::Parent {
            ghost: index(rng),
            node: (rng.below(3) == 0).then(|| rng.next_u64() as u16),
        }),
    };
    let pose = (rng.below(3) != 0).then(|| PoseField {
        translation: std::array::from_fn(|_| {
            if rng.below(8) == 0 {
                rng.next_u64() as i32
            } else {
                rng.below(20_000) as i32 - 10_000
            }
        }),
        dropped: rng.below(4) as u8,
        parts: std::array::from_fn(|_| rng.next_u64() as u16),
        scale: (rng.below(3) == 0).then(|| std::array::from_fn(|_| rng.next_u64() as u16)),
        snap: rng.below(2) == 0,
    });
    GhostBlock { link, pose }
}

#[test]
fn hostile_blocks_never_panic_the_client_and_its_caps_hold() {
    let mut rng = TestRng::new(0x6e7f_1a2b_3c4d);
    let mut world: ClientWorld<TgeClientSet> = ClientWorld::default();
    let mut table = Table(BTreeMap::new());
    let mut clock = 0u32;
    let on = BlockSections { links: true };
    let mut codec = TgeCodec::new(
        torque_ghosts::tge::TGE_OBJECT_CLASSES,
        NetWidths::engine_default(),
        StringTable::torque(),
    )
    .with_blocks(on);
    let mut serial = 0;
    let mut changes = Vec::new();
    let (mut most_linked, mut most_waiting) = (0, 0);
    // Which ghosts' records put them at a sane pose (the last record each was sent).
    let mut sane: BTreeMap<u32, bool> = BTreeMap::new();
    let mut checked = 0u64;
    for op in 0..count() {
        changes.clear();
        match rng.below(100) {
            0..=29 => {
                let i = index(&mut rng);
                serial += 1;
                let (sent, is_sane) = record(&mut rng);
                sane.insert(i, is_sane);
                table.0.insert(
                    i,
                    TgeGhost {
                        class: Some(TgeClass::StaticShape),
                        serial,
                        record: Some(sent),
                        block: (rng.below(4) != 0).then(|| block(&mut rng)),
                        ..TgeGhost::default()
                    },
                );
                changes.push(GhostChange::Created(i));
            }
            30..=64 => {
                let i = index(&mut rng);
                if let Some(g) = table.0.get_mut(&i) {
                    g.updates += 1;
                    g.record = (rng.below(2) == 0).then(|| {
                        let (sent, is_sane) = record(&mut rng);
                        sane.insert(i, is_sane);
                        sent
                    });
                    g.block = Some(block(&mut rng));
                }
                changes.push(GhostChange::Updated(i));
            }
            65..=79 => {
                let i = index(&mut rng);
                table.0.remove(&i);
                sane.remove(&i);
                changes.push(GhostChange::Deleted(i));
            }
            80..=89 => {
                // Raw bytes through the block decoder and the record codec: an error is fine, a panic is not.
                let len = rng.index(64);
                let bytes: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
                let mut r = BitReader::new(&bytes);
                let read = GhostBlock::read(&mut r, on, 12, &mut |_| true);
                let mut r = BitReader::new(&bytes);
                let id = rng.below(64) as u32;
                if let Ok(mut ghost) = codec.create(ClassForm::Id(id), &mut r) {
                    let _ = codec.update(&mut ghost, &mut r);
                }
                if let Ok(block) = read {
                    let i = index(&mut rng);
                    if let Some(g) = table.0.get_mut(&i) {
                        g.block = Some(block);
                        g.updates += 1;
                        changes.push(GhostChange::Updated(i));
                    }
                }
            }
            90..=96 => {
                clock = clock.wrapping_add(rng.below(7_000) as u32);
                world.set_clock(clock);
                world.anim_ms = clock;
                world.animate = rng.below(2) == 0;
            }
            97 => {
                // A flood of links to ghosts the client will never hold.
                for k in 0..(MAX_WAITING as u32 + 64) {
                    let i = 10_000 + k;
                    serial += 1;
                    table.0.insert(
                        i,
                        TgeGhost {
                            class: Some(TgeClass::StaticShape),
                            serial,
                            record: Some(record(&mut rng).0),
                            block: Some(GhostBlock {
                                link: Some(LinkField::Parent {
                                    ghost: 1_000_000 + k,
                                    node: None,
                                }),
                                pose: None,
                            }),
                            ..TgeGhost::default()
                        },
                    );
                    changes.push(GhostChange::Created(i));
                }
            }
            _ => {
                if rng.below(4) == 0 {
                    table.0.clear();
                    sane.clear();
                    changes.push(GhostChange::Cleared);
                }
            }
        }
        world.apply_ghosts(op as usize, &changes, &table, [0.0; 3], &mut ());
        let links = &world.links;
        assert!(links.waiting_count() <= MAX_WAITING, "op {op}");
        most_linked = most_linked.max(links.linked_count());
        most_waiting = most_waiting.max(links.waiting_count());
        if op % 64 == 0 {
            links.audit().unwrap_or_else(|v| panic!("op {op}: {v:?}"));
        }
        let probe = index(&mut rng);
        // A ghost drawn through a chain of sane records, none left where its parent went, is drawn at a number: whatever
        // links and poses the server sent, the client's composition stays finite.
        let mut chain =
            std::iter::successors(Some(probe), |&g| links.parent(g).map(|p| p.object.0));
        let sane_chain = chain.all(|g| {
            sane.get(&g).copied().unwrap_or(false)
                && world.ghosts.contains_key(&g)
                && links.held(g).is_none()
        });
        if sane_chain {
            let (m, s) = world
                .world_pose(probe, &|_, _| None)
                .unwrap_or_else(|| panic!("op {op}: ghost {probe} has a pose"));
            assert!(
                m.iter().chain(&s).all(|v| v.is_finite()),
                "op {op}: ghost {probe} is drawn at {m:?} {s:?}"
            );
            checked += 1;
        }
        if let Some(p) = links.parent(probe) {
            assert!(
                world.ghosts.contains_key(&p.object.0),
                "op {op}: a link to a ghost the client lacks"
            );
        }
    }
    world
        .links
        .audit()
        .expect("the client's tree keeps its rules");
    // The fuzz reached what it is for: links made, links refused, links waiting up to the cap.
    let stats = world.links.stats();
    assert!(
        most_linked > 0 && stats.refused > 0 && stats.bad_poses > 0 && checked > 0,
        "{stats:?} {most_linked} {checked}"
    );
    if count() >= 2_000 {
        assert_eq!(most_waiting, MAX_WAITING, "{stats:?}");
        assert!(stats.over_cap > 0 && stats.expired > 0, "{stats:?}");
    }
}
