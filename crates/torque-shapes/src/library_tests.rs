use super::*;

#[test]
fn the_plain_table_is_used_until_one_is_installed() {
    assert!(std::ptr::eq(get(), &PLAIN));
}

#[test]
fn the_plain_thread_sort_is_stable() {
    let mut v = vec![3, 1, 4, 1, 5, 9, 2, 6];
    // By the value's parity only: the evens keep their order, then the odds keep theirs.
    let cmp = |a: &usize, b: &usize| (a % 2) as i32 - (b % 2) as i32;
    (get().sort_threads)(&mut v, &cmp);
    assert_eq!(v, vec![4, 2, 6, 3, 1, 1, 5, 9]);
}

#[test]
fn the_plain_scalar_routines_follow_the_source_order() {
    assert_eq!((get().lerp)(1.0, 3.0, 0.25), 1.5);
    assert_eq!((get().mul_add)(2.0, 3.0, 1.0), 7.0);
    assert_eq!((get().mul_div)(2.0, 3.0, 4.0), 1.5);
    assert!((get().same_side)(-1.0, -2.0));
    assert!(!(get().same_side)(0.0, -2.0));
    assert!((get().ray_time_less)(1.0, 1.0, 2.0, 1.0, 1.0));
    assert!(!(get().ray_time_less)(1.0, 1.0, 2.0, 1.0, -1.0));
    assert_eq!((get().to_int)(-2.9), -2);
    assert_eq!((get().to_int)(f32::NAN), 0);
    assert!((get().in_front)(&[0.0, 0.0, 1.0, -1.0], [0.0, 0.0, 2.0]));
}
