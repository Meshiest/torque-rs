//! The WASI imports every guest of this crate gets, answered as `web/lua.js` answers them: no files, no environment, no
//! arguments, no exit; the guest's own clocks (one per instance, starting with it, in steps of 1 µs: never the process's
//! CPU time or the machine's wall clock), random bytes, and standard output and error printed a line at a time, rate
//! limited, with every hidden character a space. No import reaches the instance's exports, so a guest is never re-entered.

use std::hash::{BuildHasher, RandomState};
use std::ops::Range;
use std::time::{Duration, Instant};

use torque_client_script_format::is_hidden;
use wasmtime::{AsContext, AsContextMut, Caller, Linker, Memory, format_err};

/// WASI's error numbers the imports answer with.
pub(crate) const ERRNO_SUCCESS: i32 = 0;
pub(crate) const ERRNO_BADF: i32 = 8;
pub(crate) const ERRNO_INVAL: i32 = 28;
pub(crate) const ERRNO_NOTSUP: i32 = 58;
pub(crate) const ERRNO_NOTCAPABLE: i32 = 76;
/// WASI's clocks: the monotonic one and the process's and thread's CPU time (C's `clock()`); the wall clock is refused.
const MONOTONIC: i32 = 1;
const PROCESS_CPUTIME: i32 = 2;
const THREAD_CPUTIME: i32 = 3;
/// The step of the guest's clocks: one microsecond, the resolution they advertise.
const CLOCK_STEP_NANOS: u64 = 1_000;
/// A line the guest writes to its standard output or error is printed in pieces of this many bytes at most.
pub(crate) const MAX_LINE: usize = 4096;
/// How many texts an argument list (or `fd_write`'s list of buffers) may name.
pub(crate) const MAX_TEXTS: u32 = 1024;
/// The most bytes such a list may name in all: past it the list is refused before anything is copied.
pub(crate) const MAX_TEXT_BYTES: usize = 4 << 20;
/// What a guest writes to standard output and error: at most this many lines a second are printed, and one notice for
/// those dropped.
pub(crate) const LINES_PER_SECOND: u32 = 20;

/// The state the WASI imports keep for one instance.
pub(crate) struct Wasi {
    /// The instance's exported memory, once it is made.
    pub(crate) memory: Option<Memory>,
    /// When the instance was made: its clocks start here.
    started: Instant,
    random: RandomState,
    draws: u64,
    pub(crate) printer: Printer,
}

impl Wasi {
    /// The state of a new instance, whose output lines are printed after `name`.
    pub(crate) fn new(name: &'static str) -> Wasi {
        Wasi {
            memory: None,
            started: Instant::now(),
            random: RandomState::new(),
            draws: 0,
            printer: Printer::new(name),
        }
    }
}

/// A store's data that holds a [`Wasi`].
pub(crate) trait WasiData: 'static {
    fn wasi(&self) -> &Wasi;
    fn wasi_mut(&mut self) -> &mut Wasi;
}

impl WasiData for Wasi {
    fn wasi(&self) -> &Wasi {
        self
    }

    fn wasi_mut(&mut self) -> &mut Wasi {
        self
    }
}

/// The guest's memory.
pub(crate) fn memory<T: WasiData>(store: &impl AsContext<Data = T>) -> wasmtime::Result<Memory> {
    store
        .as_context()
        .data()
        .wasi()
        .memory
        .ok_or_else(|| format_err!("the guest has no memory yet"))
}

/// The range of the `len` bytes at `ptr`, checked to lie in the guest's memory (a range out of it traps the guest).
pub(crate) fn range<T: WasiData>(
    store: &impl AsContext<Data = T>,
    ptr: i32,
    len: i32,
) -> wasmtime::Result<Range<usize>> {
    let size = memory(store)?.data_size(store);
    let start = ptr as u32 as usize;
    start
        .checked_add(len as u32 as usize)
        .filter(|&end| end <= size)
        .map(|end| start..end)
        .ok_or_else(|| format_err!("the guest named bytes outside its memory"))
}

