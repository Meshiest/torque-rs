//! Client audio in the design of the Torque Game Engine's `alx` layer:
//!
//! - [`wav`] and [`ogg`]: the sound files as `AudioBuffer::readWAV` and `readOgg` give them to OpenAL;
//!   [`buffer`]: `AudioBuffer::find` (the `.wav` name's `.ogg` twin) and `alxGetWaveLen`.
//! - [`system`]: the `alx` source table, handles, channel volumes, listener, looping images and `alxUpdate`; [`al`]: the enum names;
//!   [`gain`]: the volume curve (none by default, as Torque3D: the volume is the gain; a host may give a 128-entry table, looked
//!   up as TGE looks up its own).
//! - [`mix`]: the voices mixed; `device` (feature `device`): a `cpal` output stream. Without a device the host moves the voices on
//!   its own clock ([`system::Audio::advance`], the null sink): every test runs so and no sound is ever played on the machine.
//! - `natives` (feature `script`): the `alx` console functions over the layer, and the client side of `AudioProfile`.
//!
//! Sound files are hostile input (downloaded content): every read is capped ([`MAX_SOUND_FILE_BYTES`], [`MAX_PCM_BYTES`],
//! [`ogg::MAX_OGG_PACKETS`]) and gives a value or an error, never a panic (`fuzz`: 50,000 cases), and never an allocation sized by a
//! header field the file does not back ([`vorbis_check`]).
//!
//! # Maintaining
//!
//! **Fuzz timing.** `fuzz::hostile_sound_files_never_panic` reports its slowest case and fails past a bound; a decoder's work must be
//! tied to its input.

pub mod al;
pub mod buffer;
#[cfg(feature = "device")]
pub mod device;
#[cfg(test)]
mod fuzz;
pub mod gain;
pub mod mix;
#[cfg(feature = "script")]
pub mod natives;
pub mod ogg;
#[cfg(feature = "engine")]
pub mod plugin;
pub mod system;
pub mod vorbis_check;
pub mod wav;

pub use buffer::{Buffer, BufferCache, Found, SoundFiles};
pub use gain::GainTable;
pub use system::{Audio, Description};

/// The largest sound file read (a hard cap of this crate).
pub const MAX_SOUND_FILE_BYTES: usize = 64 << 20;
/// The most bytes of samples one buffer holds (a hard cap: the formats allow what the file says).
pub const MAX_PCM_BYTES: usize = 128 << 20;
/// The most file names whose buffers (or failures) are kept.
pub const MAX_CACHED_BUFFERS: usize = 8_192;
/// The most bytes of samples the cached buffers hold together (a hard cap; the least recently used buffers nobody holds are
/// evicted past it).
pub const MAX_TOTAL_PCM_BYTES: usize = 512 << 20;
