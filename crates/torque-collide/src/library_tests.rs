use super::*;
use crate::math::{IDENTITY, mul_p};

#[test]
fn the_plain_table_is_used_until_one_is_installed() {
    // Nothing in this crate's tests installs a table.
    assert!(std::ptr::eq(get(), &PLAIN));
    let mut m = IDENTITY;
    m[3] = 1.0;
    assert_eq!(mul_p(&m, [1.0, 2.0, 3.0]), [2.0, 2.0, 3.0]);
}
