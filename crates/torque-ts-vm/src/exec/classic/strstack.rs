//! The string stack: a byte buffer holding the strings of nested expressions and call arguments, with a
//! current slot (`start`, `len`) and a stack of slot offsets, as TGE 1.4's `StringStack` (`console/stringStack.h`
//! and `stringStack.cc`, Copyright (C) GarageGames.com, Inc.); the buffer is NOT cleared between uses, which is
//! part of the observable behavior.

use crate::text::{atof, atoi, to_f32, to_f64, until_nul, with_format_g, with_format_int};

pub const MAX_STACK_DEPTH: usize = 1024;
/// Arguments a call receives at most, the name not counted: TGE's argument array (`mArgV`) has `MaxArgs` (20)
/// entries, the name first.
pub const MAX_ARGS: usize = 19;
pub const RETURN_BUFFER_SPACE: usize = 512;
/// The most bytes the string stack holds (TGE has no limit): a write that would pass it is cut short and the run
/// fails with `VmError::StringTooLong`.
pub const MAX_STRING_BYTES: usize = crate::vm::MAX_STRING_BYTES;

/// What an entry of the argument array points at now (see [`StringStack::arg_text`]).
#[derive(Debug, PartialEq, Eq)]
pub enum ArgText<'a> {
    /// The null pointer that ends the array.
    Null,
    /// This text.
    Text(&'a [u8]),
    /// No terminator within the bytes looked at.
    TooLong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackError {
    Overflow,
    Underflow,
}

/// A snapshot of a [`StringStack`]'s bookkeeping, taken with [`StringStack::mark`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    start: usize,
    len: usize,
    start_offsets: usize,
    frame_offsets: usize,
}

pub struct StringStack {
    buffer: Vec<u8>,
    start: usize,
    len: usize,
    start_offsets: Vec<usize>,
    frame_offsets: Vec<usize>,
    /// Bytes written (and call arguments collected) since the last [`take_written`](Self::take_written).
    written: u64,
    /// Bytes [`string`](Self::string) scanned since the last [`take_written`](Self::take_written): an operation that reads
    /// the current string without popping it (a save, a conversion) can repeat over one long string, so each read costs
    /// its bytes.
    scanned: std::cell::Cell<u64>,
    /// A write was cut at [`MAX_STRING_BYTES`] since the last [`take_overflow`](Self::take_overflow).
    overflowed: bool,
    /// Where `argv[1]` and `argv[2]` of the argument array point, which [`args`](Self::args) sets as `getArgcArgv`
    /// does: the buffer offsets of the first two arguments of the last call, None for the null pointer (the array
    /// ends in one, so a call with one argument leaves the second slot null).
    arg_at: [Option<usize>; 2],
    /// Argument buffers a shorter argument list let go, for the next longer one ([`args_into`](Self::args_into)).
    spare_args: Vec<Vec<u8>>,
}

/// The argument buffers kept for reuse, and the largest one kept.
const SPARE_ARGS: usize = 64;
const SPARE_ARG_BYTES: usize = 1024;

impl Default for StringStack {
    fn default() -> Self {
        Self::new()
    }
}

impl StringStack {
    pub fn new() -> StringStack {
        StringStack {
            buffer: vec![0; 8192],
            start: 0,
            len: 0,
            start_offsets: Vec::new(),
            frame_offsets: Vec::new(),
            written: 0,
            scanned: std::cell::Cell::new(0),
            overflowed: false,
            arg_at: [None; 2],
            spare_args: Vec::new(),
        }
    }

