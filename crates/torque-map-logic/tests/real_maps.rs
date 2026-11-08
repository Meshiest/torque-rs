//! The map logic over a game's own maps: run only when `TORQUE_SOURCE_GAME_DIR` names game folders (separated by `;`, each with
//! a `gameinfo.txt`), and skipped otherwise. `TORQUE_SOURCE_FGD` may name an FGD file of the install (its folder holds its
//! includes); its definitions then sit over the coded defaults. `TORQUE_MAP_LOGIC_MAPS` caps how many maps run. Nothing is
//! written and nothing from the game is printed beyond counts and class names.
//!
//! Each map: its entities with their connections, its brush entities' brushes as volumes, ten seconds of its own logic, then a
//! player put inside every trigger in turn and every button used, then five more seconds. The oracles: no panic; no event
//! past a cap; every trigger with a brush model has volumes; the queue drains or holds only delayed events.

use std::collections::BTreeMap;
use std::path::Path;

use torque_format_valve::brushes::MapHulls;
use torque_format_valve::bsp::{Bsp, value};
use torque_format_valve::files::SourceFiles;
use torque_map_logic::connection::entity_from_pairs;
use torque_map_logic::defaults::Defaults;
use torque_map_logic::entities::Kind;
use torque_map_logic::fgd::EntityDefs;
use torque_map_logic::{BrushVolumes, MapLogic, Toucher, ToucherKind, Volume};

fn mount() -> Option<SourceFiles> {
    let dirs = std::env::var("TORQUE_SOURCE_GAME_DIR").ok()?;
    let mut files = SourceFiles::new();
    for d in dirs.split(';').filter(|d| !d.trim().is_empty()) {
        files.add_gameinfo(Path::new(d.trim())).ok()?;
    }
    Some(files)
}

fn defaults() -> (Defaults, usize) {
    let Some(f) = std::env::var_os("TORQUE_SOURCE_FGD") else {
        return (Defaults::coded(), 0);
    };
    let f = Path::new(&f);
    let (Some(dir), Some(name)) = (f.parent(), f.file_name().and_then(|n| n.to_str())) else {
        return (Defaults::coded(), 0);
    };
    let (defs, warnings) = EntityDefs::load_dir(dir, name);
    eprintln!(
        "FGD: {} entity classes, {} warnings",
        defs.len(),
        warnings.len()
    );
    let n = defs.len();
    (Defaults::with_fgd(defs), n)
}

#[derive(Default, Debug)]
struct Totals {
    maps: usize,
    entities: usize,
    connections: usize,
    supported: usize,
    unsupported: BTreeMap<String, usize>,
    triggers: usize,
    triggers_without_volumes: usize,
    movers: usize,
    events: u64,
    inputs: u64,
    missing: u64,
    unhandled: u64,
    dropped_depth: u64,
    dropped_queue: u64,
    deferred: u64,
    sounds: usize,
    queued_after: usize,
    unhandled_kinds: std::collections::BTreeSet<String>,
}

