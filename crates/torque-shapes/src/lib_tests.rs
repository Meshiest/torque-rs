use super::*;

/// Every vector a reader sizes from a file count goes through `reserved` (the allocation bound): no reader source, above its
/// test module, sizes a vector itself.
#[test]
fn no_reader_sizes_a_vector_from_a_file_count_by_itself() {
    for (name, source) in [
        ("dts.rs", include_str!("dts.rs")),
        ("dsq.rs", include_str!("dsq.rs")),
        ("matlist.rs", include_str!("matlist.rs")),
        ("write.rs", include_str!("write.rs")),
        ("bounds.rs", include_str!("bounds.rs")),
    ] {
        let code = source.split("#[cfg(test)]").next().unwrap_or(source);
        for banned in ["with_capacity", ".resize(", "repeat_n(", "vec![0"] {
            assert!(
                !code.contains(banned),
                "{name}: `{banned}` sizes a vector by hand; use crate::reserved"
            );
        }
    }
}

#[test]
fn the_helper_never_asks_for_more_than_the_bytes_or_the_cap() {
    // A count the bytes cannot back is cut to what they could hold.
    assert!(reserved::<u8>(usize::MAX, 10, 1).capacity() <= 10);
    assert!(reserved::<[f32; 3]>(1 << 40, 120, 12).capacity() <= 10);
    assert!(reserved::<u32>(5, 0, 4).capacity() == 0);
    // Whatever the bytes could hold, the request stays under the cap.
    let big = reserved::<[f32; 16]>(usize::MAX, usize::MAX, 1);
    assert!(big.capacity() * std::mem::size_of::<[f32; 16]>() <= MAX_ALLOC_BYTES);
    assert!(largest_request() <= MAX_ALLOC_BYTES);
    assert!(largest_request() >= 10);
}
