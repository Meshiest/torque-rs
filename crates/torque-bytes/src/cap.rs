//! The error of a count or size above its cap.

use std::fmt;

/// A count or size above its cap: what was counted, the value found and the cap it passed. Both numbers are kept so a message
/// can say how far over it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapExceeded {
    pub what: &'static str,
    pub value: u64,
    pub cap: u64,
}

impl CapExceeded {
    /// Passes when `value` is at most `cap`.
    ///
    /// # Errors
    /// [`CapExceeded`] naming `what`, when `value` is above `cap`.
    pub fn check(what: &'static str, value: u64, cap: u64) -> Result<(), CapExceeded> {
        if value > cap {
            Err(CapExceeded { what, value, cap })
        } else {
            Ok(())
        }
    }
}

impl fmt::Display for CapExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} is above the limit of {}",
            self.what, self.value, self.cap
        )
    }
}

impl std::error::Error for CapExceeded {}

#[cfg(test)]
#[path = "cap_tests.rs"]
mod tests;
