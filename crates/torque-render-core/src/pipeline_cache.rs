//! The device's pipeline cache: the driver's compiled pipelines kept in a file between runs, so a start after a driver update
//! or a cleared driver cache does not compile every pipeline again (two seconds for the enhanced renderer's view on a cold
//! cache, and seconds more at exit while the driver writes its own cache).
//!
//! wgpu supports pipeline caches on Vulkan only (`Features::PIPELINE_CACHE`); on other backends, and in a browser, a [`Gpu`]
//! has no cache and pipelines are made as before. The file is read only from the directory a host sets ([`set_dir`], such as
//! [`user_dir`]'s); with none set a device keeps its cache in memory only. One file per adapter and driver
//! ([`file_name`]), at most [`MAX_FILE`] bytes, written whole to a temporary file and renamed over the old one. A file that is
//! missing, too large, truncated, corrupt or another adapter's is ignored (and replaced at the next save); nothing here is
//! ever fatal.
//!
//! This module holds the workspace's only `unsafe` block (in the private `create`): wgpu's `Device::create_pipeline_cache`
//! is unsafe because the driver trusts the data it is given. Its `SAFETY` comment says why the use here is sound.
//!
//! [`Gpu`]: crate::Gpu

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The largest cache file read or written (a big game's pipelines take a few MiB).
pub const MAX_FILE: u64 = 64 << 20;

/// The file's own header ahead of wgpu's data: a magic, the data's length and its FNV-1a hash (both little-endian u64).
/// wgpu checks its header (adapter, driver, version) but not the bytes after it; the hash catches a file damaged on disk.
const MAGIC: [u8; 8] = *b"TRSPIPE1";
const HEADER: usize = 24;

/// The directory cache files are read from and written to ([`set_dir`]); None keeps caches in memory.
static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Sets the directory devices made from now on load their pipeline cache from and save it to (None: no file). The directory
/// must be writable only by the current user ([`user_dir`]'s is): its files' data is handed to the GPU driver, which trusts
/// it once wgpu's and its own header checks pass.
pub fn set_dir(dir: Option<PathBuf>) {
    *DIR.lock().unwrap_or_else(|e| e.into_inner()) = dir;
}

/// The directory [`set_dir`] set.
pub fn dir() -> Option<PathBuf> {
    DIR.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The per-user cache directory of an application named `app`: the platform's cache directory (`%LOCALAPPDATA%` on
/// Windows, `~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or `~/.cache` elsewhere), then `torque-rs/<app>` (the name's
/// characters other than ASCII letters, digits, `-` and `_` become `_`). None when the platform's directory is unknown,
/// and in a browser.
pub fn user_dir(app: &str) -> Option<PathBuf> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    let absolute = |v: std::ffi::OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    let home = || std::env::var_os("HOME").and_then(absolute);
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").and_then(absolute)?
    } else if cfg!(target_os = "macos") {
        home()?.join("Library").join("Caches")
    } else {
        match std::env::var_os("XDG_CACHE_HOME").and_then(absolute) {
            Some(p) => p,
            None => home()?.join(".cache"),
        }
    };
    let mut name: String = app
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.is_empty() {
        name.push_str("app");
    }
    Some(base.join("torque-rs").join(name))
}

/// The cache file's name for an adapter: `pipelines-<key>-<driver>.bin`, the key wgpu's
/// [`pipeline_cache_key`](wgpu::util::pipeline_cache_key) (the backend, vendor and device) and `driver` a hash of the driver's
/// name and version, so another GPU's or another driver's data is never loaded. None for a backend without caches.
pub fn file_name(info: &wgpu::AdapterInfo) -> Option<String> {
    let key = wgpu::util::pipeline_cache_key(info)?;
    let mut driver = Fnv::new();
    driver.write(info.driver.as_bytes());
    driver.write(&[0]);
    driver.write(info.driver_info.as_bytes());
    Some(format!("pipelines-{key}-{:016x}.bin", driver.0))
}

/// FNV-1a, 64 bits.
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

fn hash(bytes: &[u8]) -> u64 {
    let mut h = Fnv::new();
    h.write(bytes);
    h.0
}

/// `data` with the file's header ahead of it.
pub fn encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + data.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&(data.len() as u64).to_le_bytes());
    out.extend_from_slice(&hash(data).to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// Why a file's bytes are not a cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bad {
    /// Shorter than its header, or than the length it records.
    Truncated,
    /// Not this format (another file, or a newer one).
    Magic,
    /// Longer than the length it records.
    Extended,
    /// The data's hash does not match.
    Corrupt,
}

/// The data of a file's bytes (as [`encode`] wrote them).
pub fn decode(file: &[u8]) -> Result<&[u8], Bad> {
    let (Some(magic), Some(len), Some(sum)) = (
        file.get(..8),
        file.get(8..16).and_then(|b| <[u8; 8]>::try_from(b).ok()),
        file.get(16..24).and_then(|b| <[u8; 8]>::try_from(b).ok()),
    ) else {
        return Err(Bad::Truncated);
    };
    if magic != MAGIC {
        return Err(Bad::Magic);
    }
    let data = &file[HEADER..];
    let len = u64::from_le_bytes(len);
    if (data.len() as u64) < len {
        return Err(Bad::Truncated);
    }
    if (data.len() as u64) > len {
        return Err(Bad::Extended);
    }
    if hash(data) != u64::from_le_bytes(sum) {
        return Err(Bad::Corrupt);
    }
    Ok(data)
}