/// The `len` bytes at `ptr` in the guest's memory (a pointer out of it traps the guest).
pub(crate) fn read<T: WasiData>(
    store: &impl AsContext<Data = T>,
    ptr: i32,
    len: i32,
) -> wasmtime::Result<Vec<u8>> {
    let range = range(store, ptr, len)?;
    Ok(memory(store)?.data(store)[range].to_vec())
}

/// Writes `bytes` at `ptr` in the guest's memory.
pub(crate) fn write<T: WasiData>(
    store: &mut impl AsContextMut<Data = T>,
    ptr: i32,
    bytes: &[u8],
) -> wasmtime::Result<()> {
    let memory = memory(store)?;
    let start = ptr as u32 as usize;
    let end = start.checked_add(bytes.len());
    end.and_then(|end| memory.data_mut(&mut *store).get_mut(start..end))
        .ok_or_else(|| format_err!("the guest named bytes outside its memory"))?
        .copy_from_slice(bytes);
    Ok(())
}

pub(crate) fn write_u32<T: WasiData>(
    store: &mut impl AsContextMut<Data = T>,
    ptr: i32,
    value: u32,
) -> wasmtime::Result<()> {
    write(store, ptr, &value.to_le_bytes())
}

fn write_u64<T: WasiData>(
    store: &mut impl AsContextMut<Data = T>,
    ptr: i32,
    value: u64,
) -> wasmtime::Result<()> {
    write(store, ptr, &value.to_le_bytes())
}

/// The ranges of the `count` texts (a pointer and a length each) at `ptr`, each checked to lie in the guest's memory
/// (one out of it traps the guest): `None` when there are more than [`MAX_TEXTS`] or they name more than
/// [`MAX_TEXT_BYTES`] in all.
pub(crate) fn text_ranges<T: WasiData>(
    store: &impl AsContext<Data = T>,
    ptr: i32,
    count: i32,
) -> wasmtime::Result<Option<Vec<Range<usize>>>> {
    let count = count as u32;
    if count > MAX_TEXTS {
        return Ok(None);
    }
    let list = read(store, ptr, (count * 8) as i32)?;
    let mut ranges = Vec::with_capacity(count as usize);
    let mut total = 0usize;
    for pair in list.as_chunks::<8>().0 {
        let ptr = i32::from_le_bytes([pair[0], pair[1], pair[2], pair[3]]);
        let len = i32::from_le_bytes([pair[4], pair[5], pair[6], pair[7]]);
        let range = range(store, ptr, len)?;
        match total.checked_add(range.len()) {
            Some(sum) if sum <= MAX_TEXT_BYTES => total = sum,
            _ => return Ok(None),
        }
        ranges.push(range);
    }
    Ok(Some(ranges))
}

/// The `count` texts (a pointer and a length each) at `ptr`: `None` when [`text_ranges`] refuses them.
pub(crate) fn read_texts<T: WasiData>(
    store: &impl AsContext<Data = T>,
    ptr: i32,
    count: i32,
) -> wasmtime::Result<Option<Vec<Vec<u8>>>> {
    let Some(ranges) = text_ranges(store, ptr, count)? else {
        return Ok(None);
    };
    let bytes = memory(store)?.data(store);
    Ok(Some(
        ranges.into_iter().map(|r| bytes[r].to_vec()).collect(),
    ))
}

