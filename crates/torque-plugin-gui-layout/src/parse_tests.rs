use taffy::prelude::*;
use taffy::style::GridTemplateRepetition;

use crate::parse::*;

#[test]
fn numbers_lengths_and_percentages() {
    assert_eq!(number(" 12 "), Some(12.0));
    assert_eq!(number("1e9"), Some(MAX_LENGTH), "clamped");
    assert_eq!(number("nan"), None);
    assert_eq!(number("wide"), None);
    assert_eq!(length_percentage("8"), Some(LengthPercentage::length(8.0)));
    assert_eq!(
        length_percentage("50%"),
        Some(LengthPercentage::percent(0.5))
    );
    assert_eq!(
        length_percentage_auto("auto"),
        Some(LengthPercentageAuto::auto())
    );
    assert_eq!(dimension("AUTO"), Some(Dimension::auto()));
    assert_eq!(dimension("25%"), Some(Dimension::percent(0.25)));
}

#[test]
fn sides_follow_the_css_shorthand() {
    let read = |t| sides(t, length_percentage, LengthPercentage::ZERO);
    let l = LengthPercentage::length;
    let rect = |top, right, bottom, left| Rect {
        left: l(left),
        right: l(right),
        top: l(top),
        bottom: l(bottom),
    };
    assert_eq!(read("8"), rect(8.0, 8.0, 8.0, 8.0));
    assert_eq!(read("8 16"), rect(8.0, 16.0, 8.0, 16.0));
    assert_eq!(read("1 2 3"), rect(1.0, 2.0, 3.0, 2.0));
    assert_eq!(read("1 2 3 4"), rect(1.0, 2.0, 3.0, 4.0));
    assert_eq!(read(""), rect(0.0, 0.0, 0.0, 0.0));
    assert_eq!(
        read("x 4"),
        rect(0.0, 4.0, 0.0, 4.0),
        "an unreadable side is 0"
    );
}

#[test]
fn gap_is_rows_then_columns() {
    let g = gap("8 16");
    assert_eq!(g.height, LengthPercentage::length(8.0));
    assert_eq!(g.width, LengthPercentage::length(16.0));
    let g = gap("6");
    assert_eq!(
        (g.width, g.height),
        (LengthPercentage::length(6.0), LengthPercentage::length(6.0))
    );
}

#[test]
fn keywords() {
    assert_eq!(direction("column"), FlexDirection::Column);
    assert_eq!(direction("sideways"), FlexDirection::Row);
    assert_eq!(wrap("wrap"), FlexWrap::Wrap);
    assert_eq!(wrap("1"), FlexWrap::Wrap);
    assert_eq!(wrap(""), FlexWrap::NoWrap);
    assert_eq!(align_items("Center"), Some(AlignItems::CENTER));
    assert_eq!(align_items("auto"), None);
    assert_eq!(
        align_content("space-between"),
        Some(AlignContent::SPACE_BETWEEN)
    );
    assert_eq!(auto_flow("column dense"), GridAutoFlow::ColumnDense);
    assert_eq!(auto_flow(""), GridAutoFlow::Row);
}

#[test]
fn templates_read_tracks_minmax_and_repeat() {
    let t = template(
        "100 1fr auto 25% minmax(80, 2fr) fit-content(120) min-content",
        None,
        0.0,
    );
    let single = |f: TrackSizingFunction| GridTemplateComponent::Single(f);
    assert_eq!(
        t,
        vec![
            single(length(100.0)),
            single(fr(1.0)),
            single(auto()),
            single(percent(0.25)),
            single(minmax(
                MinTrackSizingFunction::length(80.0),
                MaxTrackSizingFunction::fr(2.0)
            )),
            single(fit_content(LengthPercentage::length(120.0))),
            single(min_content()),
        ]
    );
    // 500 wide, gaps of 10: 80, then 3 x (1fr + 20) holds 80 + 60 and 7 gaps (210), so (500 - 210 - 96) / 106 more 96s fit.
    let t = template(
        "80 repeat(3, 1fr 20) repeat(auto-fill, 96)",
        Some(500.0),
        10.0,
    );
    assert_eq!(t.len(), 3);
    assert_eq!(
        t[1],
        GridTemplateComponent::Repeat(GridTemplateRepetition {
            count: RepetitionCount::Count(3),
            tracks: vec![fr(1.0), length(20.0)],
            line_names: Vec::new(),
        })
    );
    assert!(matches!(
        &t[2],
        GridTemplateComponent::Repeat(r) if r.count == RepetitionCount::Count(2)
    ));
    assert!(
        template("bogus 3em", None, 0.0).is_empty(),
        "unknown words are left out"
    );
}

