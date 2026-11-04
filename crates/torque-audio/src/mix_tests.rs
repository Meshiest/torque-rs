use std::sync::Arc;

use super::*;
use crate::buffer::Buffer;
use crate::system::Description;
use crate::wav::Pcm;

#[test]
fn a_voice_mixes_and_ends_as_advance_ends_it() {
    let mut a = Audio::default();
    a.init();
    let data: Vec<u8> = (0..2_205)
        .flat_map(|i: i32| ((i % 100 - 50) as i16 * 300).to_le_bytes())
        .collect();
    let b = Some(Arc::new(Buffer {
        name: b"x.wav".to_vec(),
        pcm: Pcm {
            channels: 1,
            bits: 16,
            rate: 22_050,
            size: data.len(),
            data,
        },
    }));
    let h = a.create_source(&Description::default(), b, None, 0);
    a.play(h);
    let mut out = vec![0f32; 2 * 4_000];
    a.render(&mut out, 2, 44_100);
    assert!(out.iter().any(|x| *x != 0.0));
    assert!(a.is_playing(h));
    let mut out = vec![0f32; 2 * 1_000];
    a.render(&mut out, 2, 44_100);
    assert!(
        !a.is_playing(h),
        "100 ms of sound in 5,000 frames at 44.1 kHz"
    );
    assert!(out.iter().all(|x| x.is_finite()));
}

#[test]
fn attenuation_is_inverse_and_clamped() {
    assert_eq!(attenuation(1.0, 100.0, 0.5), 1.0);
    assert_eq!(attenuation(1.0, 100.0, 4.0), 0.25);
    assert_eq!(attenuation(1.0, 10.0, 50.0), 0.1);
    assert_eq!(attenuation(0.0, 10.0, 5.0), 1.0);
}