/// The data of the cache file at `path`, at most [`MAX_FILE`] bytes read. None (with a note) when it is missing, too large or
/// not a whole cache file.
pub fn read_file(path: &Path) -> Option<Vec<u8>> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("pipeline cache: {}: {e}", path.display());
            return None;
        }
    };
    let mut bytes = Vec::new();
    if let Err(e) = file.take(MAX_FILE + 1).read_to_end(&mut bytes) {
        eprintln!("pipeline cache: {}: {e}", path.display());
        return None;
    }
    if bytes.len() as u64 > MAX_FILE {
        eprintln!(
            "pipeline cache: {}: larger than {MAX_FILE} bytes, ignored",
            path.display()
        );
        return None;
    }
    match decode(&bytes) {
        Ok(data) => Some(data.to_vec()),
        Err(why) => {
            eprintln!("pipeline cache: {}: {why:?}, ignored", path.display());
            None
        }
    }
}

/// Writes `data` as the cache file at `path`: whole to a temporary file beside it, then renamed over it, so a reader never
/// sees half a file. Other drivers' files for the same adapter are removed (a driver update leaves its old cache useless),
/// and so are this file's temporary files a minute old or more (a save a crash cut short).
pub fn write_file(path: &Path, data: &[u8]) -> io::Result<()> {
    if data.len() as u64 > MAX_FILE - HEADER as u64 {
        return Err(io::Error::other(format!(
            "{} bytes is larger than the {MAX_FILE} a file may hold",
            data.len()
        )));
    }
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("the cache file has no directory"))?;
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| io::Error::other("the cache file has no name"))?;
    let tmp = dir.join(format!("{name}.{}.tmp", std::process::id()));
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&encode(data))?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return written;
    }
    // `pipelines-<key>-<driver>.bin`: the same key with another driver.
    if let Some((stem, _)) = name.rsplit_once('-')
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for entry in entries.flatten() {
            let other = entry.file_name();
            let Some(other) = other.to_str() else {
                continue;
            };
            let other_driver = other != name
                && other.ends_with(".bin")
                && other.strip_prefix(stem).is_some_and(|r| r.starts_with('-'));
            // `<name>.<pid>.tmp` of a save that did not finish (a crash), a minute old or more: not one being written now.
            let stale_tmp = other.ends_with(".tmp")
                && other.strip_prefix(name).is_some_and(|r| r.starts_with('.'))
                && entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age >= std::time::Duration::from_secs(60));
            if other_driver || stale_tmp {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

/// A device's pipeline cache, the file it was loaded from and is saved to, and how many bytes of the file it accepted.
#[derive(Clone, Debug)]
pub(crate) struct DeviceCache {
    pub(crate) cache: wgpu::PipelineCache,
    pub(crate) file: Option<PathBuf>,
    pub(crate) loaded: Option<usize>,
}

/// The pipeline cache of `device` (made on the adapter `info` describes), loaded from `dir`'s file for the adapter when there
/// is one and wgpu accepts it. None when the device has no `PIPELINE_CACHE` feature (every backend but Vulkan), in a browser,
/// and when even an empty cache cannot be made.
pub(crate) async fn open(
    device: &wgpu::Device,
    info: &wgpu::AdapterInfo,
    dir: Option<&Path>,
) -> Option<DeviceCache> {
    if cfg!(target_arch = "wasm32") || !device.features().contains(wgpu::Features::PIPELINE_CACHE) {
        return None;
    }
    let file = dir.zip(file_name(info)).map(|(d, n)| d.join(n));
    let data = file.as_deref().and_then(read_file);
    if let Some(data) = data.as_deref() {
        match checked(device, Some(data)).await {
            Ok(cache) => {
                // wgpu drops data it finds outdated or cut short (a driver update) and makes an empty cache: the data was used
                // only if the cache gives at least as much back, and more than an empty cache does.
                let size = |c: &wgpu::PipelineCache| c.get_data().map_or(0, |d| d.len());
                let now = size(&cache);
                let empty = checked(device, None).await.map_or(0, |c| size(&c));
                let loaded = (now >= data.len() && now > empty).then_some(data.len());
                // A file no larger than an empty cache holds no pipelines (a driver that keeps none, such as Mesa's
                // lavapipe, saves only its header): nothing to load, and nothing outdated.
                if loaded.is_none() && data.len() > empty {
                    eprintln!("pipeline cache: the saved pipelines are outdated, starting empty");
                }
                return Some(DeviceCache {
                    cache,
                    file,
                    loaded,
                });
            }
            Err(e) => {
                eprintln!("pipeline cache: the saved pipelines were refused ({e}), starting empty")
            }
        }
    }
    match checked(device, None).await {
        Ok(cache) => Some(DeviceCache {
            cache,
            file,
            loaded: None,
        }),
        Err(e) => {
            eprintln!("pipeline cache: none made: {e}");
            None
        }
    }
}

/// [`create`] inside error scopes: a cache wgpu refused (another device's data, out of memory) is an error here rather than an
/// invalid cache that fails every pipeline made with it, and never reaches the device's uncaptured-error handler.
async fn checked(
    device: &wgpu::Device,
    data: Option<&[u8]>,
) -> Result<wgpu::PipelineCache, wgpu::Error> {
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let cache = create(device, data);
    let refused = validation.pop().await;
    let out_of_memory = memory.pop().await;
    match refused.or(out_of_memory) {
        Some(e) => Err(e),
        None => Ok(cache),
    }
}

/// A pipeline cache on `device` started from `data` (a file's, [`decode`]d), or empty.
#[expect(unsafe_code)]
fn create(device: &wgpu::Device, data: Option<&[u8]>) -> wgpu::PipelineCache {
    // SAFETY: wgpu requires `data` to be bytes an earlier `PipelineCache::get_data` returned, for an adapter with the
    // same `util::pipeline_cache_key`, because the driver trusts them. That cannot be proven of a file, so this rests
    // on what is checked. The file is read from the directory a host set (`set_dir`: one only the current user may
    // write, such as `user_dir`'s; that crosses no privilege boundary, since the driver's own caches are just as writable
    // by that user), under a name made of the pipeline cache key (backend, vendor, device) and a hash of the driver's
    // name and version, so another GPU's or driver's file is not picked. It is written whole to a temporary file and
    // renamed over the old one, so it is never read half-written, and at most `MAX_FILE` bytes are read. Its own
    // header's length and FNV-1a hash only catch accidental damage (truncation, extension, flipped bits; `decode`), not
    // a deliberate edit. Stale or foreign data is handled by the checks after that: wgpu checks its header in the data
    // (magic, header version, pointer size, backend, adapter key from the vendor and device, and a validation key that
    // on Vulkan is the 4-byte driver version), and the Vulkan driver checks its own header (vendor, device and its
    // pipeline cache UUID) and ignores data that does not match. With `fallback: true` data wgpu finds outdated or cut
    // short is dropped for an empty cache; data it finds to be another device's is refused, the error caught by
    // `checked`'s scope and an empty cache made instead. `None` data is always sound.
    unsafe {
        device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
            label: Some("pipelines"),
            data,
            fallback: true,
        })
    }
}

