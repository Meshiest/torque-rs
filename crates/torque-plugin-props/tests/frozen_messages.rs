//! The `props-frozen` channel's messages as a hostile server may write them: every field is checked, a message is taken whole
//! or not at all, and the table a client keeps stays within its caps whatever it is sent. `TORQUE_FUZZ_PROPS` sets the
//! iterations (20,000 by default; the owner's checks run 100,000).

use torque_net::chunk::Kind;
use torque_plugin_props::frozen::client::{FrozenTable, MAX_FROZEN_PROPS};
use torque_plugin_props::frozen::messages::{
    self, Frozen, MAX_MESSAGE_BYTES, MAX_MESSAGE_OPS, MessageError, Op, SET_BYTES, TAG_SET,
};
use torque_plugin_props::quant::Quantized;
use torque_plugin_props::world::MAX_PROPS;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn iterations() -> u64 {
    std::env::var("TORQUE_FUZZ_PROPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000)
}

/// A state inside the wire's ranges.
fn state(rng: &mut Rng) -> Quantized {
    Quantized {
        pos: std::array::from_fn(|_| rng.below(2_000_000) as i32 - 1_000_000),
        rot_index: rng.below(4) as u8,
        rot: std::array::from_fn(|_| rng.below(32_000) as u16),
        ..Quantized::default()
    }
    .held()
}

fn op(rng: &mut Rng) -> Op {
    let slot = rng.below(u64::from(MAX_PROPS)) as u16;
    if rng.below(4) == 0 {
        return Op::Remove { slot };
    }
    Op::Set {
        slot,
        prop: Frozen {
            data: rng.next() as u32,
            mine: rng.below(2) == 0,
            q: state(rng),
        },
    }
}

fn message(rng: &mut Rng) -> Vec<Op> {
    (0..rng.below(40)).map(|_| op(rng)).collect()
}

/// What a table must hold whatever it was sent.
fn check(table: &FrozenTable) {
    assert!(table.len() <= MAX_FROZEN_PROPS);
    for (slot, p) in table.iter() {
        assert!(u32::from(slot) < MAX_PROPS);
        assert_eq!(p.prop.q, p.prop.q.held(), "slot {slot} is in range");
        assert!(p.prop.q.position().iter().all(|v| v.is_finite()));
        assert!(p.prop.q.rotation().iter().all(|v| v.is_finite()));
    }
}

#[test]
fn what_a_server_writes_a_client_reads_back() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..2_000 {
        let ops = message(&mut rng);
        let bytes = messages::write(&ops);
        assert_eq!(messages::read(&bytes), Ok(ops));
    }
}

#[test]
fn every_field_is_checked_and_a_refused_message_gives_no_op() {
    let set = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut bytes = messages::write(&[Op::Set {
            slot: 3,
            prop: Frozen {
                data: 9,
                mine: false,
                q: Quantized::default(),
            },
        }]);
        f(&mut bytes);
        messages::read(&bytes)
    };
    assert!(set(&|_| {}).is_ok());
    // A tag that is no op, a slot past the most props, flags that mean nothing, a state outside its ranges, a cut-off op.
    assert_eq!(set(&|b| b[0] = 9), Err(MessageError::Tag(9)));
    assert_eq!(
        set(&|b| b[1..3].copy_from_slice(&(MAX_PROPS as u16).to_le_bytes())),
        Err(MessageError::Slot(MAX_PROPS as u16))
    );
    assert_eq!(set(&|b| b[7] = 0x80), Err(MessageError::Flags(0x80)));
    assert_eq!(
        set(&|b| b[8..12].copy_from_slice(&i32::MAX.to_le_bytes())),
        Err(MessageError::Range)
    );
    assert_eq!(set(&|b| b[20] = 7), Err(MessageError::Range));
    assert_eq!(
        set(&|b| b[21..23].copy_from_slice(&u16::MAX.to_le_bytes())),
        Err(MessageError::Range)
    );
    assert_eq!(
        set(&|b| b.truncate(SET_BYTES - 1)),
        Err(MessageError::Truncated)
    );
    // A good op followed by a bad one gives nothing at all.
    assert!(set(&|b| b.push(TAG_SET)).is_err());
}

#[test]
fn a_message_is_held_to_its_size_and_its_count() {
    let long = vec![messages::TAG_REMOVE; MAX_MESSAGE_BYTES + 1];
    assert_eq!(messages::read(&long), Err(MessageError::TooMany));
    // The most ops of the cheapest kind fit the byte cap, and past the op cap they are refused.
    let removes: Vec<u8> = (0..=MAX_MESSAGE_OPS)
        .flat_map(|k| [messages::TAG_REMOVE, (k % 100) as u8, 0])
        .collect();
    assert!(removes.len() <= MAX_MESSAGE_BYTES);
    assert_eq!(messages::read(&removes), Err(MessageError::TooMany));
    assert_eq!(
        messages::read(&removes[..removes.len() - 3]).map(|o| o.len()),
        Ok(MAX_MESSAGE_OPS)
    );
}

