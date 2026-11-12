//! The readers a native uses on its arguments.
//!
//! A script's arguments are byte strings that may hold any byte: the readers never fail and never index past the
//! arguments given. A missing argument reads as the empty string, which every number reader turns into 0.

use super::{NativeCall, Vm};
use crate::fields::atob;
use crate::sim::ObjId;
use crate::text::{atof, atoi, scan_floats, to_f32, until_nul};

impl NativeCall {
    /// How many entries `argv` has: the called name and then the arguments.
    #[must_use]
    pub fn argc(&self) -> usize {
        self.argv.len()
    }

    /// Argument `i` as given (`argv[i]`, `argv[0]` being the name): all its bytes, NULs included; empty when the call
    /// has no such argument.
    #[must_use]
    pub fn arg(&self, i: usize) -> &[u8] {
        self.argv.get(i).map_or(&[][..], Vec::as_slice)
    }

    /// Argument `i` as a C string: its bytes up to the first NUL; empty when missing.
    #[must_use]
    pub fn cstr(&self, i: usize) -> &[u8] {
        until_nul(self.arg(i))
    }

    /// Argument `i` as `dAtoi` reads it: white space, a sign and digits, saturating at the `i32` range; 0 when missing
    /// or not a number.
    #[must_use]
    pub fn int(&self, i: usize) -> i32 {
        atoi(self.arg(i))
    }

    /// Argument `i` as `dAtof` reads it, converted to a float the way a C cast does ([`to_f32`]: round to nearest,
    /// a NaN quieted with its sign and top payload bits kept); 0 when missing or not a number.
    #[must_use]
    pub fn float(&self, i: usize) -> f32 {
        to_f32(atof(self.arg(i)))
    }

    /// Argument `i` as `dAtob` reads it ([`atob`]): `true` in any case, or a number that is not 0 as a float;
    /// `false` when missing.
    #[must_use]
    pub fn boolean(&self, i: usize) -> bool {
        atob(self.arg(i))
    }

    /// Argument `i` as `sscanf(s, "%f %f %f")` into zeros: the numbers the text gives, 0 for the others.
    #[must_use]
    pub fn vec3(&self, i: usize) -> [f32; 3] {
        let mut v = [0.0; 3];
        scan_floats(self.arg(i), &mut v);
        v
    }

    /// The arguments from index `from` on (none past the last).
    #[must_use]
    pub fn rest(&self, from: usize) -> &[Vec<u8>] {
        self.argv.get(from..).unwrap_or_default()
    }

    /// The object of the method that is running, if it still exists.
    #[must_use]
    pub fn this(&self, vm: &Vm) -> Option<ObjId> {
        self.this_object.filter(|id| vm.objects.contains(*id))
    }
}

#[cfg(test)]
#[path = "call_tests.rs"]
mod tests;
