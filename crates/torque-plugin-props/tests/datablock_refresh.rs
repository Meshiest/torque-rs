//! A `PropData` changed by a datablock event has every prop of it read its numbers again, in the book the client's ticks keep:
//! after the refresh the book's numbers equal a fresh book's, read from the same objects. A record sent again unchanged moves
//! no object generation and refreshes nothing, and a refresh is spread over ticks at the frame's budget.

use torque_client::datablocks::{DatablockObjects, Received, Unpacked, UnpackedField, receive};
use torque_engine::net_classes::GhostEvent;
use torque_engine::{ClientStage, EngineBuilder};
use torque_ghost_world::world::MAX_REFRESHES_PER_FRAME;
use torque_ghosts::plugin::DatablockRef;
use torque_plugin_props::client::{heard, params_of, refresh_datablocks};
use torque_plugin_props::records::InitWire;
use torque_plugin_props::{ClientProps, PropBook, PropGhost, PropsPlugin};
use torque_ts_vm::vm::Vm;

const HEAVY: u32 = 40;
const LIGHT: u32 = 41;

/// A client's script engine with the props plugin's classes.
fn client() -> Vm {
    let engine = EngineBuilder::new()
        .add_plugins(PropsPlugin)
        .build()
        .expect("the engine builds");
    let mut vm = Vm::new();
    // The class's parent (`GameBaseData`) is the engine's.
    torque_game::specs::install(&mut vm);
    for &stage in ClientStage::ALL {
        engine.replay(stage, &mut vm).expect("the stage replays");
    }
    vm
}

/// A `PropData` event's unpack: `fields` by name and text.
fn unpacked(id: u32, fields: &[(&str, &str)]) -> Unpacked {
    Unpacked {
        id,
        class: "PropData".into(),
        fields: fields
            .iter()
            .map(|(name, text)| UnpackedField {
                name: name.as_bytes().to_vec(),
                index: Vec::new(),
                text: text.as_bytes().to_vec(),
                pointer: None,
            })
            .collect(),
        error: None,
        emitter_ids: None,
    }
}

fn fields(mass: &str, size: &str, friction: &str) -> Vec<(&'static str, String)> {
    vec![
        ("collision", "box".to_string()),
        ("mass", mass.to_string()),
        ("size", size.to_string()),
        ("friction", friction.to_string()),
    ]
}

fn send(vm: &mut Vm, id: u32, fields: &[(&str, String)]) -> Received {
    let borrowed: Vec<(&str, &str)> = fields.iter().map(|(n, t)| (*n, t.as_str())).collect();
    receive(vm, &unpacked(id, &borrowed), None).expect("the event is taken")
}

fn prop(data: u32, slot: u16) -> PropGhost {
    PropGhost {
        data_block: DatablockRef(Some(data)),
        init: Some(InitWire {
            slot,
            mine: false,
            predict: 0,
            hulls: None,
        }),
        state: None,
        flags: None,
    }
}

/// `count` props heard into `book`, the even ghosts of [`HEAVY`], the odd of [`LIGHT`].
fn hear(vm: &mut Vm, book: &ClientProps, count: u32) {
    for g in 1..=count {
        let data = if g % 2 == 0 { HEAVY } else { LIGHT };
        heard(
            vm,
            book,
            GhostEvent::Added {
                index: g,
                record: &prop(data, g as u16),
            },
        );
    }
}

/// A book made now of the same props, its numbers read fresh.
fn fresh(vm: &mut Vm, count: u32) -> ClientProps {
    let book = ClientProps::default();
    hear(vm, &book, count);
    book
}

fn same_params(a: &PropBook, b: &PropBook) -> bool {
    a.props().len() == b.props().len()
        && a.props()
            .iter()
            .all(|(g, p)| b.get(*g).is_some_and(|q| q.params == p.params))
}

