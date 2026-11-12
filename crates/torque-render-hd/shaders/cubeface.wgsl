// A cube map face's directions, shared by the cubes' compute passes (the prefilter, the irradiance's projection).

// The direction a cube face's point (u, v in -1..1, v down) is sampled along (the cube map convention every API samples by).
fn face_dir(face: u32, uv: vec2<f32>) -> vec3<f32> {
    switch face {
        case 0u: {
            return vec3<f32>(1.0, -uv.y, -uv.x);
        }
        case 1u: {
            return vec3<f32>(-1.0, -uv.y, uv.x);
        }
        case 2u: {
            return vec3<f32>(uv.x, 1.0, uv.y);
        }
        case 3u: {
            return vec3<f32>(uv.x, -1.0, -uv.y);
        }
        case 4u: {
            return vec3<f32>(uv.x, -uv.y, 1.0);
        }
        default: {
            return vec3<f32>(-uv.x, -uv.y, -1.0);
        }
    }
}