#[test]
fn templates_are_capped() {
    let many = "1fr ".repeat(1000);
    assert_eq!(template(&many, None, 0.0).len(), MAX_TRACKS);
    let t = template("repeat(60000, 1fr 1fr)", None, 0.0);
    let GridTemplateComponent::Repeat(r) = &t[0] else {
        panic!("{t:?}");
    };
    assert_eq!(r.count, RepetitionCount::Count((MAX_TRACKS / 2) as u16));
    assert!(template("repeat(0, 1fr)", None, 0.0).is_empty());
}

/// Every repeat counted out (an automatic one too) and its inner tracks cut to the room left: never more than
/// [`MAX_TRACKS`] tracks in all.
fn track_total(t: &[GridTemplateComponent<String>]) -> usize {
    t.iter()
        .map(|c| match c {
            GridTemplateComponent::Single(_) => 1,
            GridTemplateComponent::Repeat(r) => match r.count {
                RepetitionCount::Count(n) => usize::from(n) * r.tracks.len(),
                other => panic!("an automatic count is left to taffy: {other:?}"),
            },
        })
        .sum()
}

#[test]
fn hostile_templates_stay_within_the_track_cap() {
    let wide = format!("repeat(auto-fill, {})", "1 ".repeat(400));
    let long_inner = format!("10 10 repeat(5, {})", "2 ".repeat(400));
    for text in [
        "repeat(auto-fill, 1)",
        "repeat(auto-fit, 1 1 1)",
        "repeat(auto-fill, minmax(1, 1fr))",
        "200 repeat(auto-fill, 1) repeat(auto-fit, 1)",
        "repeat(255, 1) repeat(auto-fill, 1)",
        wide.as_str(),
        long_inner.as_str(),
        "repeat(auto-fill, )",
        "repeat(3, nothing)",
    ] {
        for room in [
            None,
            Some(0.0),
            Some(1e30),
            Some(MAX_LENGTH),
            Some(f32::NAN),
        ] {
            for gap in [0.0, -5.0, 1e9] {
                let t = template(text, room, gap);
                assert!(
                    track_total(&t) <= MAX_TRACKS,
                    "{text} {room:?} {gap}: {}",
                    track_total(&t)
                );
            }
        }
    }
    // A repeat's inner tracks are cut to the room left after the tracks before it.
    let t = template(&long_inner, None, 0.0);
    assert_eq!(track_total(&t), MAX_TRACKS);
    // One pixel tracks in the largest room fill the cap and no more.
    assert_eq!(
        track_total(&template("repeat(auto-fill, 1)", Some(MAX_LENGTH), 0.0)),
        MAX_TRACKS
    );
    assert!(
        template("repeat(auto-fill, )", Some(500.0), 0.0).is_empty(),
        "an empty repeat is left out"
    );
}

#[test]
fn long_texts_are_read_only_to_the_cap() {
    let long = "1fr ".repeat(100_000);
    assert!(template(&long, None, 0.0).len() <= MAX_TEXT_BYTES / 4 + 1);
    assert_eq!(number(&"1".repeat(10_000)), None);
    assert_eq!(
        gap(&"8 ".repeat(100_000)).width,
        LengthPercentage::length(8.0)
    );
}

#[test]
fn placements_are_a_line_and_a_span() {
    let p = placement("2", "3");
    assert_eq!(p.start, line::<GridPlacement<String>>(2));
    assert_eq!(p.end, GridPlacement::Span(3));
    let p = placement("", "");
    assert_eq!(
        (p.start, p.end),
        (GridPlacement::Auto, GridPlacement::Span(1))
    );
    assert_eq!(
        placement("0", "0").start,
        GridPlacement::Auto,
        "line 0 is none"
    );
    assert_eq!(
        placement("-1", "1").start,
        line::<GridPlacement<String>>(-1)
    );
}

#[test]
fn an_automatic_repeat_needs_tracks_of_a_fixed_size() {
    let count = |text: &str| match &template(text, Some(500.0), 0.0)[0] {
        GridTemplateComponent::Repeat(r) => r.count,
        other => panic!("{other:?}"),
    };
    // Counted in the room: five 96s fit in 500, six 80s.
    assert_eq!(count("repeat(auto-fill, 96)"), RepetitionCount::Count(5));
    assert_eq!(
        count("repeat(auto-fit, minmax(80, 1fr))"),
        RepetitionCount::Count(6)
    );
    assert_eq!(count("repeat(auto-fill, 0)"), RepetitionCount::Count(1));
    assert_eq!(count("repeat(auto-fill, 1fr)"), RepetitionCount::Count(1));
    assert_eq!(
        count("repeat(auto-fill, minmax(0, 1fr))"),
        RepetitionCount::Count(1)
    );
}

#[test]
fn sizes_gaps_and_padding_are_never_negative() {
    assert_eq!(dimension("-20"), Some(Dimension::length(0.0)));
    assert_eq!(length_percentage("-5"), Some(LengthPercentage::length(0.0)));
    assert_eq!(
        length_percentage_auto("-5"),
        Some(LengthPercentageAuto::length(-5.0)),
        "a margin may be"
    );
}
