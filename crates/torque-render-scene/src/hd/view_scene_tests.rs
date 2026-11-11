use std::ops::Deref;

use super::*;
use crate::hd::HdScene;
use crate::hd::cells::StaticCells;

/// A scene defined outside torque-render-hd: the trait is implementable from this crate alone (no wgpu).
struct Mine {
    frame: HdScene<StaticCells>,
    mode: Option<HdMode>,
    time: f32,
}

impl Default for Mine {
    fn default() -> Mine {
        Mine {
            frame: HdScene::new(StaticCells::default()),
            mode: None,
            time: 0.0,
        }
    }
}

impl Deref for Mine {
    type Target = HdScene<StaticCells>;

    fn deref(&self) -> &HdScene<StaticCells> {
        &self.frame
    }
}

impl DerefMut for Mine {
    fn deref_mut(&mut self) -> &mut HdScene<StaticCells> {
        &mut self.frame
    }
}

impl ViewScene for Mine {
    fn set_mode(&mut self, mode: Option<HdMode>) {
        self.mode = mode;
    }

    fn set_time(&mut self, seconds: f32) {
        self.time = seconds;
    }
}

#[test]
fn a_scene_without_batch_cells_takes_the_modes_and_time_and_offers_no_cells() {
    let mut scene = Mine::default();
    scene.set_mode(Some(HdMode::default()));
    scene.set_time(2.5);
    assert_eq!(scene.mode, Some(HdMode::default()));
    assert_eq!(scene.time, 2.5);
    assert!(scene.batch_cells().is_none());
}
