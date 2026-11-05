//! A clip of the kit's figure crouching, lying down, moving in each pose and reacting to hits, drawn offscreen for review as
//! another player's client draws it: its threads run by the client's animation ([`torque::client::player_anim`]) from the
//! pose, the velocity and the script threads a record gives, as a ghost's are. The pose is the one a server shows: between
//! standing and prone it kneels in the crouch on the way ([`torque::sim::player::pose::KNEEL_TICKS`]). In order:
//!
//! 1. standing, crouching, standing up, lying down and getting up (the blends between poses);
//! 2. the crouched walk forward, the crouched side step to the left and the crouched walk backward;
//! 3. the crawl forward and backward;
//! 4. walking, then sprinting, then walking again;
//! 5. holding a pistol, hit in the body from the front, the back, the left and the right, then in the head from the front
//!    and the left; then lying down with the pistol and hit there too.
//!
//! Each frame shows a side view and a three-quarter view side by side over a floor marked every half unit (so a sliding
//! foot shows), with a bar under them: its length the speed, its color the pose (gray standing, green walking, orange
//! sprinting, blue crouched, purple prone), red while a hit plays.
//!
//! The frames go to `ffmpeg` (on the path) as `poses-clip.mp4`; a strip of a few frames goes beside it as `poses-clip.png`.
//!
//! `cargo run -p torque-example-common --release --example poses-clip -- <dir>` (default `poses-clip`)

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use torque::app::view::{Daylight, SceneShape, WorldView};
use torque::app::window::offscreen_gpu;
use torque::client::player_anim::{PlayerAnimation, PlayerMotion, SCRIPT_SLOTS, ScriptPlay, WALK};
use torque::game::sim::player::poses::Pose;
use torque::shapes::instance::find_sequence;
use torque_example_common::{KitAssets, kit_path};
use torque_render_scene::hd::kit::{box_object, look_at};
use torque_render_scene::math::Mat;

const VIEW: (u32, u32) = (640, 480);
const FPS: u32 = 30;
const BAR: u32 = 24;
/// The fastest the bar shows (units a second).
const TOP_SPEED: f32 = 7.0;

/// One stretch of the clip: how long, the pose, the velocity it eases to (the figure's own frame: x right, y ahead), whether it
/// holds a pistol, and a hit that lands 0.3 s in.
struct Stretch {
    seconds: f32,
    pose: Pose,
    velocity: [f32; 2],
    hold: bool,
    hit: Option<&'static str>,
}

const fn s(seconds: f32, pose: Pose, velocity: [f32; 2]) -> Stretch {
    Stretch {
        seconds,
        pose,
        velocity,
        hold: false,
        hit: None,
    }
}

const fn hit(pose: Pose, name: &'static str) -> Stretch {
    Stretch {
        seconds: 1.1,
        pose,
        velocity: [0.0, 0.0],
        hold: true,
        hit: Some(name),
    }
}

const STILL: [f32; 2] = [0.0, 0.0];

fn stretches() -> Vec<Stretch> {
    use Pose::*;
    vec![
        s(1.0, Stand, STILL),
        s(2.0, Crouch, STILL),
        s(1.6, Stand, STILL),
        s(2.4, Prone, STILL),
        s(2.2, Stand, STILL),
        // The crouched walk forward, the side step left and the walk backward.
        s(0.8, Crouch, STILL),
        s(3.0, Crouch, [0.0, 1.2]),
        s(2.2, Crouch, [-0.9, 0.0]),
        s(2.2, Crouch, [0.0, -1.0]),
        s(0.8, Crouch, STILL),
        // The crawl.
        s(1.5, Prone, STILL),
        s(3.5, Prone, [0.0, 0.75]),
        s(2.2, Prone, [0.0, -0.6]),
        s(1.8, Stand, STILL),
        // Walking, sprinting, walking.
        s(2.6, Stand, [0.0, 2.4]),
        s(2.6, Sprint, [0.0, 6.5]),
        s(2.0, Stand, [0.0, 2.4]),
        s(1.2, Stand, STILL),
        // Hits from four sides and two to the head, standing with a pistol; then lying down with it.
        hit(Stand, "flinch_front"),
        hit(Stand, "flinch_back"),
        hit(Stand, "flinch_left"),
        hit(Stand, "flinch_right"),
        hit(Stand, "flinch_head_front"),
        hit(Stand, "flinch_head_left"),
        Stretch {
            seconds: 2.0,
            pose: Prone,
            velocity: STILL,
            hold: true,
            hit: None,
        },
        hit(Prone, "flinch_front"),
        hit(Prone, "flinch_head_right"),
        s(2.0, Stand, STILL),
    ]
}

