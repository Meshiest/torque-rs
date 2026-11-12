use super::*;

#[test]
fn a_mode_is_the_engines_settings_at_its_quality_and_differs_by_either() {
    let high = HdMode::default();
    assert_eq!(high, HdMode::new(Quality::High));
    assert_eq!(high.image, ImageSettings::DEFAULT);
    assert_ne!(high, HdMode::new(Quality::Ultra));
    let other = HdMode {
        image: ImageSettings {
            sharpen: 0.5,
            ..ImageSettings::DEFAULT
        },
        quality: Quality::High,
    };
    assert_ne!(high, other);
}

#[test]
fn the_ao_is_at_half_resolution_at_every_quality_unless_the_settings_ask_for_full() {
    assert_eq!(ImageSettings::DEFAULT.ao_scale(), 2);
    let full = ImageSettings {
        ao_full_res: true,
        ..ImageSettings::DEFAULT
    };
    assert_eq!(full.ao_scale(), 1);
    assert_ne!(HdMode::new(Quality::Ultra).image, full);
}
