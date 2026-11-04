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
