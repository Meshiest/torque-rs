//! Where the bolts go and where the own figure is drawn, against the server: a client that turned its view with the mouse over
//! many moves fires at a block and the bolt bursts where the crosshair's ray meets it; looking up into the empty sky the bolt
//! flies along the crosshair's ray; and walking forward, the own figure the client draws keeps up with the server's and never
//! steps back. The game plays `game/fixtures/aim.mis`: a floor and one block ahead and to the right of the first spawn point.

use torque::client::input::Key;
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;

type F3 = [f32; 3];

fn host() -> ArenaGame {
    let mut a = ArenaGame::new(
        GameFiles::bundled().unwrap(),
        ClientOptions {
            net: NetMode::Loopback,
            name: "Ann".to_string(),
            port: 0,
            map: Some("fixtures/aim.mis".to_string()),
            ..ClientOptions::default()
        },
    )
    .unwrap();
    a.call("hostGame", &[]).unwrap();
    for _ in 0..300 {
        frame(&mut a, 16);
        if a.in_game() && a.own_look().is_some_and(|o| o.position.is_some()) {
            break;
        }
    }
    assert!(a.in_game(), "in the game");
    frames(&mut a, 500);
    a
}

/// `ms` of the clock and a frame after it, as the window runs them.
fn frame(a: &mut ArenaGame, ms: u32) {
    a.step(ms);
    a.add_frame_time(ms as f32 / 1000.0);
    a.view_frame();
}

fn frames(a: &mut ArenaGame, ms: u32) {
    for _ in 0..ms / 16 {
        frame(a, 16);
    }
}

/// The eye and the direction the frame looks in (the crosshair's ray).
fn crosshair(a: &ArenaGame) -> (F3, F3) {
    let own = a.scene().own().expect("the own figure");
    let (sy, cy) = own.yaw.sin_cos();
    let (sp, cp) = own.pitch.sin_cos();
    (own.eye, [sy * cp, cy * cp, -sp])
}

fn sub(a: F3, b: F3) -> F3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: F3, b: F3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn len(a: F3) -> f32 {
    dot(a, a).sqrt()
}

/// How far `p` is from the ray from `o` along unit `d`.
fn off_ray(p: F3, o: F3, d: F3) -> f32 {
    let v = sub(p, o);
    let along = dot(v, d);
    len(sub(v, [d[0] * along, d[1] * along, d[2] * along]))
}

/// The server's Projectiles: id, position, velocity, exploded, explosion point.
fn bolts(a: &mut ArenaGame) -> Vec<(u32, F3, F3, bool, F3)> {
    let server = a.server.as_mut().unwrap();
    let vm = server.vm_mut();
    let Some(group) = vm.objects().find(b"MissionCleanup") else {
        return Vec::new();
    };
    let count: u32 = String::from_utf8_lossy(&vm.call_method(group, b"getCount", &[]).unwrap())
        .parse()
        .unwrap();
    let mut out = Vec::new();
    for i in 0..count {
        let id = vm
            .call_method(group, b"getObject", &[i.to_string().into_bytes()])
            .unwrap();
        let id: u32 = String::from_utf8_lossy(&id).parse().unwrap();
        if let Some(s) = torque::game::sim::projectile::state(vm, id) {
            out.push((id, s.position, s.velocity, s.exploded, s.explosion_pos));
        }
    }
    out
}

/// Turns the view by `yaw` and `pitch` (radians, positive pitch looks down) with the mouse, a little every frame over a second
/// and a half, as a player's hand turns it (many small moves, each quantized).
fn turn(a: &mut ArenaGame, yaw: f32, pitch: f32) {
    let steps = 90;
    for _ in 0..steps {
        a.host
            .mouse_motion(yaw / 0.003 / steps as f32, pitch / 0.003 / steps as f32);
        frame(a, 16);
    }
    frames(a, 300);
}

fn fire(a: &mut ArenaGame) {
    a.call("fire", &["1"]).unwrap();
    frames(a, 64);
    a.call("fire", &["0"]).unwrap();
}

