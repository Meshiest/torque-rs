use super::*;

#[test]
fn a_bitmaps_size_comes_from_its_header_once_per_file_and_the_cache_keeps_taking_names() {
    // The size decoded the whole image, and past 4,096 names every reflow decoded each new one again.
    // The file's header says 640 x 480; its body is not an image at all (a decode would fail).
    let mut file = vec![137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13];
    file.extend_from_slice(b"IHDR");
    file.extend_from_slice(&640u32.to_be_bytes());
    file.extend_from_slice(&480u32.to_be_bytes());
    file.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
    file.extend_from_slice(&[0xab; 64]);
    let read = |p: &str| (p.eq_ignore_ascii_case("ui/pic.png")).then(|| file.clone());
    let mut sizes = BitmapSizes::default();
    for name in ["ui/pic", "UI/Pic", "ui/PIC.png", "Ui/pIc"] {
        assert_eq!(sizes.size(name, &read), Some((640, 480)), "{name}");
    }
    assert_eq!(sizes.reads, 1, "one header read for every spelling");
    // Past the cap of names the cache keeps answering (the least recently used names go).
    for i in 0..(crate::textures::MAX_TEXTURES + 100) {
        assert_eq!(sizes.size(&format!("missing/{i}"), &read), None);
    }
    assert_eq!(sizes.size("UI/PIC", &read), Some((640, 480)));
    assert_eq!(sizes.reads, 1);
}
