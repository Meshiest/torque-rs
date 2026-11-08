//! `map <out-dir> <maps/x.bsp> [--baked] [--lightmap-only] [--hide classes] [--shots N] [--eye x y z --yaw D --pitch D] [--probe x y]`: a map's brush faces,
//! displacements (their two textures blended), brush entities, static props and prop entities, drawn from above, from its four
//! sides and from its spawn points (or from one given eye, in the map's units); `--probe` prints the surfaces' heights at a
//! point. With `--viewmodel <model> --sequence <name> [--frames N] [--flip]` and `--eye`, a first-person weapon plays the sequence at the
//! eye, one picture a frame (`view-frame-0000.png`, ...), and the sequence's sound events mixed into `view.wav`. With
//! `--walk SECONDS [--walk-height H]`, a walk through the map's soundscape entities: frames and `walk.wav`, its soundscapes and
//! ambient sounds playing. The map's pak lump is mounted before the game's files while it loads. The sun
//! comes from the map's `light_environment`; `--baked` draws the map's lightmaps (an atlas the renderer samples per pixel) and lights props by its ambient light cubes, instead of the
//! renderer's lighting.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use torque_format_valve::bsp::{
    Atlas, Bsp, Entity, FULL_LIGHT_TEXEL, GAIN_LIGHT, GAIN_TEXEL, LightCube, MapSurface,
    cube_light, direct_lights, value, vec3,
};
use torque_format_valve::load_model;
use torque_format_valve::mdl::Mdl;
use torque_format_valve::model::{Choice, INCH, StudioModel};
use torque_modelcore::math::{self, Mat4};
use torque_render_scene::Blend;
use torque_render_scene::hd::{
    HdDraw, HdVertex, Lightmap, half, lightmap_uv, linear_to_srgb, material, oct_decode,
    srgb_to_linear,
};

use crate::Content;
use crate::audio::{Mix, ScapePlayer, SoundBank, soundscape_at, start_ambients};
use crate::draw::{Look, Renderer, bounds, normal, object};

const SIZE: (u32, u32) = (1600, 900);

/// A point of the map (inches, x forward, y left) in the world (meters, Torque's axes).
fn to_world(p: [f32; 3]) -> [f32; 3] {
    [-p[1] * INCH, p[0] * INCH, p[2] * INCH]
}

fn to_world_dir(n: [f32; 3]) -> [f32; 3] {
    [-n[1], n[0], n[2]]
}

/// The rotation of Source angles (pitch, yaw, roll in degrees) in the map's frame, as a 4 x 4 at `origin`.
fn placement(origin: [f32; 3], angles: [f32; 3]) -> Mat4 {
    let [p, y, r] = angles.map(f32::to_radians);
    let (sp, cp) = p.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sr, cr) = r.sin_cos();
    [
        cp * cy,
        sr * sp * cy - cr * sy,
        cr * sp * cy + sr * sy,
        origin[0],
        cp * sy,
        sr * sp * sy + cr * cy,
        cr * sp * sy - sr * cy,
        origin[1],
        -sp,
        sr * cp,
        cr * cp,
        origin[2],
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

fn flag_f32(args: &[String], name: &str, k: usize) -> Option<f32> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1 + k))
        .and_then(|v| v.parse().ok())
}

/// Brush entities that are volumes, not drawn things: triggers and environment volumes, entities set not to render
/// (`rendermode` 10), and the classes `--hide` lists (comma-separated). Volumes whose faces carry tool materials are left out
/// with those materials anyway.
fn invisible(e: &Entity, hide: &[String]) -> bool {
    let c = value(e, "classname").unwrap_or("").to_ascii_lowercase();
    c.starts_with("trigger")
        || c.starts_with("env_")
        || value(e, "rendermode").is_some_and(|m| m.trim() == "10")
        || hide.iter().any(|h| h.eq_ignore_ascii_case(&c))
}

