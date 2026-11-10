//! `logic <out-dir> <maps/x.bsp> [--script "<code>"] [--touch] [--focus name] [--frames N] [--ticks K] [--back units]
//! [--eye x y z --yaw D --pitch D]`:
//! a map's logic run by the map-logic plugin, with its movers drawn where they are, one picture a frame
//! (`logic-frame-0000.png`, ...).
//!
//! The map's entities become the logic's (their brush models' brushes as volumes); a script engine gets the plugin's console
//! functions and runs `--script` (TorqueScript) after the map's first second, by default `mapLogicFire("<focus>", "Open");`
//! for the focus mover (`--focus`, else the first door of the map; an unnamed door is reached by its class); with `--touch`, a
//! player put by script (`mapLogicPlayer`) inside the trigger whose outputs name the focus. Each frame runs `K`
//! ticks (3 by default) and draws the world, its fixed brush entities and every mover at the pose its `MapMover` ghost record
//! gives, from `--eye` or from a camera placed before the focus mover. The sounds the logic starts are looked up through the
//! game's sound scripts and printed. `TORQUE_SOURCE_FGD` may name an FGD of the install, read over the coded defaults.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use torque_engine::{EngineBuilder, ServerStage};
use torque_format_valve::brushes::MapHulls;
use torque_format_valve::bsp::{Bsp, MapSurface, value};
use torque_format_valve::sound;
use torque_map_logic::connection::entity_from_pairs;
use torque_map_logic::defaults::Defaults;
use torque_map_logic::entities::Kind;
use torque_map_logic::fgd::EntityDefs;
use torque_map_logic::{BrushVolumes, MapLogic, MapLogicPlugin, SoundEvent, Volume, install};
use torque_ts_vm::vm::Vm;

use crate::Content;
use crate::draw::Renderer;
use crate::map::{flag_f32, invisible, surface_draw, to_world};

const SIZE: (u32, u32) = (1280, 720);

