// The GPU particle pool's sprites (our own code; composed after forward.wgsl and gpu_particle_types.wgsl): one vertex entry that
// makes the same VertexOut vs_main makes of the CPU path's quads, so the forward pass's fragment entries draw them unchanged
// (fs_main, fs_wboit, fs_particle; particle_lit for a lit look).
//
// Each bucket's draw is non-indexed, six vertices a particle from its first place in the instance list (`first_vertex`): vertex
// v is corner v mod 6 of the particle at list place v / 6. A corner is TGE's `renderBillboardParticle`'s: a square of half its
// size in the camera's plane (the frame's view rows), turned by its spin times its age; the corners in the CPU path's order
// (v0 v1 v3, v1 v2 v3 of v0 (-1, -1), v1 (1, -1), v2 (1, 1), v3 (-1, 1)). A lit look's corner normal points from the center and
// its extra word is the half diagonal (what `shape_sprites` writes); a particle at a place that is not finite draws nothing.
// A look that orients its particles (words.z) turns the square instead: `setupOriented`'s, along the particle's ejection direction
// or its velocity. A look with `sizeByVelocity` stretches the square along the particle's motion in the camera's plane instead of
// turning it: its head at the particle, its tail `sizeByVelocity` seconds of that motion behind.
//
// The fill governor's scale (`sprite_fill`, written by `cs_governor` in the same frame) shrinks every sprite and thins its alpha
// when the frame's summed sprite area is past the quality's budget. A flipbook look's frame is found from the particle's age in
// its atlas: the frame in view and the next one's place (`uv2`) with the share of the next the fragment mixes in (`pool.x`). A lit
// look's sun visibility is found here once, at the particle's center, instead of at every pixel (`pool.y`).

@group(1) @binding(2) var<storage, read> sprite_particles: array<PoolParticle>;
@group(1) @binding(3) var<storage, read> sprite_instances: array<u32>;
@group(1) @binding(4) var<storage, read> sprite_looks: array<PoolLook>;
// The governor's size scale and alpha scale.
@group(1) @binding(5) var<storage, read> sprite_fill: array<f32>;

// Degrees a second at an age in ms, to radians.
const SPRITE_SPIN: f32 = (1.0 / 1000.0) * (3.14159265358979 / 180.0);

fn sprite_finite(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7f800000u) != 0x7f800000u;
}

// The cell of frame `k` in an atlas `grid` of columns and rows, as its corner in uv.
fn sprite_cell(k: u32, grid: vec2<f32>) -> vec2<f32> {
    let cols = u32(grid.x);
    return vec2<f32>(f32(k % cols), f32(k / cols)) / grid;
}

@vertex
fn vs_gpu_sprite(@builtin(vertex_index) vi: u32) -> VertexOut {
    var out: VertexOut;
    let q = sprite_particles[sprite_instances[vi / 6u]];
    var material = sprite_looks[q.look].words.x;
    let corner = vi % 6u;
    // v0 v1 v3 v1 v2 v3.
    var k = corner;
    if corner == 2u || corner == 5u {
        k = 3u;
    } else if corner == 3u {
        k = 1u;
    } else if corner == 4u {
        k = 2u;
    }
    let x = select(-1.0, 1.0, k == 1u || k == 2u);
    let z = select(-1.0, 1.0, k >= 2u);
    let right = vec3<f32>(frame.view[0][0], frame.view[1][0], frame.view[2][0]);
    let up = vec3<f32>(frame.view[0][1], frame.view[1][1], frame.view[2][1]);
    let width = q.size * 0.5 * sprite_fill[0];
    let angle = q.spin * f32(q.age) * SPRITE_SPIN;
    let s = sin(angle);
    let c = cos(angle);
    var off = (right * (x * c - z * s) + up * (x * s + z * c)) * width;
    var center = q.pos;
    // An oriented particle (TGE's `setupOriented`): a square of its size whose z axis lies along its direction (ejection, or
    // velocity: a particle at rest draws nothing) and whose x axis is across it and the line of sight; it has no spin.
    let orient = sprite_looks[q.look].words.z;
    let by_velocity = sprite_looks[q.look].fx.y;
    if orient != 0u {
        var dir = pool_orient_dir(q.flags);
        if orient == POOL_ORIENT_VELOCITY {
            let speed = length(q.vel);
            if speed > 1e-6 {
                dir = q.vel / speed;
            } else {
                out.clip = vec4<f32>(0.0, 0.0, 0.0, 1.0);
                return out;
            }
        }
        let across = cross(q.pos - frame.eye.xyz, dir);
        let across_length = length(across);
        if across_length > 1e-9 {
            off = (across / across_length * x + dir * z) * width;
        }
    } else if by_velocity > 0.0 {
        let flat = vec2<f32>(dot(q.vel, right), dot(q.vel, up));
        let speed = length(flat);
        if speed > 1e-4 {
            let d = flat / speed;
            let along = right * d.x + up * d.y;
            let side = up * d.x - right * d.y;
            let stretch = 0.5 * speed * by_velocity;
            off = along * (x * (width + stretch)) + side * (z * width);
            center = q.pos - along * stretch;
        }
    }
    let at = center + off;
    if !(sprite_finite(at.x) && sprite_finite(at.y) && sprite_finite(at.z) && sprite_finite(width)
        && sprite_finite(angle)) {
        out.clip = vec4<f32>(0.0, 0.0, 0.0, 1.0);
        return out;
    }
    let p = fx_position(at, material);
    out.clip = frame.view_proj * p;
    if (material & MAT_EMITTER) != 0u {
        out.clip = frame.unjit_view_proj * p;
    }
    out.world = p.xyz;
    out.rest = at;
    out.lod = 0.0;
    // The sprite's place in its texture: the whole of it, or its frame's cell of the atlas and the next frame's.
    let base = vec2<f32>(0.5 + 0.5 * x, 0.5 - 0.5 * z);
    out.uv = base;
    out.uv2 = base;
    var share = 0.0;
    let flip = sprite_looks[q.look].flip;
    if flip.x > 0.0 {
        let n = clamp(sprite_looks[q.look].words.y, 1u, POOL_MAX_FRAMES);
        let ticks = u32(max(q.age, 0)) * u32(max(sprite_looks[q.look].coef.w, 0.0));
        let frame_now = (ticks / 1000u) % n;
        let grid = flip.xy;
        out.uv = sprite_cell(frame_now, grid) + base / grid;
        out.uv2 = sprite_cell((frame_now + 1u) % n, grid) + base / grid;
        if flip.z > 0.5 {
            share = f32(ticks % 1000u) / 1000.0;
        }
    }
    let rgba = unpack4x8unorm(q.color);
    out.color = vec4<f32>(rgba.rgb, rgba.a * sprite_fill[1]);
    out.normal = vec3<f32>(0.0, 0.0, 1.0);
    out.extra = 0u;
    var vis = -1.0;
    let half = length(off);
    if (material & MAT_LIT) != 0u {
        if half > 1e-6 {
            out.normal = off / half;
            out.extra = bitcast<u32>(half);
            vis = particle_sun_shadow(q.pos) * cloud_sun(q.pos, sky.cloud_use.x);
        } else {
            material = material & ~MAT_LIT;
        }
    }
    out.pool = vec4<f32>(share, vis, sprite_looks[q.look].fx.x, 0.0);
    out.material = material;
    return out;
}