pub fn run(c: &mut Content, args: &[String]) -> Result<(), String> {
    let usage = "map <out-dir> <maps/x.bsp> [--baked] [--shots N] [--eye x y z --yaw D --pitch D]";
    let out = PathBuf::from(args.first().ok_or(usage)?);
    let path = args.get(1).ok_or(usage)?.clone();
    let baked = args.iter().any(|a| a == "--baked");
    let hide: Vec<String> = args
        .iter()
        .position(|a| a == "--hide")
        .and_then(|i| args.get(i + 1))
        .map(|l| l.split(',').map(|c| c.trim().to_string()).collect())
        .unwrap_or_default();
    let shots = flag_f32(args, "--shots", 0).unwrap_or(4.0) as usize;
    let t0 = std::time::Instant::now();
    let map: Rc<Bsp> = c
        .assets
        .load(&c.files, &path)
        .map_err(|e| format!("{path}: {e}"))?;
    if let Some(pak) = map.pak().map_err(|e| e.to_string())? {
        println!("pak lump: {} files", pak.entries().len());
        c.files.mount_zip_first("pak", pak);
    }
    let ents = map.entities().map_err(|e| e.to_string())?;
    let leaves = map.leaves().map_err(|e| e.to_string())?;
    // The 3D sky's area (around its camera), left out of the pictures.
    let sky_area = ents
        .iter()
        .find(|e| value(e, "classname") == Some("sky_camera"))
        .and_then(|e| value(e, "origin").and_then(vec3))
        .and_then(|o| map.leaf_at(o).ok().flatten())
        .and_then(|l| leaves.get(l))
        .map(|l| l.area);
    let areas = map.face_areas().map_err(|e| e.to_string())?;
    let world: Vec<usize> = map
        .world_faces()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|&f| sky_area.is_none() || areas.get(f).copied().flatten() != sky_area)
        .collect();
    let models = map.models().map_err(|e| e.to_string())?;
    // The brush entities drawn: their faces and offsets.
    let mut entity_faces: Vec<(Vec<usize>, [f32; 3])> = Vec::new();
    for e in &ents {
        let Some(model) = value(e, "model") else {
            continue;
        };
        let Some(n) = model
            .strip_prefix('*')
            .and_then(|n| n.parse::<usize>().ok())
        else {
            continue;
        };
        if n == 0 || invisible(e, &hide) {
            continue;
        }
        let Some(m) = models.get(n) else { continue };
        let origin = value(e, "origin").and_then(vec3).unwrap_or([0.0; 3]);
        let faces = map.model_faces(m).map_err(|e| e.to_string())?;
        entity_faces.push((faces, origin));
    }
    let brush_entities = entity_faces.len();
    // With --baked, every drawn face's lightmap packed into one atlas, the scene's lightmap.
    let atlas = if baked {
        let mut all = world.clone();
        all.extend(entity_faces.iter().flat_map(|(f, _)| f.iter().copied()));
        let a = map.lightmap_atlas(&all).map_err(|e| e.to_string())?;
        println!(
            "lightmap atlas: {} x {} texels, {} faces left out",
            a.width, a.height, a.left_out
        );
        Some(a)
    } else {
        None
    };
    let mut surfaces = map
        .build_lit(&world, [0.0; 3], atlas.as_ref())
        .map_err(|e| e.to_string())?;
    for (faces, origin) in &entity_faces {
        surfaces.extend(
            map.build_lit(faces, *origin, atlas.as_ref())
                .map_err(|e| e.to_string())?,
        );
    }
    if let (Some(x), Some(y)) = (flag_f32(args, "--probe", 0), flag_f32(args, "--probe", 1)) {
        // The heights of every surface above or below a point of the map (a check of the built geometry).
        for s in &surfaces {
            for t in s.indices.as_chunks::<3>().0 {
                let [a, b, cc] = [0, 1, 2].map(|k| s.vertices[t[k] as usize].position);
                let d = (b[0] - a[0]) * (cc[1] - a[1]) - (b[1] - a[1]) * (cc[0] - a[0]);
                if d.abs() < 1e-6 {
                    continue;
                }
                let u = ((x - a[0]) * (cc[1] - a[1]) - (y - a[1]) * (cc[0] - a[0])) / d;
                let v = ((b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0])) / d;
                if u >= 0.0 && v >= 0.0 && u + v <= 1.0 {
                    let z = a[2] + u * (b[2] - a[2]) + v * (cc[2] - a[2]);
                    println!(
                        "probe: {} at z {z:.1}{}",
                        s.material,
                        if s.displacement {
                            " (displacement)"
                        } else {
                            ""
                        }
                    );
                }
            }
        }
    }
    let mut r = Renderer::new(SIZE)?;
    // The sun of the map's light_environment: it shines along its yaw, down by its pitch.
    if let Some(env) = ents
        .iter()
        .find(|e| value(e, "classname") == Some("light_environment"))
    {
        let angles = value(env, "angles")
            .and_then(vec3)
            .unwrap_or([0.0, 0.0, 0.0]);
        let pitch = value(env, "pitch")
            .and_then(|p| p.trim().parse::<f32>().ok())
            .unwrap_or(angles[0]);
        let elev = pitch.abs().clamp(15.0, 80.0).to_radians();
        let yaw = angles[1].to_radians();
        let towards = [-yaw.cos() * elev.cos(), -yaw.sin() * elev.cos(), elev.sin()];
        r.set_sun(to_world_dir(towards));
    }
    let mut planes = Vec::new();
    let mut tris = 0usize;
    let mut skipped_tools = 0;
    for s in &surfaces {
        let vmt = c.material(&s.material);
        if vmt.as_ref().is_some_and(|m| m.tool()) {
            skipped_tools += 1;
            continue;
        }
        let water = vmt.as_ref().is_some_and(|m| m.water());
        let mut look = r.look(c, std::slice::from_ref(&s.material), false);
        if water {
            look = Look {
                texture: None,
                blend: Blend::Alpha,
                two_sided: true,
                unlit: false,
                tint: [40, 70, 80, 170],
                cutout: false,
            };
        }
        tris += s.indices.len() / 3;
        planes.push(surface_draw(s, &look, atlas.as_ref(), false));
        if s.displacement
            && let Some(m) = &vmt
            && m.base_texture2().is_some()
        {
            let second = r.look(c, std::slice::from_ref(&s.material), true);
            planes.push(surface_draw(s, &second, atlas.as_ref(), true));
        }
    }
    // Static props and prop entities.
    let mut cache: HashMap<String, Option<StudioModel>> = HashMap::new();
    let props = map.static_props().map_err(|e| e.to_string())?;
    let mut placed: Vec<(String, [f32; 3], [f32; 3], usize)> = props
        .iter()
        .filter(|p| {
            sky_area.is_none()
                || !p
                    .leaves
                    .iter()
                    .any(|&l| leaves.get(usize::from(l)).map(|l| l.area) == sky_area)
        })
        .map(|p| {
            (
                p.model.clone(),
                p.origin,
                p.angles,
                usize::try_from(p.skin).unwrap_or(0),
            )
        })
        .collect();
    for e in &ents {
        let class = value(e, "classname").unwrap_or("");
        if !(class.starts_with("prop_dynamic")
            || class.starts_with("prop_physics")
            || class == "prop_door_rotating")
        {
            continue;
        }
        let (Some(m), Some(o)) = (value(e, "model"), value(e, "origin").and_then(vec3)) else {
            continue;
        };
        let a = value(e, "angles").and_then(vec3).unwrap_or([0.0; 3]);
        let skin = value(e, "skin").and_then(|s| s.parse().ok()).unwrap_or(0);
        placed.push((m.to_string(), o, a, skin));
    }
    let g = math::trs(
        [0.0; 3],
        [
            0.0,
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
        ],
        [INCH; 3],
    );
    let g_inv = math::inverse(&g);
    let mut objects = Vec::new();
    let mut missing = 0;
    let mut lit_props = 0;
    // The tree and the world lights, for the props' direct light.
    let direct = match &atlas {
        Some(_) => Some((
            map.tracer().map_err(|e| e.to_string())?,
            map.world_lights().map_err(|e| e.to_string())?,
        )),
        None => None,
    };
    // The ambient cubes read once for all the props.
    let ambient = atlas.as_ref().and_then(|_| map.ambient().ok());
    for (i, (model, origin, angles, skin)) in placed.iter().enumerate() {
        let key = format!("{}#{skin}", model.to_ascii_lowercase());
        let m = cache.entry(key).or_insert_with(|| {
            let choice = Choice {
                skin: *skin,
                body: Vec::new(),
            };
            load_model(
                &c.assets,
                &c.files,
                &model.to_ascii_lowercase(),
                &choice,
                &mut |_| false,
            )
            .ok()
        });
        let Some(m) = m else {
            missing += 1;
            continue;
        };
        let place = math::mul(&g, &math::mul(&placement(*origin, *angles), &g_inv));
        let mut draws = r.model_draws(c, m, None, 0.0, &place, "prop");
        // Baked: lit as the map's models are: the ambient light cube at the prop (a little above its origin, off the floor it
        // stands on) and the direct lights that reach it, drawn with the map's surfaces.
        let lit_at = [origin[0], origin[1], origin[2] + 16.0];
        if let (Some(a), Some((tracer, lights))) = (&atlas, &direct)
            && let Some(cube) = ambient.as_ref().and_then(|am| am.at(lit_at))
        {
            let reach = direct_lights(tracer, lights, lit_at, 0.02);
            for d in &mut draws {
                prop_lit(d, &cube, &reach, a);
            }
            planes.append(&mut draws);
            lit_props += 1;
            continue;
        }
        if !draws.is_empty() {
            objects.push(object(10 + i as u32, draws));
        }
    }
    if let Some((tracer, _)) = &direct
        && tracer.exhausted()
    {
        println!(
            "warning: the lighting traces' budget ran out; later props are lit by the ambient light only"
        );
    }
    println!(
        "{path}: {} surfaces ({tris} triangles, {brush_entities} brush entities, {skipped_tools} tool materials left out), {} props placed ({lit_props} by the map's light; {} models, {missing} missing), sky area {sky_area:?}, built in {:.1} s",
        planes.len(),
        objects.len() + lit_props,
        cache.len(),
        t0.elapsed().as_secs_f32()
    );
    // A first-person weapon (`--viewmodel <model> --sequence <name> [--frames N]`), drawn at a given eye.
    let flag_str = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let vm_frames = flag_f32(args, "--frames", 0).unwrap_or(48.0) as usize;
    let viewmodel = match flag_str("--viewmodel") {
        None => None,
        Some(p) => {
            let seq = flag_str("--sequence").unwrap_or_default();
            let m = load_model(&c.assets, &c.files, &p, &Choice::default(), &mut |s| {
                s.label.eq_ignore_ascii_case(&seq)
            })
            .map_err(|e| format!("{p}: {e}"))?;
            let anim = m
                .model
                .animations
                .iter()
                .position(|a| a.name.eq_ignore_ascii_case(&seq))
                .ok_or(format!("{p} has no sequence {seq}"))?;
            Some((m, anim))
        }
    };
    // `--lightmap-only`: every surface white, so the pictures show the light alone (a check of the lightmaps).
    if args.iter().any(|a| a == "--lightmap-only") {
        for d in planes
            .iter_mut()
            .chain(objects.iter_mut().flat_map(|o| o.draws.iter_mut()))
        {
            d.texture = None;
            let surface = d.label.starts_with("map ");
            for v in &mut d.vertices {
                v.material &= !(material::TEX_ENV_MASK << material::TEX_ENV_SHIFT);
                if surface {
                    v.color = [255, 255, 255, v.color[3]];
                }
            }
        }
    }
    let (lo, hi) = bounds(&planes);
    if let Some(a) = &atlas {
        r.scene.frame.lightmap = Some(lightmap_of(a));
        r.scene.frame.lightmap_generation += 1;
    }
    r.scene.frame.planes = planes;
    r.scene.frame.planes_generation += 1;
    r.scene.frame.objects = objects;
    let center = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
    let extent = (0..2).map(|k| hi[k] - lo[k]).fold(0.0f32, f32::max);
    let mut views: Vec<(String, [f32; 3], [f32; 3])> = Vec::new();
    if let (Some(x), Some(y), Some(z)) = (
        flag_f32(args, "--eye", 0),
        flag_f32(args, "--eye", 1),
        flag_f32(args, "--eye", 2),
    ) {
        let yaw = flag_f32(args, "--yaw", 0).unwrap_or(0.0);
        let pitch = flag_f32(args, "--pitch", 0).unwrap_or(0.0);
        views.push(("view".into(), [x, y, z], [pitch, yaw, 0.0]));
    } else {
        views.push((
            "overview".into(),
            [
                center[0] - extent * 0.45,
                center[1] - extent * 0.6,
                hi[2] + extent * 0.35,
            ],
            [f32::NAN; 3],
        ));
        // Lower views over the map from its four sides.
        for (k, (dx, dy)) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .into_iter()
            .enumerate()
        {
            views.push((
                format!("side-{k}"),
                [
                    center[0] + dx * extent * 0.32,
                    center[1] + dy * extent * 0.32,
                    hi[2] + extent * 0.04,
                ],
                [f32::NAN; 3],
            ));
        }
        // A view from each kind of player spawn (`info_player_*`), apart from the ones before it.
        let mut seen: Vec<[f32; 3]> = Vec::new();
        let mut classes: Vec<String> = Vec::new();
        for e in &ents {
            let Some(class) = value(e, "classname").map(str::to_ascii_lowercase) else {
                continue;
            };
            // The class names a picture file: keep it to plain characters.
            if !class.starts_with("info_player_")
                || !class
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || classes.contains(&class)
            {
                continue;
            }
            let (Some(o), a) = (
                value(e, "origin").and_then(vec3),
                value(e, "angles").and_then(vec3).unwrap_or([0.0; 3]),
            ) else {
                continue;
            };
            if seen
                .iter()
                .any(|s| (s[0] - o[0]).abs() + (s[1] - o[1]).abs() < 600.0)
            {
                continue;
            }
            seen.push(o);
            classes.push(class.clone());
            views.push((
                format!("{class}-{}", seen.len()),
                [o[0], o[1], o[2] + 64.0],
                [8.0, a[1], 0.0],
            ));
        }
    }
    // `--walk SECONDS [--walk-height H]`: a path through the map's soundscape entities (each the next nearest), the camera and
    // the listener along it, the soundscapes and ambient sounds playing: frames and one mix.
    if let Some(seconds) = flag_f32(args, "--walk", 0) {
        let height = flag_f32(args, "--walk-height", 0).unwrap_or(160.0);
        // The soundscape triggers: each a brush volume (its model's box moved by its origin) and the soundscape it names.
        let triggers: Vec<([f32; 3], [f32; 3], String)> = ents
            .iter()
            .filter(|e| {
                value(e, "classname").is_some_and(|c| c.eq_ignore_ascii_case("trigger_soundscape"))
            })
            .filter_map(|e| {
                let n: usize = value(e, "model")?.strip_prefix('*')?.parse().ok()?;
                let m = models.get(n)?;
                let o = value(e, "origin").and_then(vec3).unwrap_or([0.0; 3]);
                Some((
                    [0, 1, 2].map(|k| m.mins[k] + o[k]),
                    [0, 1, 2].map(|k| m.maxs[k] + o[k]),
                    value(e, "soundscape")?.to_string(),
                ))
            })
            .collect();
        let mut points: Vec<[f32; 3]> = ents
            .iter()
            .filter(|e| {
                value(e, "classname").is_some_and(|c| c.eq_ignore_ascii_case("env_soundscape"))
            })
            .filter_map(|e| value(e, "origin").and_then(vec3))
            .chain(
                triggers
                    .iter()
                    .map(|(lo, hi, _)| [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]))),
            )
            .collect();
        if points.len() < 2 {
            return Err(format!(
                "{path} has fewer than two soundscape entities to walk between"
            ));
        }
        let mut path = vec![points.remove(0)];
        while !points.is_empty() && path.len() < 8 {
            let last = *path.last().unwrap_or(&[0.0; 3]);
            let (k, _) = points
                .iter()
                .enumerate()
                .map(|(k, p)| (k, (p[0] - last[0]).powi(2) + (p[1] - last[1]).powi(2)))
                .fold((0, f32::INFINITY), |a, b| if b.1 < a.1 { b } else { a });
            path.push(points.remove(k));
        }
        let lengths: Vec<f32> = path
            .windows(2)
            .map(|w| {
                ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2))
                    .sqrt()
                    .max(1.0)
            })
            .collect();
        let total: f32 = lengths.iter().sum();
        let at_distance = |d: f32| -> ([f32; 3], [f32; 3]) {
            let mut d = d.clamp(0.0, total);
            for (k, l) in lengths.iter().enumerate() {
                if d <= *l || k + 1 == lengths.len() {
                    let u = (d / l).clamp(0.0, 1.0);
                    let (a, b) = (path[k], path[k + 1]);
                    let p = [
                        a[0] + (b[0] - a[0]) * u,
                        a[1] + (b[1] - a[1]) * u,
                        a[2] + (b[2] - a[2]) * u + height,
                    ];
                    let dir = [(b[0] - a[0]) / l, (b[1] - a[1]) / l, 0.0];
                    return (p, dir);
                }
                d -= l;
            }
            (path[0], [1.0, 0.0, 0.0])
        };
        let mut bank = SoundBank::load(c);
        let mut mix = Mix::new();
        let mut scape = ScapePlayer::new();
        let fps = 30.0;
        let frames = (seconds * fps) as usize;
        r.world.reset_history();
        let mut ambient_started = false;
        let mut look_dir = at_distance(0.0).1;
        for f in 0..frames {
            let t = f as f32 / fps;
            let (p, dir) = at_distance(total * t / seconds);
            // The view turns towards the way ahead gently.
            for k in 0..2 {
                look_dir[k] += (dir[k] - look_dir[k]) * 0.08;
            }
            mix.listen(p, look_dir);
            // A soundscape trigger the listener is in (its box, at the listener's feet) names its soundscape entity; else the
            // nearest soundscape in range.
            let feet = [p[0], p[1], p[2] - height];
            let triggered = triggers
                .iter()
                .find(|(lo, hi, _)| (0..3).all(|k| feet[k] >= lo[k] && feet[k] <= hi[k]))
                .and_then(|(_, _, name)| {
                    ents.iter().find(|e| {
                        value(e, "targetname").is_some_and(|t| t.eq_ignore_ascii_case(name))
                    })
                });
            if let Some(e) = triggered.or_else(|| soundscape_at(&ents, p)) {
                let before = scape.name.clone();
                scape.start(&mut mix, &mut bank, c, e, &ents, t);
                if scape.name != before {
                    println!("{t:.1} s: soundscape {:?}", scape.name);
                    ambient_started = false;
                }
            }
            if !ambient_started {
                let n = start_ambients(&mut mix, &mut bank, c, &ents);
                if f == 0 {
                    println!("ambient sounds: {n}");
                }
                ambient_started = true;
            }
            scape.update(&mut mix, &mut bank, c, t);
            mix.advance(1.0 / fps);
            let w = to_world(p);
            let d = to_world_dir([look_dir[0], look_dir[1], -0.12 - height / 1200.0]);
            let file = out.join(format!("walk-frame-{f:04}.png"));
            r.shot(
                w,
                [w[0] + d[0] * 10.0, w[1] + d[1] * 10.0, w[2] + d[2] * 10.0],
                if f == 0 { 16 } else { 3 },
                &file,
            )?;
        }
        mix.write_wav(&out.join("walk.wav"))?;
        println!(
            "wrote {frames} walk frames and walk.wav to {}",
            out.display()
        );
        c.files.unmount("pak");
        return Ok(());
    }
    for (name, eye, angles) in views.into_iter().take(shots.max(1)) {
        let (eye, target) = if angles[0].is_nan() {
            (eye, [center[0], center[1], lo[2] + (hi[2] - lo[2]) * 0.3])
        } else {
            let (p, y) = (angles[0].to_radians(), angles[1].to_radians());
            let fwd = [p.cos() * y.cos(), p.cos() * y.sin(), -p.sin()];
            let w = to_world(eye);
            let d = to_world_dir(fwd);
            (
                w,
                [w[0] + d[0] * 10.0, w[1] + d[1] * 10.0, w[2] + d[2] * 10.0],
            )
        };
        println!("{name}: eye {eye:?} angles {angles:?}");
        r.world.reset_history();
        if let (Some(vm), false) = (&viewmodel, angles[0].is_nan()) {
            // A first-person weapon at the eye, playing one sequence: one picture a frame.
            let (m, anim) = vm;
            let src_eye = [eye[1] / INCH, -eye[0] / INCH, eye[2] / INCH];
            // `--flip`: a weapon modeled for the left hand, mirrored to the right as its game draws it.
            let flip = args.iter().any(|a| a == "--flip");
            let mut at_eye = placement(src_eye, angles);
            if flip {
                at_eye = math::mul(
                    &at_eye,
                    &math::trs([0.0; 3], math::NO_TURN, [1.0, -1.0, 1.0]),
                );
            }
            let place = math::mul(&g, &math::mul(&at_eye, &g_inv));
            let dur = m.model.animations[*anim].duration;
            let base = r.scene.frame.objects.clone();
            // The sequence's sound events, played as their frames come (one mix beside the pictures).
            let seq_name = m.model.animations[*anim].name.clone();
            let events: Vec<(f32, String)> = flag_str("--viewmodel")
                .and_then(|p| c.assets.load::<Mdl>(&c.files, &p).ok())
                .and_then(|h| {
                    h.sequences
                        .iter()
                        .find(|q| q.label.eq_ignore_ascii_case(&seq_name))
                        .cloned()
                })
                .map(|q| {
                    q.events
                        .iter()
                        .filter(|e| e.is_sound())
                        .map(|e| (e.cycle * dur, e.options.clone()))
                        .collect()
                })
                .unwrap_or_default();
            println!("sound events: {events:?}");
            let mut bank = SoundBank::load(c);
            let mut mix = Mix::new();
            mix.listen(
                src_eye,
                [
                    angles[1].to_radians().cos(),
                    angles[1].to_radians().sin(),
                    0.0,
                ],
            );
            let step = 1.0 / 30.0;
            for f in 0..vm_frames {
                let t = dur * f as f32 / vm_frames as f32;
                let t_next = t + step;
                for (at, name) in &events {
                    if *at >= t && *at < t_next {
                        mix.play_entry(&mut bank, c, name, None, false);
                    }
                }
                mix.advance(step);
                let mut draws = r.model_draws(c, m, Some(*anim), t, &place, "viewmodel");
                for d in &mut draws {
                    for v in &mut d.vertices {
                        v.material |= material::VIEW_ONLY;
                    }
                    if flip {
                        // A mirror turns the triangles' fronts over.
                        for t in d.indices.as_chunks_mut::<3>().0 {
                            t.swap(1, 2);
                        }
                    }
                }
                let mut objects = base.clone();
                objects.push(object(1, draws));
                r.scene.frame.objects = objects;
                let file = out.join(format!("{name}-frame-{f:04}.png"));
                r.shot(eye, target, if f == 0 { 16 } else { 3 }, &file)?;
            }
            r.scene.frame.objects = base;
            mix.advance(0.5);
            mix.write_wav(&out.join(format!("{name}.wav")))?;
            println!(
                "wrote {vm_frames} frames of the weapon's {} from {name}, and its sound",
                m.model.animations[*anim].name
            );
            continue;
        }
        let file = out.join(format!("{name}.png"));
        r.shot(eye, target, 24, &file)?;
        println!("wrote {}", file.display());
    }
    c.files.unmount("pak");
    Ok(())
}