fn flag_str(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn defaults() -> Defaults {
    let Some(f) = std::env::var_os("TORQUE_SOURCE_FGD") else {
        return Defaults::coded();
    };
    let f = PathBuf::from(f);
    let (Some(dir), Some(name)) = (f.parent(), f.file_name().and_then(|n| n.to_str())) else {
        return Defaults::coded();
    };
    let (defs, warnings) = EntityDefs::load_dir(dir, name);
    println!(
        "FGD: {} entity classes ({} warnings)",
        defs.len(),
        warnings.len()
    );
    Defaults::with_fgd(defs)
}

/// A sound the logic started, as the game's files have it: a sound file, or a sound script entry's first wave.
fn resolve_sound(c: &Content, scripts: &HashMap<String, Vec<String>>, ev: &SoundEvent) -> String {
    let name = ev.sound.trim();
    let wave = if name.contains('.') {
        Some(name.to_string())
    } else {
        scripts
            .get(&name.to_ascii_lowercase())
            .and_then(|w| w.first().cloned())
    };
    let Some(wave) = wave else {
        return format!("{name}: not in the sound scripts");
    };
    let path = sound::wave_path(&wave);
    match c.files.read_path(&path) {
        Some(Ok(b)) => match sound::read_sound(&path, &b) {
            Ok(w) => format!("{name} -> {path} ({:.2} s, {} Hz)", w.duration(), w.rate),
            Err(e) => format!("{name} -> {path}: {e}"),
        },
        _ => format!("{name} -> {path}: missing"),
    }
}

/// The sound scripts' entries by lowercase name: their waves.
fn sound_scripts(c: &Content) -> HashMap<String, Vec<String>> {
    let mut out = HashMap::new();
    let Some(Ok(manifest)) = c.files.read_path("scripts/game_sounds_manifest.txt") else {
        return out;
    };
    for f in sound::read_manifest(&manifest).unwrap_or_default() {
        let Some(Ok(text)) = c.files.read_path(&f) else {
            continue;
        };
        for e in sound::read_soundscript(&text).unwrap_or_default() {
            out.insert(e.name.to_ascii_lowercase(), e.waves);
        }
    }
    out
}

/// A surface moved by a pose (map space).
fn posed(s: &MapSurface, pose: &torque_map_logic::Pose) -> MapSurface {
    let mut s = s.clone();
    let turn = torque_map_logic::volume::rotation(pose.angles);
    let rot =
        |v: [f32; 3]| [0, 1, 2].map(|i| turn[i][0] * v[0] + turn[i][1] * v[1] + turn[i][2] * v[2]);
    for v in &mut s.vertices {
        v.position = pose.point(v.position);
        v.normal = rot(v.normal);
    }
    s
}

pub fn run(c: &mut Content, args: &[String]) -> Result<(), String> {
    let usage =
        "logic <out-dir> <maps/x.bsp> [--script code] [--focus name] [--frames N] [--ticks K]";
    let out = PathBuf::from(args.first().ok_or(usage)?);
    let path = args.get(1).ok_or(usage)?.clone();
    let frames = flag_f32(args, "--frames", 0).unwrap_or(48.0).max(1.0) as usize;
    let ticks = flag_f32(args, "--ticks", 0).unwrap_or(3.0).max(1.0) as u64;
    let map: Rc<Bsp> = c
        .assets
        .load(&c.files, &path)
        .map_err(|e| format!("{path}: {e}"))?;
    if let Some(pak) = map.pak().map_err(|e| e.to_string())? {
        c.files.mount_zip_first("pak", pak);
    }
    let pairs = map.entities().map_err(|e| e.to_string())?;
    let models = map.models().map_err(|e| e.to_string())?;
    let entities: Vec<_> = pairs.iter().map(|e| entity_from_pairs(e)).collect();
    let mut volumes = BrushVolumes::new();
    let mut hull_maker = MapHulls::new(&map).map_err(|e| e.to_string())?;
    for e in &pairs {
        if let Some(n) = value(e, "model")
            .and_then(|m| m.strip_prefix('*'))
            .and_then(|n| n.parse::<u32>().ok())
            .filter(|&n| n > 0)
            && !volumes.contains_key(&n)
        {
            let hulls = hull_maker.model(n as usize, !0);
            volumes.insert(
                n,
                hulls
                    .iter()
                    .map(|h| Volume {
                        points: h.points.clone(),
                        planes: h.planes.clone(),
                        edges: h.edges(),
                    })
                    .collect(),
            );
        }
    }
    let mut d = defaults();
    let logic = MapLogic::new(&entities, &volumes, &mut d);
    for line in d.differences.iter().take(8) {
        println!("default: {line}");
    }
    // The focus: the named mover, else the first door.
    let focus = match flag_str(args, "--focus") {
        Some(n) => logic
            .find(&n)
            .into_iter()
            .find(|&id| logic.kind(id).and_then(|k| k.as_mover()).is_some())
            .ok_or(format!("no mover named {n}"))?,
        None => logic
            .entities()
            .find(|(_, e)| e.class == "func_door" || e.class == "func_door_rotating")
            .map(|(id, _)| id)
            .ok_or("the map has no door; name a mover with --focus")?,
    };
    let fe = logic.entity(focus).ok_or("no focus")?.clone();
    let target = if fe.name.is_empty() {
        fe.class.clone()
    } else {
        fe.name.clone()
    };
    // `--touch`: a player stands in the trigger whose outputs name the focus, by script.
    let touch = args.iter().any(|a| a == "--touch").then(|| {
        logic.entities().find_map(|(id, e)| {
            let names_focus = e
                .outputs
                .iter()
                .any(|(_, c)| c.target.eq_ignore_ascii_case(&target));
            match logic.kind(id) {
                Some(Kind::Trigger(t)) if names_focus => t.bounds(),
                _ => None,
            }
        })
    });
    let script = match (flag_str(args, "--script"), touch) {
        (Some(s), _) => s,
        (None, Some(Some((lo, hi)))) => {
            let c = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
            format!(
                "mapLogicPlayer(\"walker\", \"{} {} {}\", \"{} {} {}\");",
                c[0] - 16.0,
                c[1] - 16.0,
                c[2] - 36.0,
                c[0] + 16.0,
                c[1] + 16.0,
                c[2] + 36.0
            )
        }
        (None, Some(None)) => return Err(format!("no trigger names {target}")),
        (None, None) => format!("mapLogicFire(\"{target}\", \"Open\");"),
    };
    println!(
        "{path}: {} entities, {} connections, {} movers; focus {} {:?}; script: {script}",
        entities.len(),
        entities.iter().map(|e| e.outputs.len()).sum::<usize>(),
        logic.mover_ghosts().len(),
        fe.class,
        fe.name
    );

    // The script engine with the plugin's functions, the logic on it.
    let engine = EngineBuilder::new()
        .add_plugins(MapLogicPlugin)
        .build()
        .map_err(|e| e.to_string())?;
    let mut vm = Vm::new();
    for &stage in ServerStage::ALL {
        engine.replay(stage, &mut vm).map_err(|e| e.to_string())?;
    }
    let shared = install(&mut vm, logic);
    vm.eval(b"mapLogicTick(31);").map_err(|e| e.to_string())?;
    vm.eval(script.as_bytes()).map_err(|e| e.to_string())?;

    // The fixed geometry: the world and every drawn brush entity that is not a mover.
    let movers: HashSet<u32> = shared
        .borrow()
        .logic
        .as_ref()
        .map(|ml| {
            ml.mover_ghosts()
                .into_iter()
                .map(|(_, g)| g.model)
                .collect()
        })
        .unwrap_or_default();
    let mut fixed = map
        .build_lit(
            &map.world_faces().map_err(|e| e.to_string())?,
            [0.0; 3],
            None,
        )
        .map_err(|e| e.to_string())?;
    let mut mover_surfaces: HashMap<u32, Vec<MapSurface>> = HashMap::new();
    for e in &pairs {
        let Some(model) = value(e, "model") else {
            continue;
        };
        let Some(n) = model.strip_prefix('*').and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let Some(m) = models.get(n as usize) else {
            continue;
        };
        if n == 0 || invisible(e, &[]) {
            continue;
        }
        let faces = map.model_faces(m).map_err(|e| e.to_string())?;
        if movers.contains(&n) {
            mover_surfaces.insert(
                n,
                map.build_lit(&faces, [0.0; 3], None)
                    .map_err(|e| e.to_string())?,
            );
        } else {
            let origin = value(e, "origin")
                .and_then(torque_format_valve::bsp::vec3)
                .unwrap_or([0.0; 3]);
            fixed.extend(
                map.build_lit(&faces, origin, None)
                    .map_err(|e| e.to_string())?,
            );
        }
    }
    let mut r = Renderer::new(SIZE)?;
    let mut fixed_planes = Vec::new();
    let mut looks = HashMap::new();
    for s in &fixed {
        if c.material(&s.material).is_some_and(|m| m.tool()) {
            continue;
        }
        let look = *looks
            .entry(s.material.clone())
            .or_insert_with(|| r.look(c, std::slice::from_ref(&s.material), false));
        fixed_planes.push(surface_draw(s, &look, None, false));
    }
    for list in mover_surfaces.values() {
        for s in list {
            looks
                .entry(s.material.clone())
                .or_insert_with(|| r.look(c, std::slice::from_ref(&s.material), false));
        }
    }

    // The camera: given, or before the focus mover's closed pose, across its thinnest side.
    let (eye, aim) = match (
        flag_f32(args, "--eye", 0),
        flag_f32(args, "--eye", 1),
        flag_f32(args, "--eye", 2),
    ) {
        (Some(x), Some(y), Some(z)) => {
            let (p, yw) = (
                flag_f32(args, "--pitch", 0).unwrap_or(0.0).to_radians(),
                flag_f32(args, "--yaw", 0).unwrap_or(0.0).to_radians(),
            );
            let fwd = [p.cos() * yw.cos(), p.cos() * yw.sin(), -p.sin()];
            (
                [x, y, z],
                [x + fwd[0] * 100.0, y + fwd[1] * 100.0, z + fwd[2] * 100.0],
            )
        }
        _ => {
            let vols = volumes
                .get(&fe.model.unwrap_or(0))
                .cloned()
                .unwrap_or_default();
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            for v in &vols {
                let (a, b) = v.bounds();
                for k in 0..3 {
                    lo[k] = lo[k].min(a[k] + fe.origin[k]);
                    hi[k] = hi[k].max(b[k] + fe.origin[k]);
                }
            }
            if vols.is_empty() {
                lo = fe.origin;
                hi = fe.origin;
            }
            let center = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
            let thin = if hi[0] - lo[0] < hi[1] - lo[1] { 0 } else { 1 };
            let back = flag_f32(args, "--back", 0).unwrap_or(150.0);
            let mut eye = center;
            eye[thin] -= back;
            eye[2] = lo[2] + 56.0;
            (eye, [center[0], center[1], lo[2] + 48.0])
        }
    };
    println!("camera: eye {eye:?} aim {aim:?}");
    let scripts = sound_scripts(c);
    let (weye, waim) = (to_world(eye), to_world(aim));
    for f in 0..frames {
        let (ghosts, sounds, fraction) = {
            let mut g = shared.borrow_mut();
            let touchers: Vec<_> = g.players.iter().map(|(_, t)| *t).collect();
            let ml = g.logic.as_mut().ok_or("no logic")?;
            ml.observe_outputs(true);
            if f > 0 {
                for _ in 0..ticks {
                    ml.tick(&touchers);
                }
            }
            for o in ml.take_fired() {
                let name = ml
                    .entity(o.entity)
                    .map(|e| e.name.clone())
                    .unwrap_or_default();
                println!("output {name}.{}", o.output);
            }
            let fraction = ml
                .kind(focus)
                .and_then(|k| k.as_mover())
                .map_or(0.0, |m| m.frac);
            (ml.mover_ghosts(), ml.take_sounds(), fraction)
        };
        for s in &sounds {
            println!("sound {:?}: {}", s.action, resolve_sound(c, &scripts, s));
        }
        let tick = shared
            .borrow()
            .logic
            .as_ref()
            .map_or(0, |ml| ml.tick_count()) as f64;
        let mut planes = fixed_planes.clone();
        for (_, g) in &ghosts {
            // As a client takes a record off the wire.
            let Some(g) = g.sanitized(models.len() as u32) else {
                continue;
            };
            if !g.visible {
                continue;
            }
            let pose = g.pose_at(tick);
            for s in mover_surfaces.get(&g.model).into_iter().flatten() {
                if c.material(&s.material).is_some_and(|m| m.tool()) {
                    continue;
                }
                let Some(look) = looks.get(&s.material) else {
                    continue;
                };
                planes.push(surface_draw(&posed(s, &pose), look, None, false));
            }
        }
        r.scene.frame.planes = planes;
        r.scene.frame.planes_generation += 1;
        let file = out.join(format!("logic-frame-{f:04}.png"));
        r.shot(weye, waim, if f == 0 { 16 } else { 4 }, &file)?;
        println!("frame {f}: tick {tick}, focus at {fraction:.3}");
    }
    let g = shared.borrow();
    if let Some(ml) = g.logic.as_ref() {
        let st = ml.stats;
        println!(
            "logic: {} events, {} inputs, {} missing targets, {} unhandled inputs, {} dropped, {} queued",
            st.events,
            st.inputs,
            st.missing_targets,
            st.unhandled_inputs,
            st.dropped_depth + st.dropped_queue,
            ml.queued()
        );
    }
    c.files.unmount("pak");
    Ok(())
}