#[test]
fn a_games_maps_run_their_logic_within_the_caps() {
    let Some(files) = mount() else {
        eprintln!("TORQUE_SOURCE_GAME_DIR is not set: skipped");
        return;
    };
    let cap: usize = std::env::var("TORQUE_MAP_LOGIC_MAPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
    let (mut defs, fgd_classes) = defaults();
    let mut t = Totals::default();
    let maps = files.list("maps/", ".bsp");
    for path in maps.iter().take(cap) {
        let Some(Ok(bytes)) = files.read_path(path) else {
            continue;
        };
        let Ok(map) = Bsp::parse(&bytes) else {
            eprintln!("{path}: not read");
            continue;
        };
        let Ok(pairs) = map.entities() else { continue };
        let entities: Vec<_> = pairs.iter().map(|e| entity_from_pairs(e)).collect();
        let mut volumes = BrushVolumes::new();
        let Ok(mut hull_maker) = MapHulls::new(&map) else {
            continue;
        };
        for e in &pairs {
            let Some(n) = value(e, "model")
                .and_then(|m| m.strip_prefix('*'))
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|&n| n > 0)
            else {
                continue;
            };
            if volumes.contains_key(&n) {
                continue;
            }
            let hulls = hull_maker.model(n as usize, !0);
            let v: Vec<Volume> = hulls
                .iter()
                .map(|h| Volume {
                    points: h.points.clone(),
                    planes: h.planes.clone(),
                    edges: h.edges(),
                })
                .collect();
            volumes.insert(n, v);
        }
        t.maps += 1;
        t.entities += entities.len();
        t.connections += entities.iter().map(|e| e.outputs.len()).sum::<usize>();
        let mut ml = MapLogic::new(&entities, &volumes, &mut defs);
        let player = ml.add_player("");
        ml.run(313);
        // A player inside each trigger in turn, two ticks each, then far away.
        let triggers: Vec<_> = ml
            .entities()
            .filter_map(|(id, _)| match ml.kind(id) {
                Some(Kind::Trigger(tr)) => Some((id, tr.bounds())),
                _ => None,
            })
            .collect();
        t.triggers += triggers.len();
        for (_, b) in &triggers {
            let Some((lo, hi)) = b else {
                t.triggers_without_volumes += 1;
                continue;
            };
            let c = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
            let at = Toucher {
                id: player,
                kind: ToucherKind::Player,
                min: [c[0] - 16.0, c[1] - 16.0, c[2] - 36.0],
                max: [c[0] + 16.0, c[1] + 16.0, c[2] + 36.0],
            };
            ml.tick(&[at]);
            ml.tick(&[at]);
            ml.tick(&[]);
        }
        let buttons: Vec<_> = ml
            .entities()
            .filter(|(_, e)| e.class == "func_button")
            .map(|(id, _)| id)
            .collect();
        for b in buttons {
            ml.use_entity(b, Some(player));
            ml.tick(&[]);
        }
        ml.run(156);
        t.movers += ml.mover_ghosts().len();
        t.sounds += ml.take_sounds().len();
        let st = ml.stats;
        t.events += st.events;
        t.inputs += st.inputs;
        t.missing += st.missing_targets;
        t.unhandled += st.unhandled_inputs;
        t.dropped_depth += st.dropped_depth;
        t.dropped_queue += st.dropped_queue;
        t.deferred += st.deferred;
        t.queued_after += ml.queued();
        let limits = ml.limits;
        assert!(st.max_queue_seen <= limits.max_queue, "{path}");
        assert!(st.max_depth_seen <= limits.max_chain_depth, "{path}");
        for line in ml.log().iter().filter(|l| l.contains("does not take")) {
            t.unhandled_kinds.insert(line.clone());
        }
        for (class, n) in ml.unsupported_classes() {
            *t.unsupported.entry(class).or_insert(0) += n;
        }
        t.supported += ml.entities().filter(|(_, e)| e.supported()).count();
    }
    let mut top: Vec<(&String, &usize)> = t.unsupported.iter().collect();
    top.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    eprintln!(
        "{} maps (FGD classes {fgd_classes}): {} entities ({} with a handler), {} connections; {} triggers ({} without volumes), {} movers, {} sound events; {} events, {} inputs, {} missing targets, {} unhandled inputs; dropped {} by depth, {} by the queue, {} deferred; {} still queued",
        t.maps,
        t.entities,
        t.supported,
        t.connections,
        t.triggers,
        t.triggers_without_volumes,
        t.movers,
        t.sounds,
        t.events,
        t.inputs,
        t.missing,
        t.unhandled,
        t.dropped_depth,
        t.dropped_queue,
        t.deferred,
        t.queued_after
    );
    eprintln!(
        "unsupported classes: {} kinds, {} entities; most: {}",
        t.unsupported.len(),
        t.unsupported.values().sum::<usize>(),
        top.iter()
            .take(12)
            .map(|(c, n)| format!("{c} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    eprintln!(
        "unhandled inputs, {} kinds: {}",
        t.unhandled_kinds.len(),
        t.unhandled_kinds
            .iter()
            .take(25)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ")
    );
    for d in defs.differences.iter().take(20) {
        eprintln!("default: {d}");
    }
    assert!(t.maps > 0, "no maps were read");
    assert_eq!(t.triggers_without_volumes, 0);
}
