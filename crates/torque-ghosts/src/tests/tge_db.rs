//! TGE's datablock records: each class's `(field, width)` list written by hand from its `packData`, the bit totals of the
//! record a datablock with TGE's constructor defaults sends, the packers' quantisers, round trips in both directions, the caps,
//! and hostile decodes of every class through [`TgeDatablocks`](crate::TgeDatablocks).

use super::tge_shapes::canonical;
use super::*;
use crate::codec::F32;

mod audio;
mod effects;
mod set;
mod shapes;
mod vehicles;
mod weather;

/// An empty string's bits with the packet's string buffer: the shared-prefix flag, the compressed flag and the 8-bit length.
const EMPTY: usize = 10;

/// The datablock id's width, in the connection's widths.
fn db() -> usize {
    db_bits()
}

/// A golden list with `{db}` standing for the datablock id's width.
fn spec(s: &str) -> String {
    s.replace("{db}", &db().to_string())
}

/// `n` copies of a golden fragment.
fn times(fragment: &str, n: usize) -> String {
    vec![fragment; n].join(" ")
}

/// Checks a record's `(field, width)` list and bit total, and that it round trips both ways.
fn check<L: Layout + PartialEq + Debug>(rec: &L, golden: &str, bits: usize) {
    check_log(rec, &spec(golden));
    assert_eq!(bit_total(rec, &table()), bits, "{rec:?}");
    canonical(rec, &ReadCtx::new(widths(), true));
}

/// A view with these float fields (each at index 0).
fn floats(pairs: &[(&str, f32)]) -> MapView {
    pairs
        .iter()
        .fold(MapView::default(), |v, (n, x)| v.with(n, 0, Value::F32(*x)))
}

/// Adds `n` datablock ids `first..` to a view's array field.
fn with_ids(v: MapView, name: &str, n: usize, first: u32) -> MapView {
    (0..n).fold(v, |v, i| v.with(name, i, Value::Id(Some(first + i as u32))))
}

/// Adds the strings to a view's array field.
fn with_strs(v: MapView, name: &str, strs: &[&str]) -> MapView {
    strs.iter()
        .enumerate()
        .fold(v, |v, (i, s)| v.with(name, i, Value::str(s)))
}

#[test]
fn an_empty_string_is_ten_bits() {
    let t = table();
    let mut w = writer();
    torque_net::strcoder::write_string(&mut w, &t, b"", 255);
    torque_net::strcoder::write_string(&mut w, &t, b"", 255);
    assert_eq!(w.bits(), 2 * EMPTY);
}
