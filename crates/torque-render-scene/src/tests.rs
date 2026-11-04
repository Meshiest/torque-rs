//! Scene-level tests.

use crate::{Draw, Scene, Texture};

fn send_sync<T: Send + Sync>() {}

#[test]
fn a_frame_is_plain_data_another_thread_can_take() {
    send_sync::<Scene>();
    send_sync::<Draw>();
    send_sync::<Texture>();
}
