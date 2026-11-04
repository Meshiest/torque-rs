// The engine's own extension (no extension surfaces), common part: composed with common.wgsl wherever it is. An extension's
// common chunk declares the uniforms' extension blocks: `FrameExt` (the frame's, written after the engine's fields) and `ReflExt`
// (the reflections', whose first member is `classes`: x and y the roughness of the reflective classes 1 and 2).

struct FrameExt {
    unused: vec4<f32>,
};

struct ReflExt {
    classes: vec4<f32>,
};
