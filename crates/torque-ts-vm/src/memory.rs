//! The memory scripts hold: one account per [`Vm`](crate::vm::Vm) that its variables (globals and local frames), its
//! objects and their dynamic fields count their bytes in, and a limit on the total.
//!
//! A store that would take the account past its limit is refused: the value is not stored (an entry that cannot be
//! added is not added), the account notes the refusal, and the run ends with
//! [`VmError::MemoryLimit`](crate::vm::VmError::MemoryLimit) at its next instruction. What is held stays at or under the
//! limit, so the next run still runs, and a store that frees memory always succeeds. The limit is the host's
//! [`Limits::max_script_bytes`](crate::policy::Limits::max_script_bytes), capped at [`MAX_SCRIPT_BYTES`], and lowered to
//! the pref [`MEMORY_LIMIT_PREF`] (in MiB, read at the start of each outermost run) when that is smaller: a script can
//! tighten the limit, never raise it.
//!
//! The values on the evaluation stacks and in Torque3D's registers are not counted: they last one run, and making them
//! costs the run's step budget.

use std::cell::Cell;
use std::rc::Rc;

/// The most bytes scripts may hold, whatever a pref or a host asks for.
pub const MAX_SCRIPT_BYTES: u64 = 4 << 30;

/// The limit a `Vm` starts with ([`Limits::max_script_bytes`](crate::policy::Limits::max_script_bytes)'s default).
pub const DEFAULT_SCRIPT_BYTES: u64 = 1 << 30;

/// The global a game sets to lower the limit, in MiB (from 1); unset, not a number or above the host's limit, the
/// host's limit holds. Its writes have the engine effect ([`Effects::ENGINE`](crate::effects::Effects::ENGINE)), so a
/// language whose policy denies those cannot change it.
pub const MEMORY_LIMIT_PREF: &str = "$Pref::Script::MemoryLimit";

/// What a variable counts besides its name and value: its entry, its slot and its index key.
pub const ENTRY_COST: usize = 128;

/// What an object counts besides its dynamic fields.
pub const OBJECT_COST: usize = 512;

/// What a dynamic field counts besides its name, spelling and value: its map node.
pub const FIELD_COST: usize = 64;

/// The limit [`MEMORY_LIMIT_PREF`]'s text asks for, in bytes, capped at [`MAX_SCRIPT_BYTES`]; None when it is no number.
pub(crate) fn pref_limit(text: &[u8]) -> Option<u64> {
    let mib: u64 = std::str::from_utf8(text).ok()?.trim().parse().ok()?;
    Some(mib.clamp(1, MAX_SCRIPT_BYTES >> 20) << 20)
}

/// A shared count of held bytes with a limit (see the module documentation). Cloning gives another handle on the same
/// account.
#[derive(Clone, Debug)]
pub struct MemoryAccount(Rc<Inner>);

#[derive(Debug)]
struct Inner {
    held: Cell<u64>,
    limit: Cell<u64>,
    refused: Cell<bool>,
}

impl Default for MemoryAccount {
    /// An account with no limit.
    fn default() -> MemoryAccount {
        MemoryAccount::with_limit(u64::MAX)
    }
}

impl MemoryAccount {
    #[must_use]
    pub fn with_limit(limit: u64) -> MemoryAccount {
        MemoryAccount(Rc::new(Inner {
            held: Cell::new(0),
            limit: Cell::new(limit),
            refused: Cell::new(false),
        }))
    }

    /// The bytes held.
    pub fn held(&self) -> u64 {
        self.0.held.get()
    }

    pub fn limit(&self) -> u64 {
        self.0.limit.get()
    }

    pub fn set_limit(&self, limit: u64) {
        self.0.limit.set(limit);
    }

    /// Whether `extra` more bytes fit under the limit.
    pub fn fits(&self, extra: usize) -> bool {
        self.held().saturating_add(extra as u64) <= self.limit()
    }

    /// Notes that a store was refused: the run ends with `MemoryLimit` at its next instruction
    /// ([`take_refused`](Self::take_refused)).
    pub fn refuse(&self) {
        self.0.refused.set(true);
    }

    /// Something that held `before` bytes holds `after` now.
    pub fn settle(&self, before: usize, after: usize) {
        let held = self.held().saturating_sub(before as u64);
        self.0.held.set(held.saturating_add(after as u64));
    }

    /// Whether a store was refused since the last call.
    pub fn take_refused(&self) -> bool {
        self.0.refused.replace(false)
    }

    /// Whether both handles are on one account.
    pub fn same(&self, other: &MemoryAccount) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Bytes held on an account for as long as the charge lives: what a class or the runtime keeps outside the variables and
/// fields (a loop's text, a block of compiled code, a control's text) counts through one. Dropping it gives the bytes back.
#[derive(Debug)]
pub struct MemoryCharge {
    account: MemoryAccount,
    bytes: usize,
}

impl MemoryCharge {
    /// A charge of nothing yet on `account`.
    #[must_use]
    pub fn new(account: &MemoryAccount) -> MemoryCharge {
        MemoryCharge {
            account: account.clone(),
            bytes: 0,
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Whether the charge can be `bytes` (always when it shrinks).
    pub fn fits(&self, bytes: usize) -> bool {
        bytes <= self.bytes || self.account.fits(bytes - self.bytes)
    }

    /// Makes the charge `bytes`; the caller has asked [`fits`](Self::fits) when it grows.
    pub fn set(&mut self, bytes: usize) {
        self.account.settle(self.bytes, bytes);
        self.bytes = bytes;
    }

    /// [`set`](Self::set) when it [`fits`](Self::fits); otherwise the refusal is noted on the account
    /// ([`MemoryAccount::refuse`]), nothing changes, and false.
    pub fn resize(&mut self, bytes: usize) -> bool {
        if !self.fits(bytes) {
            self.account.refuse();
            return false;
        }
        self.set(bytes);
        true
    }
}

impl Drop for MemoryCharge {
    fn drop(&mut self) {
        self.account.settle(self.bytes, 0);
    }
}
