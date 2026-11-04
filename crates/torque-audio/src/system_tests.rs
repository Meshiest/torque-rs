use super::*;
use crate::wav::Pcm;

fn buffer(ms: u32) -> Option<Arc<Buffer>> {
    let frames = 22_050 * ms / 1000;
    Some(Arc::new(Buffer {
        name: b"t.wav".to_vec(),
        pcm: Pcm {
            channels: 1,
            bits: 16,
            rate: 22_050,
            data: vec![0; frames as usize * 2],
            size: frames as usize * 2,
        },
    }))
}

fn ready() -> Audio {
    let mut a = Audio::default();
    a.init();
    a
}

#[test]
fn handles_count_up_and_play_runs_the_clock() {
    let mut a = ready();
    let d = Description::default();
    let h1 = a.create_source(&d, buffer(500), None, 0);
    let h2 = a.create_source(&d, buffer(500), None, 0);
    assert_eq!((h1, h2), (1, 2));
    assert!(!a.is_playing(h1));
    assert_eq!(a.play(h1), h1);
    assert!(a.is_playing(h1));
    a.advance(300.0);
    assert!(a.is_playing(h1));
    a.advance(300.0);
    assert!(!a.is_playing(h1));
    // The next update closes both: the stopped one and the one made but never played.
    a.update();
    assert_eq!(a.sources_in_use(), 0);
    assert_eq!(a.play(h1), 0);
    assert_eq!(a.play(h2), 0);
}

#[test]
fn a_muted_channel_or_a_quiet_sound_gets_no_handle_and_no_number() {
    let mut a = ready();
    a.set_channel_volume(1, 0.0);
    let muted = Description {
        kind: 1,
        ..Description::default()
    };
    assert_eq!(a.create_source(&muted, buffer(100), None, 0), 0);
    let quiet = Description {
        volume: 0.05,
        ..Description::default()
    };
    assert_eq!(a.create_source(&quiet, buffer(100), None, 0), 0);
    assert_eq!(
        a.create_source(
            &Description {
                kind: 32,
                ..Description::default()
            },
            buffer(100),
            None,
            0
        ),
        0
    );
    assert_eq!(a.create_source(&Description::default(), None, None, 0), 0);
    assert_eq!(
        a.create_source(&Description::default(), buffer(100), None, 0),
        1
    );
}

#[test]
fn a_looper_with_no_source_is_an_image_until_its_channel_opens() {
    let mut a = ready();
    a.set_channel_volume(1, 0.0);
    let looper = Description {
        kind: 1,
        is_looping: true,
        ..Description::default()
    };
    let h = a.create_source(&looper, buffer(100), None, 0);
    assert_eq!(h, 0x8000_0001);
    assert!(!a.is_playing(h));
    assert_eq!(a.play(h), h);
    assert!(!a.is_playing(h));
    a.set_channel_volume(1, 1.0);
    a.now_ms = 1_000;
    a.update();
    assert!(a.is_playing(h), "restarted with its own handle");
    a.advance(10_000.0);
    assert!(a.is_playing(h), "loops");
    a.stop(h);
    assert!(!a.is_playing(h));
    assert!(a.loops.is_empty());
}

#[test]
fn a_culled_looper_restarts_only_above_the_uncull_gain() {
    for (channel, uncull, restarts) in [
        (0.08, MIN_UNCULL_GAIN, false),
        (0.11, MIN_UNCULL_GAIN, true),
        (0.08, 0.05, true),
    ] {
        let mut a = ready();
        a.uncull_gain = uncull;
        a.set_channel_volume(1, 0.0);
        let looper = Description {
            kind: 1,
            is_looping: true,
            ..Description::default()
        };
        let h = a.create_source(&looper, buffer(100), None, 0);
        assert_eq!(a.play(h), h);
        a.set_channel_volume(1, channel);
        a.now_ms = 1_000;
        a.update();
        assert_eq!(a.is_playing(h), restarts, "{channel} {uncull}");
    }
}