/// How fast the drawn velocity eases to the stretch's (units a second, a second).
const EASE: f32 = 10.0;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "poses-clip".to_string()),
    );
    std::fs::create_dir_all(&dir)?;
    let gpu = offscreen_gpu()?;
    let assets = KitAssets::new()?;
    let figure = SceneShape::load(&assets, &kit_path("figure.dts")).ok_or("figure.dts")?;
    let shape = figure.shape();
    let detail = figure.detail().ok_or("no detail")?;
    let mut instance = figure.instance().ok_or("no instance")?;
    let mut anim = PlayerAnimation::new(shape, &mut instance).ok_or("no threads")?;
    let action_of = |name: &[u8]| anim.actions().iter().position(|a| a.name == name);
    let walk = action_of(WALK).ok_or("the figure has no walk")?;
    let hold = action_of(b"hold").ok_or("the figure has no hold")? as u32;
    let mut views = [WorldView::new(&gpu), WorldView::new(&gpu)];
    for view in &mut views {
        view.set_plain_floor(400.0, [150, 152, 156]);
        view.set_daylight(&Daylight {
            towards: [0.45, 0.6, 0.66],
            ..Daylight::default()
        });
    }
    let (w, h) = (2 * VIEW.0, VIEW.1 + BAR);
    let mp4 = dir.join("poses-clip.mp4");
    let mut ffmpeg = Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
        ])
        .arg(format!("{w}x{h}"))
        .args([
            "-r",
            &FPS.to_string(),
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-crf",
            "18",
        ])
        .arg(&mp4)
        .stdin(Stdio::piped())
        .spawn()?;
    let mut pipe = ffmpeg.stdin.take().ok_or("no ffmpeg input")?;
    let dt = 1.0 / FPS as f32;
    let plan = stretches();
    let total: f32 = plan.iter().map(|s| s.seconds).sum();
    let frames = (total * FPS as f32) as usize;
    let (mut at, mut velocity, mut strip) = ([0.0f32; 2], [0.0f32; 2], Vec::new());
    // The pose the server shows, the one before it and the seconds since it changed.
    let kneel = torque::sim::player::pose::KNEEL_TICKS as f32 * 0.032;
    let (mut shown, mut from, mut since) = (Pose::Stand, Pose::Stand, 0.0f32);
    let strip_every = frames / 8;
    for k in 0..frames {
        let t = k as f32 * dt;
        let (mut start, mut now) = (0.0, &plan[0]);
        for stretch in &plan {
            if t < start + stretch.seconds {
                now = stretch;
                break;
            }
            start += stretch.seconds;
        }
        since += dt;
        let kneeling = shown == Pose::Crouch && from.far_from(now.pose) && since < kneel;
        let next = if shown.far_from(now.pose) || kneeling {
            Pose::Crouch
        } else {
            now.pose
        };
        if next != shown {
            (from, shown, since) = (shown, next, 0.0);
        }
        for i in 0..2 {
            let d = now.velocity[i] - velocity[i];
            velocity[i] += d.clamp(-EASE * dt, EASE * dt);
            at[i] += velocity[i] * dt;
        }
        // A hit's reaction on the first script thread, from 0.3 s into its stretch, as a record's thread would carry it.
        let mut plays = [None; SCRIPT_SLOTS];
        let mut hurt = false;
        if let Some(name) = now.hit
            && let Some(seq) = find_sequence(shape, name.as_bytes())
        {
            let since = t - start - 0.3;
            let length = shape.sequences[seq].duration;
            if since >= 0.0 {
                plays[0] = Some(ScriptPlay {
                    sequence: seq,
                    pos: (since / length).min(1.0),
                });
                hurt = since < length;
            }
        }
        let motion = PlayerMotion {
            velocity: [velocity[0], velocity[1], 0.0],
            look: 0.5,
            pose: shown,
            arm_action: now.hold.then_some(hold),
            ..PlayerMotion::default()
        };
        anim.update(shape, &mut instance, &motion, dt);
        anim.script_threads(shape, &mut instance, &plays);
        let _ = instance.animate(shape, detail, None, &mut |_| true);
        let mut place = Mat::IDENTITY;
        place.set_column3([at[0], at[1], 0.0]);
        let mut rgb = vec![0u8; (w * h * 3) as usize];
        // The side view from the figure's right, and the three-quarter view from its front right, both following it.
        for (v, (side, rise, ahead)) in [(1.0f32, 0.12f32, 0.0f32), (0.7, 0.3, 0.7)]
            .into_iter()
            .enumerate()
        {
            let view = &mut views[v];
            let mut objects = vec![view.shape_object(&assets, &figure, &instance, &place, 1)];
            // Dark tiles every half unit around the figure.
            let near = [(at[0] / 0.5).round() as i32, (at[1] / 0.5).round() as i32];
            let mut id = 10;
            for i in near[1] - 12..=near[1] + 12 {
                for j in near[0] - 3..=near[0] + 3 {
                    let mut o = box_object(
                        id,
                        [j as f32 * 0.5 + 0.25, i as f32 * 0.5, 0.0],
                        [0.04, 0.04, 0.004],
                    );
                    id += 1;
                    for vtx in o.draws.iter_mut().flat_map(|d| d.vertices.iter_mut()) {
                        vtx.color = [60, 62, 66, 255];
                    }
                    objects.push(o);
                }
            }
            view.set_objects(objects);
            let center = [at[0], at[1], 0.8];
            let distance = 4.2;
            let flat = (1.0 - rise * rise).sqrt();
            let along = (1.0 - side * side).sqrt() * ahead.signum();
            let eye_at = [
                center[0] + distance * side * flat,
                center[1] + distance * along * flat,
                center[2] + distance * rise,
            ];
            let mut eye = look_at(eye_at, center);
            eye.fov = Some(45);
            if k == 0 {
                view.reset_history();
            }
            let frame = view.render_world(&gpu, VIEW, eye)?;
            for py in 0..VIEW.1 {
                for px in 0..VIEW.0 {
                    let s = 4 * (py * VIEW.0 + px) as usize;
                    let d = 3 * (py * w + v as u32 * VIEW.0 + px) as usize;
                    rgb[d..d + 3].copy_from_slice(&frame.rgba[s..s + 3]);
                }
            }
        }
        let speed = velocity[0].hypot(velocity[1]);
        let color = if hurt {
            [220, 50, 50]
        } else {
            match shown {
                Pose::Crouch => [70, 120, 230],
                Pose::Prone => [150, 80, 200],
                Pose::Sprint => [235, 140, 40],
                Pose::Stand if anim.action().0 == walk => [70, 190, 90],
                Pose::Stand => [140, 140, 140],
            }
        };
        let filled = ((speed / TOP_SPEED).max(0.04) * w as f32) as u32;
        for py in VIEW.1..h {
            for px in 0..w {
                let d = 3 * (py * w + px) as usize;
                let c = if px < filled { color } else { [30, 30, 30] };
                rgb[d..d + 3].copy_from_slice(&c);
            }
        }
        for py in 0..VIEW.1 {
            let d = 3 * (py * w + VIEW.0) as usize;
            rgb[d..d + 3].copy_from_slice(&[30, 30, 30]);
        }
        pipe.write_all(&rgb)?;
        if k % strip_every == strip_every / 2 {
            strip.push(rgb);
        }
    }
    drop(pipe);
    if !ffmpeg.wait()?.success() {
        return Err("ffmpeg failed".into());
    }
    println!("{}", mp4.display());
    let png = dir.join("poses-clip.png");
    let rows = strip.len() as u32;
    let bytes = torque_example_assets::textures::png(w, h * rows, |x, y| {
        let f = &strip[(y / h) as usize];
        let d = 3 * ((y % h) * w + x) as usize;
        [f[d], f[d + 1], f[d + 2]]
    })?;
    std::fs::write(&png, bytes)?;
    println!("{}", png.display());
    Ok(())
}
