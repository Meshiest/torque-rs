use super::*;

#[test]
fn names_are_found_by_case_and_mask() {
    assert_eq!(
        lookup(b"al_gain_linear", MASK_LISTENER_F),
        Some(AL_GAIN_LINEAR)
    );
    assert_eq!(lookup(b"AL_PITCH", MASK_LISTENER_F), None);
    assert_eq!(lookup(b"AL_PITCH", MASK_SOURCE_F), Some(AL_PITCH));
    assert_eq!(lookup(b"AL_VENDOR", MASK_GET_STRING), Some(AL_VENDOR));
    assert_eq!(lookup(b"AL_GAIN", MASK_GET_STRING), None);
    assert_eq!(lookup(b"AL_LOOPING", MASK_SOURCE_I), Some(AL_LOOPING));
    assert_eq!(lookup(b"", MASK_SOURCE_I), None);
}
