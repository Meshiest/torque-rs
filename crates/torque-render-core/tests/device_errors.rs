//! A device error no scope catches is counted and printed, not a panic.

use torque_render_core::Gpu;

#[test]
fn a_buffer_past_the_device_limit_is_a_counted_error_not_a_panic() {
    let gpu = match Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let max = gpu.device().limits().max_buffer_size;
    let _buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("too big"),
        size: max
            .saturating_add(4)
            .next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT),
        usage: wgpu::BufferUsages::VERTEX,
        mapped_at_creation: false,
    });
    let _ = gpu.device().poll(wgpu::PollType::wait_indefinitely());
    let errors = gpu.device_errors();
    assert_eq!(errors.validation, 1, "{errors:?}");
    assert!(errors.last.is_some());
}
