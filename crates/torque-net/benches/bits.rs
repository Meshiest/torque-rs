//! The bit layer: field writes, byte blobs, the mark and rewind the ghost layer takes around every record, and growing a packet.

use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use torque_net::bits::{BitReader, BitWriter};

const FIELDS: u32 = 1_000;

fn writes(c: &mut Criterion) {
    let mut group = c.benchmark_group("bits");
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(1));

    group.bench_function("write_bits_13_x1000", |b| {
        b.iter(|| {
            let mut w = BitWriter::with_limit(1 << 20);
            for i in 0..FIELDS {
                w.write_bits(black_box(i), 13);
            }
            w
        });
    });
    group.bench_function("write_flag_x1000", |b| {
        b.iter(|| {
            let mut w = BitWriter::with_limit(1 << 20);
            for i in 0..FIELDS {
                w.write_flag(black_box(i & 1 == 0));
            }
            w
        });
    });
    for len in [1_000usize, 4_000] {
        let blob = vec![0xA5u8; len];
        group.bench_function(format!("write_bytes_{len}_unaligned"), |b| {
            b.iter(|| {
                let mut w = BitWriter::with_limit(1 << 20);
                w.write_flag(true);
                w.write_bytes(black_box(&blob));
                w
            });
        });
        let mut w = BitWriter::with_limit(1 << 20);
        w.write_flag(true);
        w.write_bytes(&blob);
        let bytes = w.into_bytes();
        group.bench_function(format!("read_bytes_{len}_unaligned"), |b| {
            b.iter(|| {
                let mut r = BitReader::new(black_box(&bytes));
                r.read_flag().expect("flag");
                r.read_bytes(len).expect("bytes")
            });
        });
    }
    group.bench_function("mark_rewind_string_buffer", |b| {
        let mut w = BitWriter::with_limit(1 << 20);
        w.enable_string_buffer();
        b.iter(|| {
            let mark = w.mark();
            w.rewind(black_box(mark));
        });
    });
    group.bench_function("mark_rewind_no_string_buffer", |b| {
        let mut w = BitWriter::with_limit(1 << 20);
        b.iter(|| {
            let mark = w.mark();
            w.rewind(black_box(mark));
        });
    });
    group.bench_function("clone_7kb_writer", |b| {
        let mut w = BitWriter::with_limit(1 << 20);
        for _ in 0..8_000 {
            w.write_bits(5, 7);
        }
        b.iter(|| black_box(&w).clone());
    });
    group.bench_function("grow_1000_byte_packet_by_20_bit_writes", |b| {
        b.iter(|| {
            let mut w = BitWriter::with_limit(8_000);
            for i in 0..400u32 {
                w.write_bits(black_box(i), 20);
            }
            w.into_bytes()
        });
    });
    group.finish();
}

criterion_group!(benches, writes);
criterion_main!(benches);