/// Defines the WASI imports in `linker`.
pub(crate) fn link<T: WasiData>(linker: &mut Linker<T>) -> wasmtime::Result<()> {
    const W: &str = "wasi_snapshot_preview1";
    linker.func_wrap(W, "args_get", |_: Caller<'_, T>, _: i32, _: i32| {
        ERRNO_SUCCESS
    })?;
    linker.func_wrap(
        W,
        "args_sizes_get",
        |mut c: Caller<'_, T>, count: i32, size: i32| {
            write_u32(&mut c, count, 0)?;
            write_u32(&mut c, size, 0)?;
            Ok(ERRNO_SUCCESS)
        },
    )?;
    linker.func_wrap(W, "environ_get", |_: Caller<'_, T>, _: i32, _: i32| {
        ERRNO_SUCCESS
    })?;
    linker.func_wrap(
        W,
        "environ_sizes_get",
        |mut c: Caller<'_, T>, count: i32, size: i32| {
            write_u32(&mut c, count, 0)?;
            write_u32(&mut c, size, 0)?;
            Ok(ERRNO_SUCCESS)
        },
    )?;
    let clock = |id: i32| matches!(id, MONOTONIC | PROCESS_CPUTIME | THREAD_CPUTIME);
    linker.func_wrap(
        W,
        "clock_res_get",
        move |mut c: Caller<'_, T>, id: i32, out: i32| {
            if !clock(id) {
                return Ok(ERRNO_NOTSUP);
            }
            write_u64(&mut c, out, CLOCK_STEP_NANOS)?;
            Ok(ERRNO_SUCCESS)
        },
    )?;
    linker.func_wrap(
        W,
        "clock_time_get",
        move |mut c: Caller<'_, T>, id: i32, _precision: i64, out: i32| {
            if !clock(id) {
                return Ok(ERRNO_NOTSUP);
            }
            let elapsed = c.data().wasi().started.elapsed().as_nanos();
            let nanos = u64::try_from(elapsed).unwrap_or(u64::MAX);
            // No sharper than the resolution the clock advertises.
            write_u64(&mut c, out, nanos - nanos % CLOCK_STEP_NANOS)?;
            Ok(ERRNO_SUCCESS)
        },
    )?;
    linker.func_wrap(
        W,
        "fd_write",
        |mut c: Caller<'_, T>, fd: i32, iovs: i32, count: i32, written: i32| {
            if fd != 1 && fd != 2 {
                return Ok(ERRNO_BADF);
            }
            let Some(ranges) = text_ranges(&c, iovs, count)? else {
                return Ok(ERRNO_INVAL);
            };
            let mut n = 0u32;
            let memory = memory(&c)?;
            let (bytes, data) = memory.data_and_store_mut(&mut c);
            let printer = &mut data.wasi_mut().printer;
            for range in ranges {
                n = n.saturating_add(range.len() as u32);
                // Streamed a line's length at a time: the output never holds more than a line and a piece.
                for piece in bytes[range].chunks(MAX_LINE) {
                    printer.write(piece);
                }
            }
            write_u32(&mut c, written, n)?;
            Ok(ERRNO_SUCCESS)
        },
    )?;
    linker.func_wrap(W, "fd_close", |_: Caller<'_, T>, _: i32| ERRNO_BADF)?;
    linker.func_wrap(W, "fd_fdstat_get", |_: Caller<'_, T>, _: i32, _: i32| {
        ERRNO_BADF
    })?;
    linker.func_wrap(W, "fd_filestat_get", |_: Caller<'_, T>, _: i32, _: i32| {
        ERRNO_BADF
    })?;
    linker.func_wrap(W, "fd_prestat_get", |_: Caller<'_, T>, _: i32, _: i32| {
        ERRNO_BADF
    })?;
    linker.func_wrap(
        W,
        "fd_prestat_dir_name",
        |_: Caller<'_, T>, _: i32, _: i32, _: i32| ERRNO_BADF,
    )?;
    linker.func_wrap(
        W,
        "fd_read",
        |_: Caller<'_, T>, _: i32, _: i32, _: i32, _: i32| ERRNO_BADF,
    )?;
    linker.func_wrap(
        W,
        "fd_seek",
        |_: Caller<'_, T>, _: i32, _: i64, _: i32, _: i32| ERRNO_BADF,
    )?;
    linker.func_wrap(
        W,
        "path_filestat_get",
        |_: Caller<'_, T>, _: i32, _: i32, _: i32, _: i32, _: i32| ERRNO_NOTCAPABLE,
    )?;
    linker.func_wrap(
        W,
        "path_open",
        |_: Caller<'_, T>,
         _: i32,
         _: i32,
         _: i32,
         _: i32,
         _: i32,
         _: i64,
         _: i64,
         _: i32,
         _: i32| ERRNO_NOTCAPABLE,
    )?;
    linker.func_wrap(
        W,
        "proc_exit",
        |_: Caller<'_, T>, code: i32| -> wasmtime::Result<()> {
            Err(format_err!("the guest exited ({code})"))
        },
    )?;
    linker.func_wrap(
        W,
        "random_get",
        |mut c: Caller<'_, T>, ptr: i32, len: i32| {
            // The range is checked first, then filled in place: nothing of its size is made on this side.
            let range = range(&c, ptr, len)?;
            if range.len() > MAX_TEXT_BYTES {
                return Ok(ERRNO_INVAL);
            }
            let memory = memory(&c)?;
            let (bytes, data) = memory.data_and_store_mut(&mut c);
            let wasi = data.wasi_mut();
            for chunk in bytes[range].chunks_mut(8) {
                wasi.draws = wasi.draws.wrapping_add(1);
                let word = wasi.random.hash_one(wasi.draws).to_le_bytes();
                chunk.copy_from_slice(&word[..chunk.len()]);
            }
            Ok(ERRNO_SUCCESS)
        },
    )?;
    linker.func_wrap(W, "sched_yield", |_: Caller<'_, T>| ERRNO_SUCCESS)?;
    Ok(())
}

