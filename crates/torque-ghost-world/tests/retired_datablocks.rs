//! A datablock the server removed is retired, kept while a ghost or a mounted image names it or the host still reads it, and
//! dropped by the next purge once nothing does; an unknown or repeated removal is reported, the 257th retired datablock is over
//! the cap, and a ghost reset lets every retired datablock go.

use ghosts::fx::FxLight;
use ghosts::parts::GameBase;
use ghosts::{NetStr, TgeClass, TgeRecord};
use torque_ghost_world::images::MountedImage;
use torque_ghost_world::tge::{TgeClientSet, TgeDatablock};
use torque_ghost_world::world::{DatablockRetire, MAX_RETIRED_DATABLOCKS};
use torque_ghost_world::{ClientWorld, Ghost};

type World = ClientWorld<TgeClientSet>;

fn datablock(class: &str) -> TgeDatablock {
    TgeDatablock {
        class: class.to_string(),
        fields: Vec::new(),
    }
}

/// A world holding datablocks `ids`.
fn world_with(ids: impl IntoIterator<Item = u32>) -> World {
    let mut world = World::default();
    for id in ids {
        assert!(world.apply_datablock(id, datablock("fxLightData")));
    }
    world
}

/// A ghost naming datablock `db`.
fn ghost(db: u32) -> Ghost<TgeClientSet> {
    Ghost {
        class: Some(TgeClass::FxLight),
        serial: 1,
        always: false,
        record: Some(TgeRecord::FxLight(FxLight {
            game: GameBase {
                scale: None,
                datablock: Some(db),
            },
            ..FxLight::default()
        })),
        part: (),
        position: None,
        updates: 0,
    }
}

fn mounted(db: u32) -> MountedImage {
    MountedImage {
        datablock: db,
        skin: NetStr::default(),
        wet: false,
        ammo: false,
        loaded: false,
        target: false,
        trigger: false,
        fire_count: 0,
        state: 0,
        delay: 0.0,
        at_ms: None,
        ambient_time: 0.0,
        spin_scale: 0.0,
        spin_time: 0.0,
        anim: None,
        recoil: None,
        cap_hits: 0,
        events: Vec::new(),
    }
}

fn none_in_use(_: u32) -> bool {
    false
}

#[test]
fn a_retired_datablock_stays_while_a_ghost_names_it_and_goes_with_the_purge_after() {
    let mut world = world_with([10, 11]);
    world.ghosts.insert(5, ghost(10));
    assert_eq!(world.retire_datablock(10), DatablockRetire::Retired);
    assert!(world.is_datablock_retired(10));
    // Named by a ghost: kept, record and all.
    assert_eq!(world.purge_retired_datablocks(none_in_use), 0);
    assert!(world.datablocks.contains_key(&10));
    assert!(world.take_purged_datablocks().is_empty());
    // The ghost leaves: the next purge drops the record, the generation and the mark, and hands the id to the host.
    world.ghosts.remove(&5);
    assert_eq!(world.purge_retired_datablocks(none_in_use), 1);
    assert!(!world.datablocks.contains_key(&10));
    assert_eq!(world.datablock_generation(10), 0);
    assert!(!world.is_datablock_retired(10));
    assert_eq!(world.take_purged_datablocks(), [10]);
    assert!(world.take_purged_datablocks().is_empty(), "taken once");
    // The other datablock was never touched.
    assert!(world.datablocks.contains_key(&11));
    assert_ne!(world.datablock_generation(11), 0);
}

#[test]
fn a_mounted_image_and_the_hosts_own_use_keep_a_retired_datablock() {
    let mut world = world_with([20, 21]);
    world
        .images
        .insert(3, [Some(mounted(20)), None, None, None]);
    assert_eq!(world.retire_datablock(20), DatablockRetire::Retired);
    assert_eq!(world.retire_datablock(21), DatablockRetire::Retired);
    // 20 is named by an image, 21 by the host (a predictor): both stay.
    assert_eq!(world.purge_retired_datablocks(|id| id == 21), 0);
    assert_eq!(world.datablocks.len(), 2);
    // The host lets 21 go: it alone is purged.
    assert_eq!(world.purge_retired_datablocks(none_in_use), 1);
    assert!(world.datablocks.contains_key(&20));
    assert_eq!(world.take_purged_datablocks(), [21]);
    world.images.clear();
    assert_eq!(world.purge_retired_datablocks(none_in_use), 1);
    assert_eq!(world.take_purged_datablocks(), [20]);
    assert!(world.datablocks.is_empty());
}

#[test]
fn an_unknown_or_repeated_removal_is_reported_and_changes_nothing() {
    let mut world = world_with([30]);
    assert_eq!(world.retire_datablock(99), DatablockRetire::Unknown);
    assert_eq!(world.retire_datablock(30), DatablockRetire::Retired);
    assert_eq!(world.retire_datablock(30), DatablockRetire::Already);
    assert_eq!(world.datablock_retired.len(), 1);
    // Nothing retired: a purge reads nothing and drops nothing.
    let mut empty = world_with([1]);
    assert_eq!(empty.purge_retired_datablocks(|_| panic!("not asked")), 0);
}

#[test]
fn the_257th_retired_datablock_is_over_the_cap_and_a_repeat_is_not() {
    let n = MAX_RETIRED_DATABLOCKS as u32;
    let mut world = world_with(10..10 + n + 1);
    // Every datablock is named by a ghost, so none is purged between removals.
    for (i, id) in (10..10 + n + 1).enumerate() {
        world.ghosts.insert(i as u32, ghost(id));
    }
    for id in 10..10 + n {
        assert_eq!(world.retire_datablock(id), DatablockRetire::Retired);
    }
    assert_eq!(world.retire_datablock(10 + n), DatablockRetire::OverCap);
    assert_eq!(world.retire_datablock(10), DatablockRetire::Already);
    assert_eq!(world.datablock_retired.len(), MAX_RETIRED_DATABLOCKS);
    assert!(!world.is_datablock_retired(10 + n));
}

#[test]
fn a_ghost_reset_lets_every_retired_datablock_go() {
    let mut world = world_with([40, 41, 42]);
    world.ghosts.insert(1, ghost(40));
    world.ghosts.insert(2, ghost(41));
    assert_eq!(world.retire_datablock(40), DatablockRetire::Retired);
    assert_eq!(world.retire_datablock(41), DatablockRetire::Retired);
    assert_eq!(world.purge_all_retired_datablocks(), 2);
    assert!(world.datablock_retired.is_empty());
    assert_eq!(world.datablocks.keys().copied().collect::<Vec<_>>(), [42]);
    assert_eq!(world.take_purged_datablocks(), [40, 41]);
    // A datablock sent again at a freed id is taken as new.
    assert!(world.apply_datablock(40, datablock("fxLightData")));
    assert!(!world.is_datablock_retired(40));
}
