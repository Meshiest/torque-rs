//! The OpenAL enum names the `alx` functions take: a table of 17 rows of name, value and flags, looked up case-insensitively, the
//! first row whose flags hold every bit of the caller's mask.

pub const AL_GAIN: u32 = 0x100a;
/// TGE's own: a linear volume, mapped through the volume curve to `AL_GAIN`.
pub const AL_GAIN_LINEAR: u32 = 0xff01;
pub const AL_PITCH: u32 = 0x1003;
pub const AL_REFERENCE_DISTANCE: u32 = 0x1020;
pub const AL_MAX_DISTANCE: u32 = 0x1023;
pub const AL_CONE_OUTER_GAIN: u32 = 0x1022;
pub const AL_POSITION: u32 = 0x1004;
pub const AL_DIRECTION: u32 = 0x1005;
pub const AL_VELOCITY: u32 = 0x1006;
pub const AL_ORIENTATION: u32 = 0x100f;
pub const AL_CONE_INNER_ANGLE: u32 = 0x1001;
pub const AL_CONE_OUTER_ANGLE: u32 = 0x1002;
pub const AL_LOOPING: u32 = 0x1007;
pub const AL_VENDOR: u32 = 0xb001;
pub const AL_VERSION: u32 = 0xb002;
pub const AL_RENDERER: u32 = 0xb003;
pub const AL_EXTENSIONS: u32 = 0xb004;
/// Not in the table: the source state `alxIsPlaying` and `alxUpdate` read, the relative flag, the buffer.
pub const AL_SOURCE_STATE: u32 = 0x1010;
pub const AL_SOURCE_RELATIVE: u32 = 0x202;

/// The callers' masks: `alGetString` 0x14; `alxListenerf` 0xa2; `alxGetListenerf` 0x91; `alxSourcef` 0xa1; `alxGetSourcef` 0x91;
/// `alxSourcei` 0x61; `alxGetSourcei` 0x51; `alxSource3f` 0x121, `alListener3f` 0x122; and (scalar names only) `alxGetSource3f`
/// and `alGetListener3f` 0x91, `alGetListeneri` 0x51.
pub const MASK_GET_STRING: u32 = 0x14;
pub const MASK_LISTENER_F: u32 = 0xa2;
pub const MASK_GET_LISTENER_F: u32 = 0x91;
pub const MASK_SOURCE_F: u32 = 0xa1;
pub const MASK_GET_SOURCE_F: u32 = 0x91;
pub const MASK_SOURCE_I: u32 = 0x61;
pub const MASK_GET_SOURCE_I: u32 = 0x51;
pub const MASK_SOURCE_3F: u32 = 0x121;
pub const MASK_GET_SOURCE_3F: u32 = 0x91;
pub const MASK_LISTENER_3F: u32 = 0x122;
pub const MASK_GET_LISTENER_3F: u32 = 0x91;
pub const MASK_GET_LISTENER_I: u32 = 0x51;

/// The table, in lookup order.
pub const ENUMS: [(&str, u32, u32); 17] = [
    ("AL_GAIN", AL_GAIN, 0xb3),
    ("AL_GAIN_LINEAR", AL_GAIN_LINEAR, 0xb3),
    ("AL_PITCH", AL_PITCH, 0xb1),
    ("AL_REFERENCE_DISTANCE", AL_REFERENCE_DISTANCE, 0xb1),
    ("AL_MAX_DISTANCE", AL_MAX_DISTANCE, 0xb1),
    ("AL_CONE_OUTER_GAIN", AL_CONE_OUTER_GAIN, 0xb1),
    ("AL_POSITION", AL_POSITION, 0x133),
    ("AL_DIRECTION", AL_DIRECTION, 0x131),
    ("AL_VELOCITY", AL_VELOCITY, 0x133),
    ("AL_ORIENTATION", AL_ORIENTATION, 0x222),
    ("AL_CONE_INNER_ANGLE", AL_CONE_INNER_ANGLE, 0x71),
    ("AL_CONE_OUTER_ANGLE", AL_CONE_OUTER_ANGLE, 0x71),
    ("AL_LOOPING", AL_LOOPING, 0x71),
    ("AL_VENDOR", AL_VENDOR, 0x14),
    ("AL_VERSION", AL_VERSION, 0x14),
    ("AL_RENDERER", AL_RENDERER, 0x14),
    ("AL_EXTENSIONS", AL_EXTENSIONS, 0x14),
];

/// The value of the first row named `name` (any case) whose flags hold all of `mask`.
pub fn lookup(name: &[u8], mask: u32) -> Option<u32> {
    ENUMS
        .iter()
        .find(|(n, _, flags)| flags & mask == mask && n.as_bytes().eq_ignore_ascii_case(name))
        .map(|(_, v, _)| *v)
}

#[cfg(test)]
#[path = "al_tests.rs"]
mod tests;