/// Where the ray from `o` along `d` meets the block (x 2..4, y 7..9, z 0..3) first.
fn block_hit(o: F3, d: F3) -> Option<F3> {
    let (lo, hi) = ([2.0, 7.0, 0.0], [4.0, 9.0, 3.0]);
    let (mut near, mut far) = (0.0f32, f32::MAX);
    for k in 0..3 {
        let (t0, t1) = ((lo[k] - o[k]) / d[k], (hi[k] - o[k]) / d[k]);
        near = near.max(t0.min(t1));
        far = far.min(t0.max(t1));
    }
    (near <= far).then(|| [0, 1, 2].map(|k| o[k] + d[k] * near))
}

#[test]
fn the_bolt_bursts_where_the_crosshair_meets_the_block() {
    let mut a = host();
    // Toward the block's near face, a little below the eye, turned there by many small mouse moves left and right.
    turn(&mut a, 0.6, 0.3);
    turn(&mut a, -0.37, -0.25);
    let (eye, aim) = crosshair(&a);
    let hit = block_hit(eye, aim).expect("the crosshair is on the block");
    fire(&mut a);
    let mut burst = None;
    for _ in 0..60 {
        frame(&mut a, 16);
        if let Some(b) = bolts(&mut a).into_iter().find(|b| b.3) {
            burst = Some(b.4);
            break;
        }
    }
    let burst = burst.expect("the bolt burst");
    let miss = len(sub(burst, hit));
    assert!(
        miss < 0.1,
        "the bolt burst {miss} from where the crosshair meets the block ({burst:?}, {hit:?})"
    );
}

#[test]
fn looking_up_into_the_sky_the_bolt_flies_along_the_crosshair() {
    let mut a = host();
    // Thirty degrees up, turned a little to the left of the block.
    turn(&mut a, -0.4, -std::f32::consts::FRAC_PI_6);
    let own = a.scene().own().unwrap();
    assert!(
        (own.pitch + std::f32::consts::FRAC_PI_6).abs() < 0.01,
        "looking up: {}",
        own.pitch
    );
    let (eye, aim) = crosshair(&a);
    fire(&mut a);
    let mut seen = 0;
    for _ in 0..30 {
        frame(&mut a, 16);
        for (_, p, v, exploded, _) in bolts(&mut a) {
            assert!(!exploded, "nothing to hit");
            let angle = (dot(v, aim) / len(v)).clamp(-1.0, 1.0).acos().to_degrees();
            assert!(
                angle < 0.25,
                "the bolt flies {angle} degrees off the crosshair"
            );
            let off = off_ray(p, eye, aim);
            assert!(
                off < 0.05,
                "the bolt is {off} off the crosshair's ray at {p:?}"
            );
            seen += 1;
        }
    }
    assert!(seen > 10, "the bolt flew");
}

#[test]
fn the_own_figure_keeps_up_with_the_server_and_never_steps_back() {
    let mut a = host();
    a.key(Key::W, true);
    let mut last: Option<F3> = None;
    for i in 0..120 {
        // Frames of uneven length, as a window's are.
        frame(&mut a, [7, 16, 9, 33][i % 4]);
        let own = a.scene().own().unwrap().position;
        if let Some(last) = last {
            // Walking north: y never goes back, and no frame jumps further than the walk could carry it.
            assert!(
                own[1] >= last[1] - 1e-4,
                "frame {i}: back from {last:?} to {own:?}"
            );
            assert!(
                len(sub(own, last)) < 0.4,
                "frame {i}: a jump from {last:?} to {own:?}"
            );
        }
        last = Some(own);
        let server = {
            let s = a.server.as_mut().unwrap();
            let vm = s.vm_mut();
            let group = vm.objects().find(b"ClientGroup").unwrap();
            let c = vm
                .call_method(group, b"getObject", &[b"0".to_vec()])
                .unwrap();
            let c: u32 = String::from_utf8_lossy(&c).parse().unwrap();
            let p = vm.objects().field(c, b"player", b"").unwrap();
            let p: u32 = String::from_utf8_lossy(&p).parse().unwrap();
            let t = vm.call_method(p, b"getPosition", &[]).unwrap();
            let v: Vec<f32> = String::from_utf8_lossy(&t)
                .split_whitespace()
                .map(|x| x.parse().unwrap())
                .collect();
            [v[0], v[1], v[2]]
        };
        // The client runs its moves before the server has them: up to two ticks ahead at a walk.
        let ahead = len(sub(own, server));
        assert!(
            ahead < 0.5,
            "frame {i}: {ahead} from the server's ({own:?}, {server:?})"
        );
        assert!(
            (own[2] - server[2]).abs() < 0.05,
            "frame {i}: on the floor as the server has it"
        );
    }
    a.key(Key::W, false);
}

