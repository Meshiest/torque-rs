use super::*;

static PLAIN: u32 = 1;
static OTHER: u32 = 2;
static THIRD: u32 = 3;

#[test]
fn the_plain_table_is_in_use_until_one_is_installed_and_the_first_install_stays() {
    let seam = Seam::new(&PLAIN);
    assert!(std::ptr::eq(seam.get(), &PLAIN));
    assert!(seam.install(&OTHER));
    assert!(seam.install(&OTHER));
    assert!(!seam.install(&THIRD));
    assert!(std::ptr::eq(seam.get(), &OTHER));
}