/// What a guest writes to standard output and error, printed on this process's standard error a line at a time (a
/// line that never ends in pieces of [`MAX_LINE`]), at most [`LINES_PER_SECOND`] lines a second.
pub(crate) struct Printer {
    /// What each line is printed after.
    name: &'static str,
    /// What was written since the last full line.
    pub(crate) partial: Vec<u8>,
    /// When the current second started, and the lines printed and dropped in it.
    second: Option<Instant>,
    lines: u32,
    pub(crate) dropped: u64,
    /// Lines printed in all.
    pub(crate) printed: u64,
}

impl Printer {
    fn new(name: &'static str) -> Printer {
        Printer {
            name,
            partial: Vec::new(),
            second: None,
            lines: 0,
            dropped: 0,
            printed: 0,
        }
    }

    /// Takes `bytes` of output (of at most [`MAX_LINE`]): every full line in them is printed.
    fn write(&mut self, mut bytes: &[u8]) {
        while let Some(at) = bytes.iter().position(|&b| b == b'\n') {
            self.partial.extend_from_slice(&bytes[..at]);
            let line = std::mem::take(&mut self.partial);
            self.line(&line);
            bytes = &bytes[at + 1..];
        }
        self.partial.extend_from_slice(bytes);
        while self.partial.len() > MAX_LINE {
            let rest = self.partial.split_off(MAX_LINE);
            let line = std::mem::replace(&mut self.partial, rest);
            self.line(&line);
        }
    }

    /// Prints one line, its hidden characters (escapes, direction controls) as spaces, unless this second's lines are
    /// spent.
    pub(crate) fn line(&mut self, text: &[u8]) {
        let now = Instant::now();
        if self
            .second
            .is_none_or(|start| now.duration_since(start) >= Duration::from_secs(1))
        {
            if self.dropped > 0 {
                eprintln!("{}: ({} lines dropped)", self.name, self.dropped);
            }
            self.second = Some(now);
            self.lines = 0;
            self.dropped = 0;
        }
        if self.lines >= LINES_PER_SECOND {
            self.dropped += 1;
            return;
        }
        self.lines += 1;
        self.printed += 1;
        let plain: String = String::from_utf8_lossy(text)
            .chars()
            .map(|c| if is_hidden(c) { ' ' } else { c })
            .collect();
        eprintln!("{}: {plain}", self.name);
    }
}