#[test]
fn the_drawn_bolt_leaves_the_pistol_and_curves_onto_the_crosshair() {
    let mut a = host();
    turn(&mut a, -0.4, -0.2);
    let (eye, aim) = crosshair(&a);
    a.call("fire", &["1"]).unwrap();
    let mut first = None;
    let (mut on_line, mut curving, mut last_off) = (0, 0, f32::MAX);
    for i in 0..40 {
        if i == 4 {
            a.call("fire", &["0"]).unwrap();
        }
        frame(&mut a, 16);
        let drawn = a.scene().bolt_positions();
        let Some(&p) = drawn.first() else {
            continue;
        };
        if first.is_none() {
            let muzzle = a.scene().own_muzzle().expect("the pistol's muzzle");
            let from = len(sub(p, muzzle));
            assert!(
                from < 0.02,
                "the bolt is first drawn {from} from the muzzle"
            );
            assert!(
                off_ray(p, eye, aim) > 0.2,
                "the muzzle is beside the crosshair's ray"
            );
            first = Some(p);
        } else if len(sub(p, eye)) > torque_example_fps::scene::BOLT_BLEND + 1.5 {
            // On the line, to within the record's quantized direction.
            let off = off_ray(p, eye, aim);
            let far = len(sub(p, eye));
            assert!(
                off < 0.05 || off / far < 0.002,
                "past its curve the bolt is drawn {off} off the crosshair's ray, {far} out"
            );
            on_line += 1;
        } else {
            // Curving in: nearer the line each frame (to within the record's quantized direction).
            let off = off_ray(p, eye, aim);
            let far = len(sub(p, eye));
            assert!(
                off <= last_off + 0.002 * far,
                "it curves in: {off} after {last_off}, {far} out"
            );
            last_off = off;
            curving += 1;
        }
    }
    assert!(first.is_some() && on_line > 3, "the bolt was drawn");
    assert!(
        curving >= 2,
        "it is drawn on its curve for a few frames ({curving})"
    );
}

#[test]
fn holding_the_right_button_raises_the_sights_narrows_the_view_and_slows_the_turn_and_the_walk() {
    use torque::client::MouseButton;
    use torque_example_fps::scene::{AIM_FOV, FOV};
    let mut a = host();
    let (eye, aim) = crosshair(&a);
    let hip = a.scene().own_muzzle().expect("the pistol's muzzle");
    assert!(
        off_ray(hip, eye, aim) > 0.2,
        "at the hip the muzzle is beside the crosshair"
    );
    assert_eq!(a.eye().fov, Some(FOV));
    let at = (400, 300);
    a.host.mouse_button(MouseButton::Right, true, at);
    frame(&mut a, 16);
    assert_eq!(a.global("$Arena::Aiming"), "1");
    let rising = a.scene().aim();
    assert!(rising > 0.0 && rising < 0.5, "on the way up: {rising}");
    frames(&mut a, 300);
    assert_eq!(a.scene().aim(), 1.0, "at the sights");
    let (eye, aim) = crosshair(&a);
    let muzzle = a.scene().own_muzzle().unwrap();
    let off = off_ray(muzzle, eye, aim);
    assert!(
        off < 0.05,
        "at the sights the muzzle is {off} off the crosshair's ray"
    );
    assert_eq!(a.eye().fov, Some(AIM_FOV as u8));
    // The mouse turns the narrower view by as much less.
    let scale: f32 = a.global("$Arena::TurnScale").parse().unwrap();
    let want = (AIM_FOV.to_radians() * 0.5).tan() / (f32::from(FOV).to_radians() * 0.5).tan();
    assert!((scale - want).abs() < 1e-3, "{scale} {want}");
    let before = a.global("$Arena::LookYaw").parse::<f32>().unwrap();
    a.host.mouse_motion(100.0, 0.0);
    frame(&mut a, 16);
    let turned = a.global("$Arena::LookYaw").parse::<f32>().unwrap() - before;
    assert!((turned - 0.3 * want).abs() < 1e-3, "turned {turned}");
    // Sprinting while aiming is slower (a walk is slower still, and stays a walk).
    a.key(Key::W, true);
    frame(&mut a, 16);
    assert_eq!(a.global("$mvForwardAction"), "0.35");
    a.key(Key::ShiftLeft, true);
    frame(&mut a, 16);
    assert_eq!(a.global("$mvForwardAction"), "0.6");
    // Let go: back to the hip, the full view and the full sprint.
    a.host.mouse_button(MouseButton::Right, false, at);
    frames(&mut a, 300);
    assert_eq!(a.global("$Arena::Aiming"), "0");
    assert_eq!(a.scene().aim(), 0.0);
    assert_eq!(a.eye().fov, Some(FOV));
    assert_eq!(a.global("$mvForwardAction"), "1");
    a.key(Key::ShiftLeft, false);
    assert_eq!(a.global("$mvForwardAction"), "0.35");
    a.key(Key::W, false);
}