impl DeviceCache {
    /// Writes the cache's data to its file. Ok(None) when there is no file or no data.
    pub(crate) fn save(&self) -> io::Result<Option<usize>> {
        let Some(path) = self.file.as_deref() else {
            return Ok(None);
        };
        let Some(data) = self.cache.get_data() else {
            return Ok(None);
        };
        write_file(path, &data)?;
        Ok(Some(data.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_decodes_to_its_data() {
        let data = b"wgpu's pipeline bytes";
        assert_eq!(decode(&encode(data)), Ok(&data[..]));
        assert_eq!(decode(&encode(&[])), Ok(&[][..]));
    }

    #[test]
    fn a_damaged_file_is_refused() {
        let file = encode(b"wgpu's pipeline bytes");
        for cut in 0..file.len() {
            assert_eq!(decode(&file[..cut]), Err(Bad::Truncated), "cut at {cut}");
        }
        let mut longer = file.clone();
        longer.push(0);
        assert_eq!(decode(&longer), Err(Bad::Extended));
        let mut flipped = file.clone();
        *flipped.last_mut().unwrap() ^= 1;
        assert_eq!(decode(&flipped), Err(Bad::Corrupt));
        let mut other = file;
        other[0] = b'X';
        assert_eq!(decode(&other), Err(Bad::Magic));
    }

    #[test]
    fn the_file_is_named_by_adapter_and_driver() {
        let info = |backend, device: u32, driver: &str| wgpu::AdapterInfo {
            vendor: 0x10de,
            device,
            driver: driver.into(),
            driver_info: "1.2.3".into(),
            ..wgpu::AdapterInfo::new(wgpu::DeviceType::DiscreteGpu, backend)
        };
        let vk = file_name(&info(wgpu::Backend::Vulkan, 1, "NVIDIA")).unwrap();
        assert!(
            vk.starts_with("pipelines-wgpu_pipeline_cache_vulkan_4318_1-") && vk.ends_with(".bin")
        );
        assert_ne!(
            Some(vk.clone()),
            file_name(&info(wgpu::Backend::Vulkan, 2, "NVIDIA"))
        );
        assert_ne!(
            Some(vk),
            file_name(&info(wgpu::Backend::Vulkan, 1, "other"))
        );
        assert_eq!(file_name(&info(wgpu::Backend::Dx12, 1, "NVIDIA")), None);
    }

    #[test]
    fn a_user_dir_is_named_after_the_app() {
        if let Some(d) = user_dir("my game/1") {
            assert!(d.ends_with(Path::new("torque-rs").join("my_game_1")));
        }
    }
}
