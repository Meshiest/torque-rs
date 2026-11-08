//! The field texts of the containers and their children, read as taffy's style values. Every reader is total: a word it does
//! not know is the field's default, as a TorqueScript field that cannot be read is.

use taffy::prelude::*;
use taffy::style::GridTemplateRepetition;

/// The largest length a field may give, in pixels either way: more is clamped (a hostile script cannot hand taffy huge sizes).
pub const MAX_LENGTH: f32 = 100_000.0;

/// The longest field text read (a longer one is read to there).
pub const MAX_TEXT_BYTES: usize = 1024;

/// The longest number read; a longer word is not a number.
const MAX_WORD_BYTES: usize = 64;

/// `text` up to [`MAX_TEXT_BYTES`], cut at a character boundary.
fn capped(text: &str) -> &str {
    let mut end = text.len().min(MAX_TEXT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The most tracks a template gives, `repeat` counts included: more are left out.
pub const MAX_TRACKS: usize = 256;

/// A number in a field (`12`, `-3`, `0.5`), clamped to [`MAX_LENGTH`]; None for anything else.
pub fn number(word: &str) -> Option<f32> {
    let word = word.trim();
    if word.len() > MAX_WORD_BYTES {
        return None;
    }
    let value: f32 = word.parse().ok()?;
    value
        .is_finite()
        .then(|| value.clamp(-MAX_LENGTH, MAX_LENGTH))
}

/// `12` (pixels) or `50%`.
fn length_or_percent(word: &str) -> Option<(f32, bool)> {
    let word = word.trim();
    match word.strip_suffix('%') {
        Some(p) => number(p).map(|v| (v / 100.0, true)),
        None => number(word).map(|v| (v, false)),
    }
}

/// `12` or `50%` (a gap or padding: never less than 0).
pub fn length_percentage(word: &str) -> Option<LengthPercentage> {
    length_or_percent(word)
        .map(|(v, pct)| (v.max(0.0), pct))
        .map(|(v, pct)| {
            if pct {
                LengthPercentage::percent(v)
            } else {
                LengthPercentage::length(v)
            }
        })
}

/// `12`, `50%` or `auto`.
pub fn length_percentage_auto(word: &str) -> Option<LengthPercentageAuto> {
    if word.trim().eq_ignore_ascii_case("auto") {
        return Some(LengthPercentageAuto::auto());
    }
    length_or_percent(word).map(|(v, pct)| {
        if pct {
            LengthPercentageAuto::percent(v)
        } else {
            LengthPercentageAuto::length(v)
        }
    })
}

/// `12`, `50%` or `auto` (a size: never less than 0).
pub fn dimension(word: &str) -> Option<Dimension> {
    if word.trim().eq_ignore_ascii_case("auto") {
        return Some(Dimension::auto());
    }
    length_or_percent(word)
        .map(|(v, pct)| (v.max(0.0), pct))
        .map(|(v, pct)| {
            if pct {
                Dimension::percent(v)
            } else {
                Dimension::length(v)
            }
        })
}

/// The words of a field, split on spaces, tabs and commas.
fn words(text: &str) -> impl Iterator<Item = &str> {
    capped(text)
        .split([' ', '\t', ','])
        .filter(|w| !w.is_empty())
}

/// The four sides of a CSS shorthand (`8`; `8 16`: top and bottom, left and right; `8 16 4`; `8 16 4 2`: top, right, bottom,
/// left). A side that cannot be read is `fallback`.
pub fn sides<T: Copy>(text: &str, read: impl Fn(&str) -> Option<T>, fallback: T) -> Rect<T> {
    let values: Vec<T> = words(text)
        .take(4)
        .map(|w| read(w).unwrap_or(fallback))
        .collect();
    let (top, right, bottom, left) = match values.as_slice() {
        [] => (fallback, fallback, fallback, fallback),
        [a] => (*a, *a, *a, *a),
        [a, b] => (*a, *b, *a, *b),
        [a, b, c] => (*a, *b, *c, *b),
        [a, b, c, d, ..] => (*a, *b, *c, *d),
    };
    Rect {
        left,
        right,
        top,
        bottom,
    }
}

/// `gap`: one value for both, or the gap between rows then the gap between columns (CSS's order).
pub fn gap(text: &str) -> Size<LengthPercentage> {
    let mut it = words(text).map(|w| length_percentage(w).unwrap_or(LengthPercentage::ZERO));
    let rows = it.next().unwrap_or(LengthPercentage::ZERO);
    let columns = it.next().unwrap_or(rows);
    Size {
        width: columns,
        height: rows,
    }
}

/// A width and a height (`"120 auto"`); a missing or unreadable part is None.
pub fn pair<T>(text: &str, read: impl Fn(&str) -> Option<T>) -> Size<Option<T>> {
    let mut it = words(text);
    Size {
        width: it.next().and_then(&read),
        height: it.next().and_then(&read),
    }
}

/// `row` (the default), `column`, `row-reverse`, `column-reverse`.
pub fn direction(text: &str) -> FlexDirection {
    match text.trim().to_ascii_lowercase().as_str() {
        "column" => FlexDirection::Column,
        "row-reverse" => FlexDirection::RowReverse,
        "column-reverse" => FlexDirection::ColumnReverse,
        _ => FlexDirection::Row,
    }
}

/// `nowrap` (the default), `wrap`, `wrap-reverse`; a number or bool reads as on or off.
pub fn wrap(text: &str) -> FlexWrap {
    match text.trim().to_ascii_lowercase().as_str() {
        "wrap" | "1" | "true" => FlexWrap::Wrap,
        "wrap-reverse" => FlexWrap::WrapReverse,
        _ => FlexWrap::NoWrap,
    }
}

/// `start`, `end`, `center`, `stretch`, `baseline`, `flex-start`, `flex-end`; None for anything else (`auto`: the
/// container's).
pub fn align_items(text: &str) -> Option<AlignItems> {
    Some(match text.trim().to_ascii_lowercase().as_str() {
        "start" => AlignItems::START,
        "end" => AlignItems::END,
        "center" => AlignItems::CENTER,
        "stretch" => AlignItems::STRETCH,
        "baseline" => AlignItems::BASELINE,
        "flex-start" => AlignItems::FLEX_START,
        "flex-end" => AlignItems::FLEX_END,
        _ => return None,
    })
}

/// [`align_items`]'s words and `space-between`, `space-around`, `space-evenly`.
pub fn align_content(text: &str) -> Option<AlignContent> {
    Some(match text.trim().to_ascii_lowercase().as_str() {
        "start" => AlignContent::START,
        "end" => AlignContent::END,
        "center" => AlignContent::CENTER,
        "stretch" => AlignContent::STRETCH,
        "flex-start" => AlignContent::FLEX_START,
        "flex-end" => AlignContent::FLEX_END,
        "space-between" => AlignContent::SPACE_BETWEEN,
        "space-around" => AlignContent::SPACE_AROUND,
        "space-evenly" => AlignContent::SPACE_EVENLY,
        _ => return None,
    })
}

/// `row` (the default), `column`, `row dense`, `column dense`.
pub fn auto_flow(text: &str) -> GridAutoFlow {
    let text = text.trim().to_ascii_lowercase();
    let dense = text.contains("dense");
    match (text.starts_with("column"), dense) {
        (true, true) => GridAutoFlow::ColumnDense,
        (true, false) => GridAutoFlow::Column,
        (false, true) => GridAutoFlow::RowDense,
        (false, false) => GridAutoFlow::Row,
    }
}

/// The parts of a track list at the outer level: words, and `name(...)` groups kept whole.
fn track_words(text: &str) -> Vec<&str> {
    let text = capped(text);
    let mut out = Vec::new();
    let (mut depth, mut start) = (0usize, None);
    for (i, c) in text.char_indices() {
        match c {
            '(' => {
                depth += 1;
                start.get_or_insert(i);
            }
            ')' => depth = depth.saturating_sub(1),
            ' ' | '\t' | ',' if depth == 0 => {
                if let Some(s) = start.take() {
                    out.push(&text[s..i]);
                }
            }
            _ => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        out.push(&text[s..]);
    }
    out
}

/// `name(inner)`: the name and the inner text.
fn call(word: &str) -> Option<(&str, &str)> {
    let open = word.find('(')?;
    let inner = word[open + 1..].strip_suffix(')')?;
    Some((word[..open].trim(), inner))
}

/// The low end of a `minmax`: a length, a percentage, `auto`, `min-content`, `max-content`.
fn min_track(word: &str) -> Option<MinTrackSizingFunction> {
    Some(match word.trim().to_ascii_lowercase().as_str() {
        "auto" => MinTrackSizingFunction::AUTO,
        "min-content" => MinTrackSizingFunction::MIN_CONTENT,
        "max-content" => MinTrackSizingFunction::MAX_CONTENT,
        w => {
            let (v, pct) = length_or_percent(w)?;
            if pct {
                MinTrackSizingFunction::percent(v)
            } else {
                MinTrackSizingFunction::length(v)
            }
        }
    })
}

/// The high end of a `minmax`: [`min_track`]'s words and `1fr`.
fn max_track(word: &str) -> Option<MaxTrackSizingFunction> {
    let word = word.trim().to_ascii_lowercase();
    if let Some(f) = word.strip_suffix("fr") {
        return number(f).map(|v| MaxTrackSizingFunction::fr(v.max(0.0)));
    }
    Some(match word.as_str() {
        "auto" => MaxTrackSizingFunction::AUTO,
        "min-content" => MaxTrackSizingFunction::MIN_CONTENT,
        "max-content" => MaxTrackSizingFunction::MAX_CONTENT,
        w => {
            let (v, pct) = length_or_percent(w)?;
            if pct {
                MaxTrackSizingFunction::percent(v)
            } else {
                MaxTrackSizingFunction::length(v)
            }
        }
    })
}

/// One track: `120`, `25%`, `1fr`, `auto`, `min-content`, `max-content`, `minmax(a, b)`, `fit-content(120)`.
fn track(word: &str) -> Option<TrackSizingFunction> {
    if let Some((name, inner)) = call(word) {
        let parts: Vec<&str> = words(inner).collect();
        return match (name.to_ascii_lowercase().as_str(), parts.as_slice()) {
            ("minmax", [low, high]) => Some(minmax(min_track(low)?, max_track(high)?)),
            ("fit-content", [limit]) => Some(fit_content(length_percentage(limit)?)),
            _ => None,
        };
    }
    let lower = word.trim().to_ascii_lowercase();
    if let Some(f) = lower.strip_suffix("fr") {
        return number(f).map(|v| fr(v.max(0.0)));
    }
    Some(match lower.as_str() {
        "auto" => auto(),
        "min-content" => min_content(),
        "max-content" => max_content(),
        w => {
            let (v, pct) = length_or_percent(w)?;
            if pct { percent(v) } else { length(v) }
        }
    })
}

/// The pixels a track is sure to take: a length, or the larger length of a `minmax`; 0 for anything else.
fn fixed_pixels(word: &str) -> f32 {
    if let Some((name, inner)) = call(word) {
        return if name.eq_ignore_ascii_case("minmax") {
            words(inner).filter_map(number).fold(0.0, f32::max)
        } else {
            0.0
        };
    }
    number(word).unwrap_or(0.0).max(0.0)
}

/// `grid-auto-rows`/`-columns`: plain tracks (no `repeat`). Empty when none can be read (taffy's `auto`).
pub fn auto_tracks(text: &str) -> Vec<TrackSizingFunction> {
    track_words(text)
        .into_iter()
        .filter_map(track)
        .take(MAX_TRACKS)
        .collect()
}

/// A template (`columns`, `rows`): tracks and `repeat(count, tracks...)`, the count a number, `auto-fill` or `auto-fit`. A
/// track that cannot be read is left out, and the template has at most [`MAX_TRACKS`] tracks with every repeat counted out.
///
/// `room` is the length the tracks are laid in (the container's size inside its padding on that axis, when known) and `gap`
/// the gap between them in pixels: an automatic repeat becomes as many repetitions as fit there (`auto-fit` as `auto-fill`:
/// empty tracks are kept), one when the room is not known or its tracks have no fixed size.
pub fn template(text: &str, room: Option<f32>, gap: f32) -> Vec<GridTemplateComponent<String>> {
    let words = track_words(text);
    // What the other tracks take, for an automatic repeat's count.
    let (mut others_fixed, mut others_count) = (0.0f32, 0usize);
    for word in &words {
        match repeat_parts(word) {
            Some((times, inner)) => {
                if let Ok(n) = times.parse::<usize>() {
                    let n = n.min(MAX_TRACKS);
                    let tracks: Vec<(TrackSizingFunction, f32)> = fixed_tracks(inner);
                    others_fixed += n as f32 * tracks.iter().map(|t| t.1).sum::<f32>();
                    others_count = others_count.saturating_add(n.saturating_mul(tracks.len()));
                }
            }
            None if track(word).is_some() => {
                others_fixed += fixed_pixels(word);
                others_count += 1;
            }
            None => {}
        }
    }
    let gap = gap.clamp(0.0, MAX_LENGTH);
    let mut out = Vec::new();
    let mut count = 0usize;
    for word in words {
        if count >= MAX_TRACKS {
            break;
        }
        let Some((times, inner)) = repeat_parts(word) else {
            if let Some(t) = track(word) {
                out.push(GridTemplateComponent::Single(t));
                count += 1;
            }
            continue;
        };
        let mut tracks = fixed_tracks(inner);
        tracks.truncate(MAX_TRACKS - count);
        if tracks.is_empty() {
            continue;
        }
        let per_fixed: f32 = tracks.iter().map(|t| t.1).sum();
        let len = tracks.len();
        let times = times.to_ascii_lowercase();
        let wanted = if times == "auto-fill" || times == "auto-fit" {
            match room {
                Some(room) if per_fixed >= 1.0 => {
                    let room = room.clamp(0.0, MAX_LENGTH);
                    let first = others_fixed
                        + per_fixed
                        + (others_count + len).saturating_sub(1) as f32 * gap;
                    if first > room {
                        1
                    } else {
                        // Saturates for a huge quotient; the cap below bounds it.
                        1 + ((room - first) / (per_fixed + len as f32 * gap)).floor() as usize
                    }
                }
                _ => 1,
            }
        } else {
            let Ok(n) = times.parse::<usize>() else {
                continue;
            };
            n
        };
        let n = wanted.min((MAX_TRACKS - count) / len);
        if n == 0 {
            continue;
        }
        count += n * len;
        out.push(GridTemplateComponent::Repeat(GridTemplateRepetition {
            count: RepetitionCount::Count(u16::try_from(n).unwrap_or(u16::MAX)),
            tracks: tracks.into_iter().map(|t| t.0).collect(),
            line_names: Vec::new(),
        }));
    }
    out
}

/// `repeat(times, inner)`: its two parts.
fn repeat_parts(word: &str) -> Option<(&str, &str)> {
    let (name, inner) = call(word)?;
    if !name.eq_ignore_ascii_case("repeat") {
        return None;
    }
    let comma = inner.find(',')?;
    Some((inner[..comma].trim(), &inner[comma + 1..]))
}

/// The tracks of a repeat that can be read, each with the pixels it is sure to take, at most [`MAX_TRACKS`].
fn fixed_tracks(inner: &str) -> Vec<(TrackSizingFunction, f32)> {
    track_words(inner)
        .into_iter()
        .filter_map(|w| track(w).map(|t| (t, fixed_pixels(w))))
        .take(MAX_TRACKS)
        .collect()
}

/// The gaps between rows and between columns in pixels (a percentage counts as 0), for [`template`].
pub fn gap_pixels(text: &str) -> (f32, f32) {
    let mut it = words(text).map(|w| number(w).unwrap_or(0.0).max(0.0));
    let rows = it.next().unwrap_or(0.0);
    (rows, it.next().unwrap_or(rows))
}

/// The padding in pixels (a percentage counts as 0).
pub fn padding_pixels(text: &str) -> Rect<f32> {
    sides(text, |w| number(w).map(|v| v.max(0.0)), 0.0)
}

/// A child's place on one axis of a grid: `start` is the line it starts at (`1` is the first, `-1` the last; empty or `auto`:
/// placed by the auto-flow), `span` how many tracks it covers (at least 1).
pub fn placement(start: &str, span: &str) -> Line<GridPlacement<String>> {
    let span = number(span)
        .map(|s| s.clamp(1.0, MAX_TRACKS as f32) as u16)
        .unwrap_or(1);
    let start = number(start)
        .map(|l| l.clamp(-(MAX_TRACKS as f32), MAX_TRACKS as f32) as i16)
        .filter(|l| *l != 0);
    Line {
        start: start.map_or(GridPlacement::Auto, line),
        end: GridPlacement::Span(span),
    }
}