    /// What `argv[1]` points at now (`slot` 0), or `argv[2]` (`slot` 1): the buffer from that argument of the last
    /// call by bytecode to its terminator, looked at for at most `limit` bytes. TGE's argument array holds pointers
    /// into the buffer, so a later write over that place shows here.
    pub fn arg_text(&self, slot: usize, limit: usize) -> ArgText<'_> {
        let Some(at) = self.arg_at.get(slot).copied().flatten() else {
            return ArgText::Null;
        };
        let rest = self.buffer.get(at..).unwrap_or_default();
        match rest.iter().take(limit).position(|&b| b == 0) {
            Some(len) => ArgText::Text(rest.get(..len).unwrap_or_default()),
            None if rest.len() >= limit => ArgText::TooLong,
            None => ArgText::Text(rest),
        }
    }

    /// A call with no arguments was made: both entries are null.
    pub fn clear_arg_slots(&mut self) {
        self.arg_at = [None; 2];
    }

    /// Makes the buffer at least `size` bytes, as far as the cap allows; false (and the overflow noted)
    /// when it does not.
    fn validate(&mut self, size: usize) -> bool {
        if size <= self.buffer.len() {
            return true;
        }
        if size > MAX_STRING_BYTES {
            // Grow to the cap, so that a write is cut there and not wherever the buffer had reached.
            if self.buffer.len() < MAX_STRING_BYTES {
                self.buffer.resize(MAX_STRING_BYTES, 0);
            }
            self.overflowed = true;
            return false;
        }
        self.buffer.resize((size + 2048).min(MAX_STRING_BYTES), 0);
        true
    }

    /// Bytes written and scanned since the last call (the VM charges them to the step budget).
    pub fn take_written(&mut self) -> u64 {
        std::mem::take(&mut self.written).saturating_add(self.scanned.take())
    }

    /// Whether a write was cut at [`MAX_STRING_BYTES`] since the last call.
    pub fn take_overflow(&mut self) -> bool {
        std::mem::take(&mut self.overflowed)
    }

    fn strlen_at(&self, at: usize) -> usize {
        self.buffer[at..]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.buffer.len() - at)
    }

    fn write(&mut self, s: &[u8]) {
        let mut s = until_nul(s);
        if !self.validate(self.start.saturating_add(s.len()).saturating_add(2)) {
            // Cut to what fits; the run fails at its next instruction.
            let room = self
                .buffer
                .len()
                .saturating_sub(self.start.saturating_add(2));
            s = s.get(..room).unwrap_or(s);
        }
        let end = self.start.saturating_add(s.len());
        if end >= self.buffer.len() {
            // At the very edge of the cap nothing fits, not even the terminator.
            self.len = 0;
            if let Some(slot) = self.buffer.get_mut(self.start) {
                *slot = 0;
            }
            return;
        }
        self.written = self.written.saturating_add(s.len() as u64);
        self.len = s.len();
        self.buffer[self.start..end].copy_from_slice(s);
        self.buffer[end] = 0;
    }

    pub fn set_string(&mut self, s: &[u8]) {
        self.write(s);
    }

    /// Writes an unsigned word the way `printf("%d")` shows it (signed).
    pub fn set_int(&mut self, value: u32) {
        with_format_int(value, |text| self.write(text));
    }

    pub fn set_float(&mut self, value: f64) {
        with_format_g(value, |text| self.write(text));
    }

    /// The current slot, up to its terminator.
    pub fn string(&self) -> &[u8] {
        let len = self.strlen_at(self.start);
        self.scanned
            .set(self.scanned.get().saturating_add(len as u64));
        let end = self.start + len;
        &self.buffer[self.start..end]
    }

    pub fn int_value(&self) -> u32 {
        atoi(self.string()) as u32
    }

    /// `OP_STR_TO_FLT`'s number: `dAtof`, the C runtime's `atof` **rounded to a float** (an `F32`), widened for the
    /// double stack. A string operand of an arithmetic expression carries only 24 bits: `"-0.500001" + 0` is
    /// -0.50000101327896 (the nearest float), not -0.500001.
    pub fn float_value(&self) -> f64 {
        to_f64(to_f32(atof(self.string())))
    }

    /// A snapshot of the bookkeeping (not of the characters), to undo what a failed nested run did.
    pub fn mark(&self) -> Mark {
        Mark {
            start: self.start,
            len: self.len,
            start_offsets: self.start_offsets.len(),
            frame_offsets: self.frame_offsets.len(),
        }
    }

    /// Returns to a [`Mark`]: drops the slots and call frames opened since, and the current slot. It
    /// never grows the stack, so a mark that is older than a state already unwound is harmless.
    pub fn restore(&mut self, mark: Mark) {
        self.start_offsets.truncate(mark.start_offsets);
        self.frame_offsets.truncate(mark.frame_offsets);
        self.start = mark.start;
        self.len = mark.len;
    }

    fn push_offset(&mut self) -> Result<(), StackError> {
        if self.start_offsets.len() >= MAX_STACK_DEPTH {
            return Err(StackError::Overflow);
        }
        self.start_offsets.push(self.start);
        Ok(())
    }

    /// Starts a new slot right after the current string.
    pub fn advance(&mut self) -> Result<(), StackError> {
        self.push_offset()?;
        self.start += self.len;
        self.len = 0;
        Ok(())
    }

    /// Like [`advance`](Self::advance) but writes `c` and a terminator first, so the next string is
    /// appended after a separator.
    pub fn advance_char(&mut self, c: u8) -> Result<(), StackError> {
        if self.start_offsets.len() >= MAX_STACK_DEPTH
            || !self.validate(self.start.saturating_add(self.len).saturating_add(3))
        {
            return Err(StackError::Overflow);
        }
        self.push_offset()?;
        self.start += self.len;
        self.buffer[self.start] = c;
        self.buffer[self.start + 1] = 0;
        self.start += 1;
        self.len = 0;
        Ok(())
    }

    /// Ends the current argument and starts the next one.
    pub fn push(&mut self) -> Result<(), StackError> {
        self.advance_char(0)
    }

    /// Returns to the previous slot; its string now includes everything appended since.
    pub fn rewind(&mut self) -> Result<(), StackError> {
        let Some(offset) = self.start_offsets.pop() else {
            return Err(StackError::Underflow);
        };
        self.start = offset;
        self.len = self.strlen_at(self.start);
        Ok(())
    }

    /// Like [`rewind`](Self::rewind) but first terminates the current slot.
    pub fn rewind_terminate(&mut self) -> Result<(), StackError> {
        if !self.validate(self.start.saturating_add(1)) {
            return Err(StackError::Overflow);
        }
        self.buffer[self.start] = 0;
        self.rewind()
    }

    /// Case-insensitive comparison of the previous slot with the current one; leaves an empty string
    /// in the previous slot.
    pub fn compare(&mut self) -> Result<bool, StackError> {
        let Some(previous) = self.start_offsets.pop() else {
            return Err(StackError::Underflow);
        };
        let current = self.start;
        self.start = previous;
        let a = &self.buffer[previous..previous + self.strlen_at(previous)];
        let b = &self.buffer[current..current + self.strlen_at(current)];
        let equal = a.eq_ignore_ascii_case(b);
        self.len = 0;
        self.buffer[self.start] = 0;
        Ok(equal)
    }

    /// Begins collecting the arguments of a call.
    pub fn push_frame(&mut self) -> Result<(), StackError> {
        if self.frame_offsets.len() >= MAX_STACK_DEPTH
            || self.start_offsets.len() >= MAX_STACK_DEPTH
        {
            return Err(StackError::Overflow);
        }
        if !self.validate(
            self.start
                .saturating_add(RETURN_BUFFER_SPACE)
                .saturating_add(2),
        ) {
            return Err(StackError::Overflow);
        }
        self.frame_offsets.push(self.start_offsets.len());
        self.start_offsets.push(self.start);
        self.start += RETURN_BUFFER_SPACE;
        Ok(())
    }

    /// Ends a call frame: returns `[name, arg1, arg2, ...]` (at most [`MAX_ARGS`] arguments) and
    /// restores the slot the call started in.
    pub fn args(&mut self, name: &[u8]) -> Result<Vec<Vec<u8>>, StackError> {
        let mut argv = Vec::new();
        self.args_into(name, &mut argv)?;
        Ok(argv)
    }

    /// [`args`](Self::args) into `argv`, reusing its strings' buffers (a pooled argument list: no allocation once warm).
    pub fn args_into(&mut self, name: &[u8], argv: &mut Vec<Vec<u8>>) -> Result<(), StackError> {
        let Some(frame) = self.frame_offsets.pop() else {
            return Err(StackError::Underflow);
        };
        let first = frame + 1;
        if self.start_offsets.len() < first {
            return Err(StackError::Underflow);
        }
        let count = (self.start_offsets.len() - first).min(MAX_ARGS);
        let mut spare = std::mem::take(&mut self.spare_args);
        while argv.len() > count + 1 {
            let Some(buffer) = argv.pop() else {
                break;
            };
            if spare.len() < SPARE_ARGS && buffer.capacity() <= SPARE_ARG_BYTES {
                spare.push(buffer);
            }
        }
        let put =
            |argv: &mut Vec<Vec<u8>>, spare: &mut Vec<Vec<u8>>, i: usize, bytes: &[u8]| match argv
                .get_mut(i)
            {
                Some(slot) => {
                    slot.clear();
                    slot.extend_from_slice(bytes);
                }
                None => {
                    let mut buffer = spare.pop().unwrap_or_default();
                    buffer.clear();
                    buffer.extend_from_slice(bytes);
                    argv.push(buffer);
                }
            };
        put(argv, &mut spare, 0, name);
        for i in 0..count {
            let at = self.start_offsets[first + i];
            let argument = &self.buffer[at..at + self.strlen_at(at)];
            // Collecting an argument copies it: charged like a write.
            self.written = self.written.saturating_add(argument.len() as u64);
            put(argv, &mut spare, i + 1, argument);
        }
        self.spare_args = spare;
        self.arg_at = std::array::from_fn(|i| (count > i).then(|| self.start_offsets[first + i]));
        let restore = self.start_offsets[first - 1];
        self.start_offsets.truncate(first - 1);
        self.start = restore;
        self.len = 0;
        Ok(())
    }
}

impl From<StackError> for crate::vm::VmError {
    fn from(e: StackError) -> crate::vm::VmError {
        match e {
            StackError::Overflow => crate::vm::VmError::StackOverflow,
            StackError::Underflow => crate::vm::VmError::StackUnderflow,
        }
    }
}
