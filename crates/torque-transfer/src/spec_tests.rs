use super::*;

#[test]
fn the_default_spec_is_valid() {
    assert_eq!(UploadSpec::default().validate(), Ok(()));
}

#[test]
fn each_out_of_range_limit_is_named() {
    let bad = |edit: fn(&mut UploadSpec)| {
        let mut spec = UploadSpec::default();
        edit(&mut spec);
        spec.validate().unwrap_err().0
    };
    assert!(bad(|s| s.max_bytes = 0).contains("max_bytes"));
    assert!(bad(|s| s.max_bytes = MAX_UPLOAD_BYTES + 1).contains("max_bytes"));
    assert!(bad(|s| s.quota.files = 0).contains("quota"));
    assert!(bad(|s| s.rate = 0).contains("rate"));
    assert!(bad(|s| s.concurrent = 0).contains("concurrent"));
    assert!(bad(|s| s.concurrent = 17).contains("concurrent"));
    assert!(bad(|s| s.server_in_flight = 1).contains("server_in_flight"));
    assert!(bad(|s| s.idle = Duration::ZERO).contains("idle"));
    assert!(bad(|s| s.meta_max = MAX_META_BYTES + 1).contains("meta_max"));
}

#[test]
fn durations_read_in_the_largest_whole_unit() {
    assert_eq!(duration_text(Duration::from_secs(30)), "30 seconds");
    assert_eq!(duration_text(Duration::from_secs(300)), "5 minutes");
    assert_eq!(duration_text(Duration::from_secs(86_400)), "24 hours");
}
