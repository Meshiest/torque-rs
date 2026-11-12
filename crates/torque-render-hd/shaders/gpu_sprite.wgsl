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
// or its velocity.

@group(1) @binding(2) var<storage, read> sprite_particles: array<PoolParticle>;
@group(1) @binding(3) var<storage, read> sprite_instances: array<u32>;
@group(1) @binding(4) var<storage, read> sprite_looks: array<PoolLook>;

// Degrees a second at an age in ms, to radians.
const SPRITE_SPIN: f32 = (1.0 / 1000.0) * (3.14159265358979 / 180.0);

fn sprite_finite(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7f800000u) != 0x7f800000u;
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
    let width = q.size * 0.5;
    let angle = q.spin * f32(q.age) * SPRITE_SPIN;
    let s = sin(angle);
    let c = cos(angle);
    var off = (right * (x * c - z * s) + up * (x * s + z * c)) * width;
    // An oriented particle (TGE's `setupOriented`): a square of its size whose z axis lies along its direction (ejection, or
    // velocity: a particle at rest draws nothing) and whose x axis is across it and the line of sight; it has no spin.
    let orient = sprite_looks[q.look].words.z;
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
    }
    let at = q.pos + off;
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
    out.uv = vec2<f32>(0.5 + 0.5 * x, 0.5 - 0.5 * z);
    out.color = unpack4x8unorm(q.color);
    out.normal = vec3<f32>(0.0, 0.0, 1.0);
    out.extra = 0u;
    let half = length(off);
    if (material & MAT_LIT) != 0u {
        if half > 1e-6 {
            out.normal = off / half;
            out.extra = bitcast<u32>(half);
        } else {
            material = material & ~MAT_LIT;
        }
    }
    out.material = material;
    return out;
}