#[test]
fn an_edited_datablock_gives_every_prop_of_it_a_fresh_books_numbers() {
    let mut vm = client();
    assert_eq!(
        send(&mut vm, HEAVY, &fields("12.5", "1 2 3", "0.5")),
        Received::Added(HEAVY)
    );
    assert_eq!(
        send(&mut vm, LIGHT, &fields("3", "1 1 1", "0.7")),
        Received::Added(LIGHT)
    );
    let book = ClientProps::default();
    hear(&mut vm, &book, 10);
    let objects = DatablockObjects::of(&vm).expect("the objects' table");
    assert!(objects.made(&vm, HEAVY) && objects.made(&vm, LIGHT));
    let epoch = objects.epoch();
    assert_eq!(objects.generation(HEAVY), 1);
    // The tick's sweep: every prop was read at its datablock's generation, so nothing is read again.
    refresh_datablocks(&vm, &book);
    book.with(|b| {
        assert_eq!(b.get(2).unwrap().params, params_of(&vm, HEAVY));
        assert_eq!(b.get(2).unwrap().generation, 1);
        assert!(b.params_current(epoch));
    });

    // The same record again: the object is written nowhere, no generation moves, nothing is read again.
    assert_eq!(
        send(&mut vm, HEAVY, &fields("12.5", "1 2 3", "0.5")),
        Received::Updated(HEAVY)
    );
    assert_eq!(objects.epoch(), epoch);
    refresh_datablocks(&vm, &book);
    book.with(|b| assert!(b.params_current(epoch)));

    // 100 edits, each read again by the props of the edited datablock alone.
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for edit in 0..100u32 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let mass = format!("{}", 1.0 + (seed % 1000) as f32 * 0.37);
        let size = format!(
            "{} {} {}",
            0.5 + (seed >> 10 & 0xf) as f32 * 0.25,
            0.5 + (seed >> 14 & 0xf) as f32 * 0.25,
            0.5 + (seed >> 18 & 0xf) as f32 * 0.25
        );
        let friction = format!("{}", (seed >> 22 & 0x7) as f32 * 0.1);
        let before = (objects.generation(HEAVY), objects.generation(LIGHT));
        assert_eq!(
            send(&mut vm, HEAVY, &fields(&mass, &size, &friction)),
            Received::Updated(HEAVY),
            "edit {edit}"
        );
        assert!(objects.generation(HEAVY) > before.0, "edit {edit}");
        assert_eq!(objects.generation(LIGHT), before.1, "edit {edit}");
        let light_before = book.with(|b| b.get(1).unwrap().params).unwrap();
        refresh_datablocks(&vm, &book);
        let fresh = fresh(&mut vm, 10);
        book.with(|b| {
            fresh.with(|f| assert!(same_params(b, f), "edit {edit}: a fresh book's numbers"));
            assert_eq!(
                b.get(2).unwrap().params,
                params_of(&vm, HEAVY),
                "edit {edit}"
            );
            assert_eq!(
                b.get(1).unwrap().params,
                light_before,
                "edit {edit}: the other datablock's props keep theirs"
            );
            assert!(b.params_current(objects.epoch()), "edit {edit}");
        });
    }
}

#[test]
fn a_refresh_is_spread_over_ticks_at_the_frames_budget() {
    let mut vm = client();
    assert_eq!(
        send(&mut vm, HEAVY, &fields("12.5", "1 2 3", "0.5")),
        Received::Added(HEAVY)
    );
    assert_eq!(
        send(&mut vm, LIGHT, &fields("3", "1 1 1", "0.7")),
        Received::Added(LIGHT)
    );
    let book = ClientProps::default();
    // Half of them are of the edited datablock: three ticks' worth and a little.
    let count = (3 * MAX_REFRESHES_PER_FRAME + 10) as u32 * 2;
    hear(&mut vm, &book, count);
    let objects = DatablockObjects::of(&vm).expect("the objects' table");
    assert_eq!(
        send(&mut vm, HEAVY, &fields("20", "2 2 2", "0.1")),
        Received::Updated(HEAVY)
    );
    let generation = objects.generation(HEAVY);
    let stale = |b: &PropBook| {
        b.props()
            .values()
            .filter(|p| p.data == Some(HEAVY) && p.generation != generation)
            .count()
    };
    let mut ticks = 0;
    while !book.with(|b| b.params_current(objects.epoch())).unwrap() {
        let before = book.with(stale).unwrap();
        refresh_datablocks(&vm, &book);
        let after = book.with(stale).unwrap();
        assert_eq!(before - after, (before).min(MAX_REFRESHES_PER_FRAME));
        ticks += 1;
        assert!(ticks <= 5, "the refresh ends");
    }
    assert_eq!(ticks, 4);
    let fresh = fresh(&mut vm, count);
    book.with(|b| fresh.with(|f| assert!(same_params(b, f))));
}