#[test]
fn a_table_holds_one_prop_a_slot_whatever_it_is_sent() {
    let mut table = FrozenTable::default();
    let mut rng = Rng(7);
    for _ in 0..40 {
        let ops: Vec<Op> = (0..MAX_MESSAGE_OPS).map(|_| op(&mut rng)).collect();
        assert!(table.apply(Kind::Delta, &messages::write(&ops)));
        check(&table);
    }
    // Every slot filled is the most there is.
    let all: Vec<Op> = (0..MAX_PROPS)
        .map(|s| Op::Set {
            slot: s as u16,
            prop: Frozen {
                data: 1,
                mine: false,
                q: Quantized::default(),
            },
        })
        .collect();
    for chunk in all.chunks(MAX_MESSAGE_OPS) {
        assert!(table.apply(Kind::Delta, &messages::write(chunk)));
    }
    assert_eq!(table.len(), MAX_FROZEN_PROPS);
    // A snapshot replaces the table, and one of nothing empties it.
    assert!(table.apply(Kind::Snapshot, &messages::write(&all[..3])));
    assert_eq!(table.len(), 3);
    assert!(table.apply(Kind::Snapshot, &[]));
    assert!(table.is_empty());
}

#[test]
fn hostile_bytes_never_panic_and_never_leave_a_value_out_of_range() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let mut table = FrozenTable::default();
    let (mut taken, mut refused) = (0u64, 0u64);
    for _ in 0..iterations() {
        let bytes: Vec<u8> = match rng.below(3) {
            // Random bytes.
            0 => (0..rng.below(300)).map(|_| rng.next() as u8).collect(),
            // A good message with bytes flipped, cut or added.
            1 => {
                let mut b = messages::write(&message(&mut rng));
                for _ in 0..rng.below(4) {
                    if b.is_empty() {
                        break;
                    }
                    let at = rng.below(b.len() as u64) as usize;
                    match rng.below(3) {
                        0 => b[at] = rng.next() as u8,
                        1 => b.truncate(at),
                        _ => b.insert(at, rng.next() as u8),
                    }
                }
                b
            }
            // A good message.
            _ => messages::write(&message(&mut rng)),
        };
        let before = table.state_words();
        let kind = if rng.below(8) == 0 {
            Kind::Snapshot
        } else {
            Kind::Delta
        };
        if table.apply(kind, &bytes) {
            taken += 1;
        } else {
            refused += 1;
            assert_eq!(
                table.state_words(),
                before,
                "a refused message changes nothing"
            );
        }
        check(&table);
    }
    assert!(taken > 0 && refused > 0, "{taken} taken, {refused} refused");
    assert_eq!(table.rejected, refused);
}

#[test]
fn a_server_naming_datablock_ids_it_never_sent_cannot_fill_the_tables_cache() {
    use torque_ts_vm::vm::Vm;
    let mut vm = Vm::new();
    vm.register_class("PropData", "SimObject", &[]);
    let real: Vec<u32> = (0..8)
        .map(|_| {
            vm.new_object(b"PropData", &[])
                .expect("an object")
                .expect("a datablock")
        })
        .collect();
    let mut table = FrozenTable::default();
    let mut rng = Rng(0xfeed_f00d);
    let slots = 6u16;
    for _ in 0..3_000 {
        // Every round names new ids: half the ones the client has, half it never got.
        let ops: Vec<Op> = (0..slots)
            .map(|slot| Op::Set {
                slot,
                prop: Frozen {
                    data: if rng.below(2) == 0 {
                        real[rng.below(real.len() as u64) as usize]
                    } else {
                        rng.next() as u32
                    },
                    mine: false,
                    q: Quantized::default(),
                },
            })
            .collect();
        assert!(table.apply(Kind::Delta, &messages::write(&ops)));
        table.resolve(&vm);
        // What is kept is what a prop names now and the client has: never more than the props, never an id it lacks.
        assert!(table.cached_blocks() <= usize::from(slots));
        for (_, p) in table.iter() {
            assert_eq!(
                table.params(p.prop.data).is_some(),
                real.contains(&p.prop.data)
            );
        }
    }
    // Nothing names a datablock, nothing is kept.
    assert!(table.apply(Kind::Snapshot, &[]));
    table.resolve(&vm);
    assert_eq!(table.cached_blocks(), 0);
}
