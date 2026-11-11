//! A development server reloads a shape whose file changed: the kept shape is read again within one poll interval and the
//! objects of every datablock on it are refreshed at that tick (box and node count); nothing is read or reloaded with the
//! gate off; a file that no longer parses keeps the old shape and prints its error once until it changes again.

use crate::net_client::{self, NetServer, eval};

use torque_engine::assets::HOT_RELOAD_INTERVAL_MS;
use torque_engine::script::{DEV_PREF, dev_allowed_in};
use torque_game::scene::{ext, refresh};
use torque_game::shapes::{self, BAD_SHAPE_LINE, Shapes, reload};
use torque_server::server::TICK_MS;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::sim::ObjId;

/// How many StaticShapes, each on its own datablock and `.dts`.
const COUNT: usize = 20;

/// A box of `half` units with `extra_nodes` below the root.
fn dts(half: f32, extra_nodes: &[&str]) -> Vec<u8> {
    let b = [-half, -half, 0.0, half, half, half];
    let shape = torque_shapes::synth::hull_shape(b, &[("detail2", b)], extra_nodes, &["ambient"]);
    torque_shapes::write::write_dts(&shape)
}

fn file(i: usize) -> String {
    format!("s{i}.dts")
}

/// A server, its first tick run, and the files its shapes come from (each of `COUNT` boxes of one unit, written at time 1).
fn server() -> (NetServer, MemFiles) {
    let files = MemFiles::new();
    for i in 0..COUNT {
        files.insert_at(&file(i), &dts(1.0, &[]), 1);
    }
    let (s, _, _) = net_client::game_server(files.clone());
    (s, files)
}

/// `COUNT` datablocks and a StaticShape on each.
fn declare(s: &mut NetServer) -> Vec<ObjId> {
    for i in 0..COUNT {
        eval(
            s,
            &format!(
                "datablock StaticShapeData(D{i}) {{ shapeFile = \"{}\"; }}; \
                 new StaticShape(S{i}) {{ dataBlock = D{i}; position = \"{i} 0 0\"; }};",
                file(i)
            ),
        );
    }
    (0..COUNT)
        .map(|i| {
            s.vm.objects()
                .find(format!("S{i}").as_bytes())
                .expect("the object")
        })
        .collect()
}

/// One poll interval of ticks, and one more.
fn interval(s: &mut NetServer) {
    for _ in 0..HOT_RELOAD_INTERVAL_MS.div_ceil(TICK_MS) + 1 {
        s.run(TICK_MS, |_| false);
    }
}

/// The object's box (its max corner's x) and its shape's node count.
fn box_and_nodes(s: &NetServer, id: ObjId) -> (f32, usize) {
    let e = ext::get(s.vm.objects(), id).expect("the object's state");
    let scene = e.scene.expect("a scene object");
    let nodes = e
        .shape
        .as_deref()
        .and_then(|st| st.body.as_ref())
        .map_or(0, |b| b.shape().nodes.len());
    (scene.object_max[0], nodes)
}

fn shapes(s: &NetServer) -> std::rc::Rc<Shapes> {
    shapes::shapes_of(&s.vm).expect("the loader")
}

fn error_lines(s: &mut NetServer) -> usize {
    s.vm.drain_log()
        .iter()
        .filter(|l| String::from_utf8_lossy(&l.text).contains(BAD_SHAPE_LINE))
        .count()
}

#[test]
fn twenty_changed_shape_files_change_their_objects_box_and_node_count_within_one_interval() {
    let (mut s, files) = server();
    let objects = declare(&mut s);
    s.run(TICK_MS, |_| false);
    for &id in &objects {
        assert_eq!(box_and_nodes(&s, id), (1.0, 1));
    }
    let before = shapes(&s).reload_stats();
    let heavy_before = refresh::stats(&s.vm).heavy;
    for i in 0..COUNT {
        files.insert_at(&file(i), &dts(2.0, &["arm", "leg"]), 2);
    }
    interval(&mut s);
    for &id in &objects {
        assert_eq!(
            box_and_nodes(&s, id),
            (2.0, 3),
            "the new shape's box and nodes"
        );
    }
    let after = shapes(&s).reload_stats();
    assert_eq!(
        after.reloads - before.reloads,
        COUNT as u64,
        "one reload per file"
    );
    assert_eq!(
        after.marked - before.marked,
        COUNT as u64,
        "one mark per datablock"
    );
    assert_eq!(after.failed, before.failed);
    assert_eq!(
        refresh::stats(&s.vm).heavy - heavy_before,
        COUNT as u64,
        "one heavy refresh per object"
    );
    // A file whose time moves back to what it was is a change too, and the same contents load again.
    files.set_modified(file(0).as_bytes(), 1);
    interval(&mut s);
    assert_eq!(shapes(&s).reload_stats().reloads - after.reloads, 1);
    assert_eq!(box_and_nodes(&s, objects[0]), (2.0, 3));
}

