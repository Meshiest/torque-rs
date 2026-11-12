//! The tagged-string functions, following Torque3D's MIT-licensed `app/net/net.cpp` and `console/consoleFunctions.cpp`
//! (Copyright (c) 2012 GarageGames, LLC):
//! `addTaggedString`, `removeTaggedString`, `getTaggedString`, `getTag`, `detag`.

use super::{Native, Vm, VmError};
use crate::tags::tag_text;
use crate::text::atoi;

/// The text after a leading 0x01, if any (`getTaggedString` skips only a real tag byte).
fn without_tag_byte(text: &[u8]) -> &[u8] {
    text.strip_prefix(&[1u8]).unwrap_or(text)
}

pub(super) fn register(vm: &mut Vm) {
    // `StringHandle h(str)` (one engine reference), `incStringRefScript`, then the handle's destructor
    // (`removeString(id, false)`): net, one more script reference. The result has at most 8 digits.
    vm.register(
        "addTaggedString",
        2,
        2,
        "addTaggedString(text) - the tag of the text, kept until removed",
        |vm, c| {
            let Some(id) = vm.tags.add_string(c.arg(1)) else {
                return Native::Fail(VmError::TagTableFull);
            };
            if let Err(message) = vm.tags.add_script_ref(id) {
                vm.error(message);
            }
            if let Some(message) = vm.tags.remove_string(id, false) {
                vm.error(message);
            }
            Native::Str(tag_text(id, 8))
        },
    );
    // `removeString(atoi(argv[1] + 1), true)`: the first character is skipped whatever it is, so "12"
    // removes id 2.
    vm.register(
        "removeTaggedString",
        2,
        2,
        "removeTaggedString(tag) - releases a tag addTaggedString made",
        |vm, c| {
            let id = atoi(c.arg(1).get(1..).unwrap_or_default()) as u32;
            if let Some(message) = vm.tags.remove_string(id, true) {
                vm.error(message);
            }
            Native::Void
        },
    );
    vm.register(
        "getTaggedString",
        2,
        2,
        "getTaggedString(tag) - the text of a tag",
        |vm, c| {
            let id = atoi(without_tag_byte(c.arg(1))) as u32;
            match vm.tags.lookup_checked(id) {
                Ok(found) => Native::Str(found.unwrap_or_default().to_vec()),
                Err(message) => {
                    vm.error(message);
                    Native::Str(Vec::new())
                }
            }
        },
    );
    // A tagged string's id: the text after 0x01 up to the first space; anything else unchanged.
    vm.register(
        "getTag",
        2,
        2,
        "getTag(taggedText) - the tag's id",
        |_, c| {
            let text = c.arg(1);
            match text.strip_prefix(&[1u8]) {
                Some(rest) => {
                    let end = rest.iter().position(|&b| b == b' ').unwrap_or(rest.len());
                    Native::Str(rest[..end].to_vec())
                }
                None => Native::Str(text.to_vec()),
            }
        },
    );
    // A tagged string's text part: after the first space, "" without one; anything else unchanged.
    vm.register(
        "detag",
        2,
        2,
        "detag(taggedText) - the text after a tag",
        |_, c| {
            let text = c.arg(1);
            if text.first() != Some(&1) {
                return Native::Str(text.to_vec());
            }
            match text.iter().position(|&b| b == b' ') {
                Some(space) => Native::Str(text[space + 1..].to_vec()),
                None => Native::Str(Vec::new()),
            }
        },
    );
}
