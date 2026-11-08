// ANCHOR: data
use torque_engine::EngineClass;

/// A kind of rope: `datablock RopeData(Cable) { segments = 16; length = 6; };`.
#[derive(EngineClass, Clone, Debug)]
#[class(name = "RopeData", parent = "SimDataBlock")]
pub struct RopeData {
    /// The pieces the rope is cut into.
    #[field(range = 2..=64, default = 12)]
    pub segments: i32,
    /// Its length in world units.
    #[field(range = 0.1..=1000.0, default = 5.0)]
    pub length: f32,
    /// Its paint, as `"r g b a"`.
    #[field(default = "200 120 60 255")]
    pub color: [u8; 4],
}
// ANCHOR_END: data