/// The light of a lightmap texel as the scene's lightmap holds it: linear light, capped where the format's own lightmaps stop.
/// The format stores a lightmap texel as its light's 1/2.2 power over its overbright factor of 2 (`BuildGammaTable`'s
/// `lineartolightmap` in the public SDK 2013 `mathlib/color_conversion.cpp`) and its shaders scale it back by 2 in gamma space,
/// so light reaches at most 2 in gamma space: `2^2.2` (about 4.6) in linear light. Light 1 shows the texture as it is.
const MAX_LINEAR_LIGHT: f32 = 4.594_794;

/// The scene's lightmap of an atlas (half floats).
fn lightmap_of(a: &Atlas) -> Lightmap {
    let texels = a
        .texels
        .iter()
        .map(|c| {
            let [r, g, b] = c.map(|v| half(v.clamp(0.0, MAX_LINEAR_LIGHT)));
            [r, g, b, half(1.0)]
        })
        .collect();
    Lightmap {
        width: a.width,
        height: a.height,
        texels: std::sync::Arc::new(texels),
    }
}

/// A texel place of the atlas as a vertex's lightmap word.
fn atlas_word(a: &Atlas, texel: [f32; 2]) -> u32 {
    lightmap_uv([texel[0] / a.width as f32, texel[1] / a.height as f32])
}

