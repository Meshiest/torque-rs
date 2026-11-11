//! The match drawn frame after frame as a window draws it: the pitch's floor is in every frame. The renderer's anti-aliasing
//! jitters the camera through 8 phases, so a floor that tied with the view's ground in the depth test showed in some phases and
//! not in others, and the window flickered.
//!
//! The server runs the Lua game mode, so this test needs the Lua extension (Luau's C++) and is ignored by `cargo test`:
//! `just examples mods-demo test` runs it.

mod common;

use torque::app::App;
use torque::app::shots::Seat;
use torque::app::window::offscreen_gpu;
use torque_example_mods_demo::game::{ClientOptions, NetMode};
use torque_example_mods_demo::shots::{SIZE, client, loopback_server};
use torque_render_core::Frame;

use common::{lua, work};

/// The mean color of the middle of the picture: the pitch's floor between its center circle and the near wall.
fn floor_color(frame: &Frame) -> [f32; 3] {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    for y in h * 55 / 100..h * 70 / 100 {
        for px in frame.rgba[(y * w + w * 30 / 100) * 4..(y * w + w * 70 / 100) * 4]
            .as_chunks::<4>()
            .0
        {
            for (s, &c) in sum.iter_mut().zip(px) {
                *s += u64::from(c);
            }
            n += 1;
        }
    }
    sum.map(|s| s as f32 / n as f32)
}

#[test]
#[ignore = "builds the Lua extension (Luau's C++)"]
fn the_pitchs_floor_is_in_every_frame() {
    let dir = work("frames");
    let lua = lua(&dir);
    let server = loopback_server(&lua, "Frames").expect("the server starts");
    let port = server.address().expect("a socket").port();
    let options = ClientOptions {
        net: NetMode::Loopback,
        port,
        ..ClientOptions::default()
    };
    let mut game = client(options, &dir.join("cache")).expect("the client is made");
    game.client.start_hosting(server);
    game.join(&format!("127.0.0.1:{port}"))
        .expect("the join starts");
    let gpu = offscreen_gpu().expect("an offscreen device");
    let mut seat = Seat::new(game, &gpu, SIZE).expect("a seat");
    for _ in 0..600 {
        if seat.game.playing() {
            break;
        }
        seat.run(&gpu, 50).expect("a frame");
    }
    assert!(seat.game.playing(), "the client joins the match");
    seat.run(&gpu, 1_000).expect("a frame");

    // Steps as short as a fast window's, and a frame drawn with no step at all: two whole cycles of the jitter's 8 phases.
    let colors: Vec<[f32; 3]> = [7, 7, 0, 3, 7, 16, 7, 1]
        .iter()
        .cycle()
        .take(16)
        .map(|&ms| {
            if ms > 0 {
                seat.game.advance(ms);
            }
            seat.frame(&gpu).expect("a frame");
            floor_color(&gpu.read_back(&seat.target).expect("a read back"))
        })
        .collect();
    let first = colors[0];
    for (i, c) in colors.iter().enumerate() {
        let apart = c
            .iter()
            .zip(first)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(
            apart < 6.0,
            "frame {i}'s floor is {c:?}, frame 0's {first:?}: {colors:?}"
        );
    }
}
