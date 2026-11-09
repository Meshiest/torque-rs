use torque_engine::GhostRecord;

// Bits 30 and 31 of a ghost's mask belong to the per-ghost block that follows every record.
#[derive(Debug, Clone, PartialEq, Default, GhostRecord)]
struct Lamp {
    #[net(mask = 1 << 30)]
    glow: Option<f32>,
}

fn main() {}
