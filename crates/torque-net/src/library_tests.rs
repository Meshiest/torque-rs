use super::*;

#[test]
fn the_plain_table_is_used_until_one_is_installed() {
    // Nothing in this crate's tests installs a table.
    assert!(std::ptr::eq(get(), &PLAIN));
}

#[test]
fn a_float_converts_to_an_integer_as_rust_casts_do() {
    assert_eq!(plain::to_int(1.9), 1);
    assert_eq!(plain::to_int(-1.9), -1);
    assert_eq!(plain::to_int(f32::NAN), 0);
    assert_eq!(plain::to_int(f32::INFINITY), i32::MAX);
    assert_eq!(plain::to_int(f32::NEG_INFINITY), i32::MIN);
    assert_eq!(plain::to_int(3.0e9), i32::MAX);
}

#[test]
fn the_unit_float_product_is_an_f32_product() {
    // 0.29f32 * 100 rounds to 29.0 in f32 and converts to 29.
    assert_eq!(plain::to_int(0.29f32 * 100.0), 29);
    assert_eq!(plain::write_float(0.5, 8), 127);
    assert_eq!(plain::write_float(1.0, 6), 63);
    assert_eq!(plain::write_signed_float(0.0, 8), 127);
    assert_eq!(plain::write_signed_float(1.0, 8), 255);
    assert_eq!(plain::read_signed_float(255, 8), 1.0);
    assert_eq!(plain::read_signed_float(0, 8), -1.0);
}

#[test]
fn the_normal_angles_use_the_double_pi() {
    // (0, -1, 0): atan2(0, -1) is pi, stored as an f32 just above pi; over the double pi it rounds back to 1.0.
    assert_eq!(plain::normal_angles([0.0, -1.0, 0.0]), (1.0, 0.0));
    assert_eq!(plain::normal_angles([0.0, 0.0, 1.0]), (0.0, 1.0));
    let v = plain::normal_vector(0.0, 1.0);
    assert!(v[0].abs() < 1e-6 && (v[2] - 1.0).abs() < 1e-6, "{v:?}");
}

#[test]
fn a_compressed_point_is_computed_in_f32() {
    let (r, axes) = plain::compress_point([1.5, -2.25, 3.0], [0.0; 3], 0.5);
    assert_eq!(axes, [3, -4, 6]);
    assert!((r - 2.0 * (1.5f32 * 1.5 + 2.25 * 2.25 + 9.0).sqrt()).abs() < 1e-5);
    assert_eq!(plain::decompress_axis(3, 0.5, 100.0), 101.5);
}

#[test]
fn a_move_angle_is_two_pi_over_65536_per_unit_signed() {
    assert_eq!(plain::move_angle(0), 0.0);
    assert_eq!(
        plain::move_angle(0x4000),
        (std::f64::consts::TAU / 4.0) as f32
    );
    assert_eq!(plain::move_angle(0x8000), -std::f32::consts::PI);
}

#[test]
fn colours_cones_and_box_centres() {
    assert_eq!(plain::color_byte(0.5), 128);
    assert_eq!(plain::color_byte(1.0), 255);
    assert_eq!(plain::color_byte(0.2), 51);
    assert_eq!(plain::cone_w(0.0, 0.0, 0.0), 1.0);
    assert_eq!(plain::cone_w(1.0, 1.0, 0.0), 0.0);
    assert_eq!(
        plain::box_centre([-1.0, 0.0, 2.0], [3.0, 1.0, 2.0]),
        [1.0, 0.5, 2.0]
    );
}