#[test]
fn the_seventeenth_sound_of_equal_volume_gets_nothing() {
    let mut a = ready();
    let d = Description::default();
    for i in 0..16 {
        let h = a.create_source(&d, buffer(5_000), None, 0);
        assert_eq!(a.play(h), h, "{i}");
    }
    assert_eq!(a.create_source(&d, buffer(5_000), None, 0), 0);
    let loud = a.create_source(&Description { volume: 1.0, ..d }, buffer(5_000), None, 0);
    assert_eq!(loud, 0, "equal scores are not culled");
    a.sources[3].volume = 0.5;
    assert_ne!(
        a.create_source(&d, buffer(5_000), None, 0),
        0,
        "a quieter one is culled"
    );
    a.stop_all();
    assert_eq!(a.sources_in_use(), 0);
}

#[test]
fn three_d_sounds_fall_off_linearly_for_their_volume() {
    let mut a = ready();
    let d = Description {
        is_3d: true,
        reference_distance: 2.0,
        max_distance: 10.0,
        ..Description::default()
    };
    assert_eq!(
        a.create_source(&d, buffer(100), Some([50.0, 0.0, 0.0]), 0),
        0
    );
    let h = a.create_source(&d, buffer(100), Some([6.0, 0.0, 0.0]), 0);
    assert_ne!(h, 0);
    assert!((a.sources[0].score - 0.5).abs() < 1e-6);
}

#[test]
fn a_2d_sound_is_relative_whatever_transform_it_was_given() {
    let mut a = ready();
    a.listener_3f(al::AL_POSITION, [400.0, -250.0, 30.0]);
    let gui = Description {
        reference_distance: 1.0,
        max_distance: 100.0,
        ..Description::default()
    };
    assert!(!gui.is_3d);
    // A script's `alxPlay`: the console function's transform at (0, 0, 0), far from the listener.
    let h = a.create_source(&gui, buffer(500), Some([0.0; 3]), 0);
    assert_eq!(a.play(h), h);
    let i = a.find(h).unwrap();
    assert!(a.sources[i].voice.relative);
    assert_eq!(a.sources[i].voice.position, [0.0, 0.0, 1.0]);
    a.update();
    a.update_scores(true);
    let i = a.find(h).unwrap();
    assert!(
        a.sources[i].voice.gain > 0.0,
        "not silenced as out of range"
    );
    assert!(a.sources[i].score > 0.0);
    // A 3D one at the same place is placed there, and silenced beyond its range.
    let near = Description { is_3d: true, ..gui };
    let h3 = a.create_source(&near, buffer(500), Some([350.0, -250.0, 30.0]), 0);
    let j = a.find(h3).unwrap();
    assert!(!a.sources[j].voice.relative);
    assert_eq!(a.sources[j].voice.position, [350.0, -250.0, 30.0]);
}

#[test]
fn source_and_listener_values_read_back() {
    let mut a = ready();
    a.listener_f(al::AL_GAIN_LINEAR, 0.0);
    assert_eq!(a.get_listener_f(al::AL_GAIN_LINEAR), 0.0);
    a.listener_f(al::AL_GAIN_LINEAR, 0.5);
    assert_eq!(a.get_listener_f(al::AL_GAIN_LINEAR), 0.5);
    let h = a.create_source(&Description::default(), buffer(100), None, 0);
    a.source_f(h, al::AL_PITCH, 2.0);
    a.source_f(h, al::AL_GAIN_LINEAR, 0.25);
    assert_eq!(a.get_source_f(h, al::AL_PITCH), Some(2.0));
    assert_eq!(a.get_source_f(h, al::AL_GAIN_LINEAR), Some(0.25));
    assert_eq!(a.get_source_f(h, al::AL_GAIN), Some(0.25));
    assert_eq!(a.get_source_f(99, al::AL_PITCH), None);
    a.source_i(h, al::AL_LOOPING, 1);
    assert_eq!(a.get_source_i(h, al::AL_LOOPING), Some(1));
    assert!(!a.set_channel_volume(32, 1.0));
    assert!(a.set_channel_volume(5, 7.0));
    assert_eq!(a.type_volume[5], 1.0);
}

#[test]
fn nothing_without_a_device() {
    let mut a = Audio::default();
    assert_eq!(
        a.create_source(&Description::default(), buffer(100), None, 0),
        0
    );
    assert_eq!(a.play(1), 0);
    a.stop_all();
    a.update();
}
