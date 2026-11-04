// The engine's own extension, shadow part: composed after shadow.wgsl. An extension's shadow chunk declares `CasterExt` (the
// casters' uniform's extension block) and `ext_excluded`, and may bind the casters' group 3 bindings 0 and 1.

struct CasterExt {
    unused: vec4<f32>,
};

// Whether a point light leaves a vertex of this extra word out of its own cube: the objects it is mounted in.
fn ext_excluded(extra: u32) -> bool {
    return any(caster.exclude == vec4<u32>(extra));
}