#[test]
fn nothing_is_read_or_reloaded_with_the_gate_off() {
    let (mut s, files) = server();
    // A release build's gate: off until the pref is set.
    assert!(!dev_allowed_in(&s.vm, false));
    s.vm.set_global(DEV_PREF, "1");
    assert!(dev_allowed_in(&s.vm, false));
    s.vm.set_global(DEV_PREF, "0");
    let shapes = shapes(&s);
    shapes.set_hot_reload(Some(false));
    s.run(TICK_MS, |_| false);
    let before = shapes.reload_stats();
    let objects = declare(&mut s);
    s.run(TICK_MS, |_| false);
    for i in 0..COUNT {
        files.insert_at(&file(i), &dts(2.0, &["arm"]), 2);
    }
    for _ in 0..3 {
        interval(&mut s);
    }
    let after = shapes.reload_stats();
    assert_eq!(after.looks, before.looks, "0 change times read");
    assert_eq!(after.reloads, 0, "0 reloads");
    assert_eq!(after.polls, before.polls, "0 polls");
    for &id in &objects {
        assert_eq!(box_and_nodes(&s, id), (1.0, 1));
    }
    // Turned on again, the first poll takes the files as they are now (nothing was read while off), and the next change is
    // seen.
    shapes.set_hot_reload(None);
    s.run(TICK_MS, |_| false);
    assert_eq!(shapes.reload_stats().reloads, 0);
    for i in 0..COUNT {
        files.insert_at(&file(i), &dts(2.0, &["arm"]), 3);
    }
    interval(&mut s);
    assert_eq!(shapes.reload_stats().reloads, COUNT as u64);
    assert_eq!(box_and_nodes(&s, objects[3]), (2.0, 2));
}

#[test]
fn a_file_that_no_longer_parses_keeps_the_old_shape_and_prints_once_until_it_changes_again() {
    let (mut s, files) = server();
    let objects = declare(&mut s);
    s.run(TICK_MS, |_| false);
    error_lines(&mut s);
    let good = dts(3.0, &["arm"]);
    files.insert_at(&file(0), &good[..40], 2);
    interval(&mut s);
    assert_eq!(
        box_and_nodes(&s, objects[0]),
        (1.0, 1),
        "the old shape stays"
    );
    assert_eq!(error_lines(&mut s), 1, "the error line, once");
    let stats = shapes(&s).reload_stats();
    assert_eq!((stats.reloads, stats.failed), (0, 1));
    for _ in 0..3 {
        interval(&mut s);
    }
    assert_eq!(
        error_lines(&mut s),
        0,
        "not tried again while the file stays"
    );
    assert_eq!(shapes(&s).reload_stats().failed, 1);
    files.insert_at(&file(0), &good, 3);
    interval(&mut s);
    assert_eq!(
        box_and_nodes(&s, objects[0]),
        (3.0, 2),
        "the fixed file loads"
    );
    assert_eq!(error_lines(&mut s), 0);
    let stats = shapes(&s).reload_stats();
    assert_eq!((stats.reloads, stats.failed), (1, 1));
    assert_eq!(
        box_and_nodes(&s, objects[1]),
        (1.0, 1),
        "the other files were not touched"
    );
}

#[test]
fn a_shape_too_large_for_the_budget_is_given_up_with_one_line_until_it_changes_again() {
    let (mut s, files) = server();
    let objects = declare(&mut s);
    s.run(TICK_MS, |_| false);
    s.vm.drain_log();
    let too_large = |s: &mut NetServer| {
        s.vm.drain_log()
            .iter()
            .filter(|l| String::from_utf8_lossy(&l.text).contains("too large to reload live"))
            .count()
    };
    shapes(&s).set_reload_work(16);
    files.insert_at(&file(0), &dts(2.0, &["arm"]), 2);
    files.insert_at(&file(1), &dts(2.0, &["arm"]), 2);
    interval(&mut s);
    assert_eq!(
        box_and_nodes(&s, objects[0]),
        (1.0, 1),
        "the old shape stays"
    );
    assert_eq!(too_large(&mut s), 2, "one line per file");
    let stats = shapes(&s).reload_stats();
    assert_eq!((stats.reloads, stats.failed), (0, 2));
    for _ in 0..3 {
        interval(&mut s);
    }
    assert_eq!(too_large(&mut s), 0, "not tried again while the files stay");
    assert_eq!(shapes(&s).reload_stats().failed, 2);
    // With the budget back and the file changed again, it loads.
    shapes(&s).set_reload_work(reload::RELOAD_WORK);
    files.set_modified(file(0).as_bytes(), 3);
    interval(&mut s);
    assert_eq!(box_and_nodes(&s, objects[0]), (2.0, 2));
    assert_eq!(
        box_and_nodes(&s, objects[1]),
        (1.0, 1),
        "the other waits for its own change"
    );
    assert_eq!(shapes(&s).reload_stats().reloads, 1);
}
