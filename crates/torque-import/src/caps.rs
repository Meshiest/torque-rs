//! The bounds every reader holds a file to, each checked before the work it bounds. They are generous: well above any model a
//! game ships, well below what would exhaust a client.

/// The largest model file read (a `.glb`, or a `.gltf`'s JSON).
pub const MAX_FILE_BYTES: usize = 64 << 20;
/// The largest JSON part of a glTF.
pub const MAX_JSON_BYTES: usize = 16 << 20;
/// The largest glTF a datablock may name ([`crate::native`]: a file a server can send), and its largest JSON part.
pub const MAX_NATIVE_FILE_BYTES: usize = 16 << 20;
pub const MAX_NATIVE_JSON_BYTES: usize = 2 << 20;
/// Values of a glTF's JSON (objects, arrays, strings, and every element and member), counted in one pass before it is
/// parsed: each costs the parse (and its validation) far more memory than its text, so their number is held under this
/// before any is built.
pub const MAX_JSON_VALUES: usize = 1 << 18;
/// All buffers of one model together (the `.glb`'s binary chunk, side files, `data:` URIs decoded).
pub const MAX_BUFFER_BYTES: usize = 128 << 20;
/// The longest `data:` URI, in bytes of text.
pub const MAX_DATA_URI_BYTES: usize = 48 << 20;
/// The longest URI of a side file.
pub const MAX_URI_BYTES: usize = 1_024;
/// Nodes of one model (a shape's own cap).
pub const MAX_NODES: usize = torque_shapes::MAX_SHAPE_NODES;
/// The deepest node: parents above it.
pub const MAX_NODE_DEPTH: usize = 128;
/// Meshes of one model, each node's mesh counted once a node.
pub const MAX_MESHES: usize = 4_096;
/// Primitives of all meshes together.
pub const MAX_PRIMITIVES: usize = 16_384;
/// Elements of one accessor.
pub const MAX_ACCESSOR_ELEMENTS: usize = 1 << 20;
/// Vertices of one model, every mesh's counted (each primitive's, before its arrays are read).
pub const MAX_VERTICES: usize = 1 << 20;
/// Triangle indices of one model.
pub const MAX_INDICES: usize = 3 << 20;
/// Joints of one skin.
pub const MAX_SKIN_JOINTS: usize = 1_024;
/// Joints of every skinned mesh together.
pub const MAX_SKIN_JOINTS_TOTAL: usize = 1 << 16;
/// Key times and values read for every animation channel together.
pub const MAX_KEY_ELEMENTS: usize = 1 << 22;
/// Animations of one model (a shape's sequence cap).
pub const MAX_ANIMATIONS: usize = torque_shapes::MAX_SEQUENCES;
/// Channels of one animation.
pub const MAX_CHANNELS: usize = 16_384;
/// The longest animation, in seconds.
pub const MAX_DURATION: f32 = 3_600.0;
/// Keys of one sampled animation.
pub const MAX_KEYS: usize = 1 << 17;
/// Sampled values of one model, every track's keys counted.
pub const MAX_SAMPLES: usize = 1 << 21;
/// Materials of one model (a shape's own cap).
pub const MAX_MATERIALS: usize = torque_shapes::MAX_MATERIALS;
/// Images of one model.
pub const MAX_IMAGES: usize = 1_024;
/// The largest image carried in the model, in bytes.
pub const MAX_IMAGE_BYTES: usize = 32 << 20;
/// Every image carried in the model together, in bytes.
pub const MAX_IMAGE_BYTES_TOTAL: usize = 64 << 20;
/// The widest or tallest image a header may give.
pub const MAX_IMAGE_SIDE: u32 = 8_192;
/// The longest name kept; longer ones are cut (at a character boundary).
pub const MAX_NAME_BYTES: usize = 255;
