use super::*;

#[test]
fn stage_names_go_both_ways_and_follow_the_frame() {
    for stage in Stage::ALL {
        assert_eq!(Stage::of_name(stage.name()), Some(stage));
    }
    assert_eq!(Stage::of_name("opaque"), Some(Stage::Opaque));
    assert_eq!(Stage::of_name("nope"), None);
    let mut sorted = Stage::ALL;
    sorted.sort();
    assert_eq!(sorted, Stage::ALL);
}

#[test]
fn passes_are_kept_by_stage_in_the_order_added() {
    let mut passes = HdPasses::default();
    assert!(passes.is_empty());
    passes.add(Stage::Opaque, Box::new(|_: &mut PassCx<'_>| {}));
    passes.add(Stage::Opaque, Box::new(|_: &mut PassCx<'_>| {}));
    passes.add(Stage::Overlay, Box::new(|_: &mut PassCx<'_>| {}));
    assert_eq!(passes.count(Stage::Opaque), 2);
    assert!(passes.has(Stage::Overlay));
    assert!(!passes.has(Stage::Shadows));
    passes.clear();
    assert!(passes.is_empty());
}
