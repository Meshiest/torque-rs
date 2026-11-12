// The GPU particle pool's records (our own code), shared by its compute passes (gpu_particles.wgsl) and its sprites' vertex
// stage (gpu_sprite.wgsl). Their bytes are written by torque-render-scene's `GpuSpawn::write` (a particle) and
// `SpriteRecord::write` (a look).

// A particle: where it is and how it moves, its age and life (ms), its spin (degrees a second), its look, its flags, its
// color (RGBA8, red in the low byte) and size as its last step keyed them.
struct PoolParticle {
    pos: vec3<f32>,
    age: i32,
    vel: vec3<f32>,
    life: i32,
    acc: vec3<f32>,
    spin: f32,
    look: u32,
    flags: u32,
    color: u32,
    size: f32,
};

// A look: a particle datablock's color, size and time keys, its coefficients (drag, gravity, wind, frames a second), its
// vertex material word, frame count and orient mode, each frame's bucket a byte (POOL_MAX_FRAMES), its flipbook atlas
// (columns and rows, 0 for none; 1 when a frame blends into the next) and its `softDistance` and `sizeByVelocity`.
struct PoolLook {
    colors: array<vec4<f32>, 4>,
    sizes: vec4<f32>,
    times: vec4<f32>,
    coef: vec4<f32>,
    words: vec4<u32>,
    frames: array<vec4<u32>, 2>,
    flip: vec4<f32>,
    fx: vec4<f32>,
};

const POOL_ALIVE: u32 = 1u;
// Made by the emitter's last call: the next step ages it without moving it.
const POOL_FRESH: u32 = 2u;
const POOL_MAX_FRAMES: u32 = 32u;

// The bucket of a look's frame at `age` (ms): its frames stepped at its rate (`(age * fps / 1000) mod count`), else its one.
fn pool_bucket(words: vec4<u32>, fps: f32, f0: vec4<u32>, f1: vec4<u32>, age: i32) -> u32 {
    let n = clamp(words.y, 1u, POOL_MAX_FRAMES);
    var k = 0u;
    if n > 1u {
        k = (u32(max(age, 0)) * u32(max(fps, 0.0)) / 1000u) % n;
    }
    let lane = select(f0, f1, k >= 16u);
    return (lane[(k / 4u) % 4u] >> ((k % 4u) * 8u)) & 0xffu;
}

// The unit direction a particle was ejected along, from the flags word above its two low bits: an octahedral fold, two 15-bit
// unorm coordinates (torque-render-scene's `orient_bits` writes them).
fn pool_orient_dir(flags: u32) -> vec3<f32> {
    let bits = flags >> 2u;
    let a = f32(bits & 0x7fffu) / 32767.0 * 2.0 - 1.0;
    let b = f32((bits >> 15u) & 0x7fffu) / 32767.0 * 2.0 - 1.0;
    var n = vec3<f32>(a, b, 1.0 - abs(a) - abs(b));
    if n.z < 0.0 {
        let sx = select(-1.0, 1.0, n.x >= 0.0);
        let sy = select(-1.0, 1.0, n.y >= 0.0);
        n = vec3<f32>((1.0 - abs(n.y)) * sx, (1.0 - abs(n.x)) * sy, n.z);
    }
    return normalize(n);
}

// A look's orient mode (words.z): billboards, along the ejection direction, along the velocity.
const POOL_ORIENT_EMISSION: u32 = 1u;
const POOL_ORIENT_VELOCITY: u32 = 2u;
