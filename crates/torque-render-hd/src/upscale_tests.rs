use super::*;

/// The frame index's period keeps the jitter's phases and the ring's layers in step across its wrap, and is exact in a float.
#[test]
fn the_upscaled_frame_period_is_a_multiple_of_the_phases_and_the_ring() {
    const {
        let phases = torque_render_scene::hd::post::JITTER_PHASES;
        assert!(UPSCALED_FRAME_PERIOD.is_multiple_of(phases));
        assert!(UPSCALED_FRAME_PERIOD.is_multiple_of(RING_FRAMES as u64));
        assert!(RING_FRAMES as u64 == phases + 1);
        assert!(UPSCALED_FRAME_PERIOD < 1 << f32::MANTISSA_DIGITS);
    }
}

/// A half-float history is the output itself; a 32-bit one is a target of its own before it.
#[test]
fn the_temporal_upscalers_targets_follow_its_precision() {
    assert_eq!(
        TemporalUpscaler::targets(HistoryPrecision::Half),
        [SHOWN_FORMAT, MOMENTS_FORMAT, RING_FORMAT]
    );
    assert_eq!(
        TemporalUpscaler::targets(HistoryPrecision::Full),
        [
            wgpu::TextureFormat::Rgba32Float,
            SHOWN_FORMAT,
            MOMENTS_FORMAT,
            RING_FORMAT
        ]
    );
}
