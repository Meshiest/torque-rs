//! Packs an object update into a bit stream the way a ghost update is written (flags, ranged integers, quantised floats,
//! a normal, a compressed point and a string through the string coder), then reads it back.
//!
//! `cargo run -p torque --example net`

use torque::net::bits::{BitReader, BitWriter};
use torque::net::huffman::Table;
use torque::net::strcoder::{read_string, write_string};

fn main() {
    let strings = Table::torque();
    let position = [12.5, -3.25, 40.0];
    let facing = [0.0, 0.6, 0.8];

    let mut w = BitWriter::new();
    w.write_flag(true); // the position changed
    w.write_compressed_point(position, 0.01);
    w.write_normal_vector(facing, 8);
    w.write_ranged_u32(73, 0, 100); // health, 0..=100 in 7 bits
    w.write_float(0.25, 6); // energy, 0..=1 in 6 bits
    w.write_signed_int(-5, 8);
    write_string(&mut w, &strings, b"a networked name", 255);
    let bits = w.bits();
    let bytes = w.into_bytes();
    println!("wrote {bits} bits in {} bytes: {bytes:02x?}", bytes.len());

    let mut r = BitReader::new(&bytes);
    let moved = r.read_flag().expect("flag");
    let read_position = r.read_compressed_point(0.01).expect("point");
    let read_facing = r.read_normal_vector(8).expect("normal");
    let health = r.read_ranged_u32(0, 100).expect("health");
    let energy = r.read_float(6).expect("energy");
    let delta = r.read_signed_int(8).expect("delta");
    let name = read_string(&mut r, &strings).expect("name");
    println!("moved {moved}, position {read_position:?}, facing {read_facing:.3?}");
    println!("health {health}, energy {energy:.3}, delta {delta}");
    println!("name {:?}", String::from_utf8_lossy(&name));

    // A reader past the end returns an error instead of zeros or a panic.
    assert!(r.read_u32().is_err());
}