/// A linear light (0..1) as a vertex color channel (sRGB).
fn srgb_byte(linear: f32) -> u8 {
    (linear_to_srgb(linear.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8
}

/// A prop's draw lit by a light cube and the direct lights that reach it: made a lightmapped plane whose vertex colors carry
/// that light along each normal over the atlas's gain texel's light.
fn prop_lit(d: &mut HdDraw, cube: &LightCube, direct: &[([f32; 3], [f32; 3])], a: &Atlas) {
    let gain = atlas_word(a, GAIN_TEXEL);
    for v in &mut d.vertices {
        let n = oct_decode(v.normal);
        // The world's normal back in the map's frame (the inverse of to_world_dir).
        let n = [n[1], -n[0], n[2]];
        let mut light = cube_light(cube, n);
        for (dir, l) in direct {
            let facing = (n[0] * dir[0] + n[1] * dir[1] + n[2] * dir[2]).max(0.0);
            for k in 0..3 {
                light[k] += l[k] * facing;
            }
        }
        for (c, l) in v.color.iter_mut().zip(light) {
            let tint = srgb_to_linear(f32::from(*c) / 255.0);
            *c = srgb_byte(tint * l / GAIN_LIGHT);
        }
        v.material = (v.material & !(0x3 << material::KIND_SHIFT) & !material::UNLIT)
            | (material::KIND_PLANE << material::KIND_SHIFT)
            | material::LIGHTMAP;
        v.extra = gain;
    }
}

/// A surface's draw: its first texture, or (`layer`) its second blended over by the displacement's alpha. With an atlas each
/// vertex is lit by its place in it (a face whose lightmap is not there by its own sampled light; a face without one fully).
fn surface_draw(s: &MapSurface, look: &Look, atlas: Option<&Atlas>, layer: bool) -> HdDraw {
    let mut d = look.draw(format!("map {}", s.material));
    if s.displacement {
        // Displacements are walked on from either side of their grid in some maps; draw both.
        d.cull = torque_render_scene::Cull::None;
    }
    let mut word = look.word(material::KIND_PLANE);
    if layer {
        word |= material::LAYER;
        d.blend = Blend::Alpha;
        d.depth_write = false;
    }
    if atlas.is_some() {
        word = (word & !material::UNLIT) | material::LIGHTMAP;
    }
    d.vertices = s
        .vertices
        .iter()
        .map(|v| {
            let mut color = look.tint;
            let mut extra = 0;
            if let Some(a) = atlas {
                extra = match (v.lightmap, v.light) {
                    (Some(t), _) => atlas_word(a, t),
                    (None, Some(l)) => {
                        for k in 0..3 {
                            let tint = srgb_to_linear(f32::from(look.tint[k]) / 255.0);
                            color[k] = srgb_byte(tint * l[k] / GAIN_LIGHT);
                        }
                        atlas_word(a, GAIN_TEXEL)
                    }
                    (None, None) => atlas_word(a, FULL_LIGHT_TEXEL),
                };
            }
            if layer {
                color[3] = (v.alpha * 255.0) as u8;
            }
            HdVertex {
                position: to_world(v.position),
                normal: normal(to_world_dir(v.normal)),
                uv: v.uv,
                color,
                material: word,
                extra,
            }
        })
        .collect();
    d.indices = s.indices.clone();
    d
}
