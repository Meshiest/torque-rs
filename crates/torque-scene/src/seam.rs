//! The seam each library table sits behind: a table of function pointers a host may replace once, before its first query.
//!
//! A library crate holds `static SEAM: Seam<Table> = Seam::new(&PLAIN)` and offers [`Seam::get`] and [`Seam::install`] as its own
//! `get` and `install`. The plain table is in use until another is installed; the first install takes for the rest of the
//! process.

use std::sync::OnceLock;

/// A table of type `T`: the plain one, or the one installed over it.
#[derive(Debug)]
pub struct Seam<T: 'static> {
    installed: OnceLock<&'static T>,
    plain: &'static T,
}

impl<T: 'static> Seam<T> {
    /// A seam whose table is `plain` until another is installed.
    pub const fn new(plain: &'static T) -> Self {
        Seam {
            installed: OnceLock::new(),
            plain,
        }
    }

    /// The table in use: the installed one, else the plain one.
    #[inline]
    pub fn get(&self) -> &'static T {
        self.installed.get().copied().unwrap_or(self.plain)
    }

    /// Installs `table` for the rest of the process. Only the first install takes; installing the same table again is a
    /// no-op. Returns false when another table was installed first.
    pub fn install(&self, table: &'static T) -> bool {
        std::ptr::eq(*self.installed.get_or_init(|| table), table)
    }
}

#[cfg(test)]
#[path = "seam_tests.rs"]
mod tests;
