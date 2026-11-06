//! A bolt's flight agrees on the server and on every client (its first update's raw floats): Ann strafes and turns past
//! the corner of the test block (`game/fixtures/aim.mis`), firing as she goes, while Bob watches from the side. Each bolt that
//! hits the block stops on both clients where the server's explosion is (the holes both clients lay there), and is never drawn
//! past it: no bolt runs on through the block and jumps back when the server's explosion arrives.

use glam::Vec3;
use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;

fn client(name: &str) -> ArenaGame {
    ArenaGame::new(
        GameFiles::bundled().unwrap(),
        ClientOptions {
            net: NetMode::Loopback,
            name: name.to_string(),
            port: 0,
            map: Some("fixtures/aim.mis".to_string()),
            ..ClientOptions::default()
        },
    )
    .unwrap()
}

fn frame(a: &mut ArenaGame, b: &mut ArenaGame, ms: u32) {
    for g in [a, b] {
        g.step(ms);
        g.add_frame_time(ms as f32 / 1000.0);
        g.view_frame();
    }
}

fn place(a: &mut ArenaGame, name: &str, at: [f32; 3], yaw: f32) {
    let server = a.server.as_mut().unwrap();
    let [x, y, z] = at.map(|v| v.to_string());
    server
        .call("placeFigure", &[name, &x, &y, &z, &yaw.to_string()])
        .unwrap();
}

/// The block's sides, a unit cube scaled 2 x 2 x 3 about (3, 8, 1.5): whether `p` lies on one (within `eps`).
fn on_block(p: [f32; 3], eps: f32) -> bool {
    let inside = |v: f32, lo: f32, hi: f32| v > lo - eps && v < hi + eps;
    let on = |v: f32, lo: f32, hi: f32| (v - lo).abs() < eps || (v - hi).abs() < eps;
    inside(p[0], 2.0, 4.0)
        && inside(p[1], 7.0, 9.0)
        && inside(p[2], 0.0, 3.0)
        && (on(p[0], 2.0, 4.0) || on(p[1], 7.0, 9.0) || on(p[2], 0.0, 3.0))
}

/// Whether `p` is inside the block, deeper than `depth` from its sides.
fn inside_block(p: [f32; 3], depth: f32) -> bool {
    let inside = |v: f32, lo: f32, hi: f32| v > lo + depth && v < hi - depth;
    inside(p[0], 2.0, 4.0) && inside(p[1], 7.0, 9.0) && inside(p[2], 0.0, 3.0)
}

/// What the corner's shots gave: the block's hits compared, those near the corner's edge, and the farthest a client's stop was
/// from the server's hit.
struct Sweep {
    compared: usize,
    near_edge: usize,
    worst: f32,
}

/// The shots across the corner, each hit checked as it comes (the stops within 3 mm, no bolt drawn inside the block).
fn sweep() -> Sweep {
    let mut a = client("Ann");
    let mut b = client("Bob");
    a.call("hostGame", &[]).unwrap();
    for _ in 0..300 {
        frame(&mut a, &mut b, 16);
        if a.in_game() {
            break;
        }
    }
    let address = a.hosted_address().unwrap().to_string();
    b.call("connectTo", &[&address]).unwrap();
    for _ in 0..300 {
        frame(&mut a, &mut b, 16);
        if b.in_game() && a.scene().others().len() == 1 && b.scene().others().len() == 1 {
            break;
        }
    }
    // Ann south-west of the block's near-left corner (2, 7), facing just right of it; Bob off to the side.
    let (from, corner) = ([-1.5_f32, -3.0, 0.1], [2.0_f32, 7.0]);
    let yaw = (corner[0] - from[0]).atan2(corner[1] - from[1]);
    place(&mut a, "Ann", from, yaw - 0.02);
    place(&mut a, "Bob", [-8.0, 4.0, 0.1], 1.2);
    for _ in 0..60 {
        frame(&mut a, &mut b, 16);
    }
    // Strafing right and turning right: the eye's line sweeps across the corner, from passing it to grazing it to hitting the
    // block's face.
    a.set_global("$mvRightAction", "0.1");
    let (mut compared, mut near_edge, mut holes_seen) = (0, 0, [0usize; 2]);
    let mut worst = 0.0f32;
    for shot in 0..10 {
        a.call("fire", &["1"]).unwrap();
        let mut stops: [Vec<[f32; 3]>; 2] = [Vec::new(), Vec::new()];
        let mut drawn: [Vec<Vec<[f32; 3]>>; 2] = [Vec::new(), Vec::new()];
        for i in 0..24 {
            if i == 3 {
                a.call("fire", &["0"]).unwrap();
            }
            let turn: f32 = a.global("$mvYaw").parse().unwrap_or(0.0);
            a.set_global("$mvYaw", &(turn + 0.0002).to_string());
            frame(&mut a, &mut b, 16);
            for (k, g) in [&a, &b].into_iter().enumerate() {
                for s in g.scene().bolt_stops() {
                    if !stops[k]
                        .iter()
                        .any(|q| Vec3::from(*q).distance(s.into()) < 1e-6)
                    {
                        stops[k].push(s);
                    }
                }
                drawn[k].push(g.scene().bolt_positions());
            }
        }
        // The holes each client laid (the server's explosion points, sent as floats).
        let holes: [Vec<[f32; 3]>; 2] = [&a, &b].map(|g| {
            g.scene()
                .decals
                .decals()
                .map(|d| d.position)
                .collect::<Vec<_>>()
        });
        for k in 0..2 {
            let new = &holes[k][holes_seen[k]..];
            holes_seen[k] = holes[k].len();
            for h in new {
                if !on_block(*h, 0.02) {
                    continue;
                }
                // The server's hit: its explosion (the hole) is 1 cm out along the face's normal (TGE's), the nearest point of the
                // block to it.
                let hit = [
                    h[0].clamp(2.0, 4.0),
                    h[1].clamp(7.0, 9.0),
                    h[2].clamp(0.0, 3.0),
                ];
                // The client stopped the bolt where the server hit, within millimeters.
                let stop = stops[k].iter().copied().min_by(|p, q| {
                    let d = |a: [f32; 3]| Vec3::from(a).distance(hit.into());
                    d(*p).total_cmp(&d(*q))
                });
                let off = stop.map_or(f32::INFINITY, |s| Vec3::from(s).distance(hit.into()));
                worst = worst.max(off);
                compared += 1;
                if (h[0] - 2.0).abs() < 0.08 || (h[1] - 7.0).abs() < 0.08 && h[0] < 2.1 {
                    near_edge += 1;
                }
                assert!(
                    off < 0.003,
                    "shot {shot}: client {k} stopped at {stop:?}, the server hit {hit:?}"
                );
                // Never drawn past the hole along the flight (it would jump back when the explosion came).
                for (f, frame) in drawn[k].iter().enumerate() {
                    for p in frame {
                        assert!(
                            !inside_block(*p, 0.02),
                            "shot {shot}: client {k}'s bolt drawn inside the block at frame {f}: {p:?}"
                        );
                    }
                }
            }
        }
    }
    a.set_global("$mvRightAction", "0");
    Sweep {
        compared,
        near_edge,
        worst,
    }
}

#[test]
fn a_bolt_at_a_corner_stops_where_the_server_hit_on_every_client() {
    let s = sweep();
    println!(
        "{} hits compared, {} at the corner's edge, the farthest stop {} m from the server's hit",
        s.compared, s.near_edge, s.worst
    );
    assert!(s.compared >= 6, "the block was hit {} times", s.compared);
    assert!(s.near_edge >= 1, "no shot came near the corner's edge");
}