#[test]
fn a_bolt_that_hits_the_block_leaves_a_hole_on_its_face_and_nowhere_else() {
    let mut a = host();
    turn(&mut a, 0.6, 0.3);
    turn(&mut a, -0.37, -0.25);
    let (eye, aim) = crosshair(&a);
    let hit = block_hit(eye, aim).expect("the crosshair is on the block");
    assert!(a.scene().decals.is_empty());
    fire(&mut a);
    frames(&mut a, 600);
    let decals: Vec<_> = a.scene().decals.decals().cloned().collect();
    assert_eq!(decals.len(), 1, "one hole");
    let d = &decals[0];
    let lifted = len(sub(d.position, hit));
    assert!(
        lifted < 0.1,
        "the hole is {lifted} from the hit ({:?}, {hit:?})",
        d.position
    );
    // On the block's face toward the eye, facing out of it, its mesh on that face (y = 7) and inside the block's bounds.
    assert!(d.normal[1] < -0.99, "{:?}", d.normal);
    assert!(!d.triangles.is_empty());
    for t in &d.triangles {
        for (p, _) in t {
            assert!((p[1] - 7.0).abs() < 0.02, "on the face: {p:?}");
            assert!(
                (2.0..=4.0).contains(&p[0]) && (0.0..=3.0).contains(&p[2]),
                "{p:?}"
            );
        }
    }
}

/// Where `p` lands on a screen of `size` pixels seen from `eye` (position, yaw, pitch) with a horizontal field of view of
/// `fov` degrees: pixels from the screen's center (right, down).
fn project(p: F3, eye: F3, yaw: f32, pitch: f32, fov: f32, size: (f32, f32)) -> (f32, f32) {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let forward = [sy * cp, cy * cp, -sp];
    let right = [cy, -sy, 0.0];
    let up = [sy * sp, cy * sp, cp];
    let v = sub(p, eye);
    let (x, y, z) = (dot(v, right), dot(v, up), dot(v, forward));
    let focal = size.0 * 0.5 / (fov.to_radians() * 0.5).tan();
    (focal * x / z, -focal * y / z)
}

#[test]
fn aimed_the_front_post_sits_in_the_rear_notch_on_the_crosshair() {
    let mut a = host();
    turn(&mut a, -0.3, 0.1);
    a.call("aim", &["1"]).unwrap();
    frames(&mut a, 400);
    assert_eq!(a.scene().aim(), 1.0);
    let own = a.scene().own().unwrap();
    let [front, rear] = a.scene().own_sights().expect("the pistol's sights");
    let fov = f32::from(a.eye().fov.unwrap());
    for size in [(1280.0, 720.0), (1920.0, 1080.0)] {
        for (name, p) in [("front post", front), ("rear notch", rear)] {
            let (x, y) = project(p, own.eye, own.yaw, own.pitch, fov, size);
            assert!(
                x.abs() <= 1.0 && y.abs() <= 1.0,
                "{name} {x:.2},{y:.2} px from the center at {size:?}"
            );
        }
    }
    // The bolt still leaves the muzzle.
    let muzzle = a.scene().own_muzzle().unwrap();
    assert!(
        len(sub(muzzle, own.eye)) > AIM_SIGHT_TO_MUZZLE,
        "{muzzle:?}"
    );
}

const AIM_SIGHT_TO_MUZZLE: f32 = 0.3;
