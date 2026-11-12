use super::*;
use torque_testkit::Rng;

fn need(id: &str, byte: u8) -> RequiredMod {
    RequiredMod {
        id: id.to_string(),
        name: id.to_string(),
        version: "1".to_string(),
        hash: [byte; 32],
        bytes: 1_000,
        files: 3,
        download: false,
    }
}

#[test]
fn a_section_reads_back_as_it_was_written() {
    let s = ModsSection {
        max_bytes: 5 << 32 | 77,
        max_files: 1_234,
    };
    assert_eq!(ModsSection::decode(&s.encode()).unwrap(), s);
    let empty = ModsSection::default();
    assert_eq!(ModsSection::decode(&empty.encode()).unwrap(), empty);
}

#[test]
fn a_section_says_nothing_about_a_clients_mods() {
    // Whatever the client holds, the section is the same few bytes: a format, two caps.
    let bytes = ModsSection {
        max_bytes: 1,
        max_files: 2,
    }
    .encode();
    assert!(bytes.len() <= MAX_SECTION_BYTES);
    assert_eq!(bytes.len(), 13);
}

#[test]
fn a_section_refuses_what_a_server_would_not_trust() {
    let good = ModsSection::default().encode();
    let mut more = good.clone();
    more.extend_from_slice(&[0, 0]);
    assert!(ModsSection::decode(&more).is_err());
    let mut format = good.clone();
    format[0] = 9;
    assert!(ModsSection::decode(&format).is_err());
    assert!(ModsSection::decode(&good[..good.len() - 3]).is_err());
    assert!(ModsSection::decode(&[0; MAX_SECTION_BYTES + 1]).is_err());
}

#[test]
fn the_required_list_is_sorted_unique_and_capped() {
    let list = RequiredMods::default();
    assert!(list.is_empty());
    let mut mods: Vec<RequiredMod> = (0..(MAX_REQUIRED + 5))
        .map(|i| need(&format!("m{i:03}"), 0))
        .rev()
        .collect();
    mods.push(need("m000", 9));
    list.set(mods);
    let got = list.get();
    assert_eq!(got.len(), MAX_REQUIRED);
    assert!(got.windows(2).all(|w| w[0].id < w[1].id));
    let shared = list.clone();
    shared.set(Vec::new());
    assert!(list.is_empty(), "a clone shares the list");
}

#[test]
fn versions_are_plain_text() {
    assert!(is_version(""));
    assert!(is_version("1.2.3"));
    assert!(!is_version("1.\u{202e}2"));
    assert!(!is_version(&"9".repeat(MAX_VERSION_BYTES + 1)));
}

#[test]
fn display_names_are_short_printable_and_not_markup() {
    for good in ["Soccer", "Big Ball", "Fußball 2", "a"] {
        assert!(is_display_name(good), "{good}");
    }
    let long = "n".repeat(MAX_DISPLAY_NAME_BYTES + 1);
    for bad in [
        "",
        " ",
        " Soccer",
        "Soccer ",
        "So\ncer",
        "So\u{1}cer",
        "So\u{202e}cer",
        "So\u{200b}cer",
        "<b>Soccer</b>",
        "a>b",
        long.as_str(),
    ] {
        assert!(!is_display_name(bad), "{bad:?}");
    }
    assert!(is_display_name(&"n".repeat(MAX_DISPLAY_NAME_BYTES)));
}

#[test]
fn a_display_name_cannot_look_empty() {
    for filler in [
        '\u{3164}', '\u{115f}', '\u{1160}', '\u{ffa0}', '\u{2800}', '\u{17b4}', '\u{17b5}',
        '\u{34f}',
    ] {
        for name in [
            filler.to_string(),
            format!("{filler}{filler}"),
            format!("Soc{filler}cer"),
        ] {
            assert!(!is_display_name(&name), "{name:?}");
        }
    }
    // White space of other kinds at an end (no-break, ideographic) is not a name either.
    for blank in ["\u{a0}", "\u{3000}", "\u{2003}x"] {
        assert!(!is_display_name(blank), "{blank:?}");
    }
    // A mark with nothing to sit on.
    assert!(!is_display_name("\u{301}"));
    assert!(!is_display_name("\u{301}Soccer"));
}

#[test]
fn a_display_name_limits_its_combining_marks() {
    let marks = |n: usize, mark: char| format!("e{}", mark.to_string().repeat(n));
    assert!(is_display_name(&marks(MAX_COMBINING_RUN, '\u{301}')));
    assert!(!is_display_name(&marks(MAX_COMBINING_RUN + 1, '\u{301}')));
    // Spread out, up to the total and no further.
    let spread = |letters: usize| "e\u{301}\u{302}".repeat(letters);
    assert!(is_display_name(&spread(
        MAX_COMBINING_TOTAL / MAX_COMBINING_RUN
    )));
    assert!(!is_display_name(&spread(
        MAX_COMBINING_TOTAL / MAX_COMBINING_RUN + 1
    )));
    // Variation selectors count as marks too.
    assert!(!is_display_name(&marks(MAX_COMBINING_RUN + 1, '\u{fe0f}')));
    assert!(is_display_name("Café"));
}

#[test]
fn hostile_sections_never_panic() {
    let mut rng = Rng::new(0x9e37_79b9_7f4a_7c15);
    let mut next = || rng.next_u64();
    let good = ModsSection {
        max_bytes: 99,
        max_files: 5,
    }
    .encode();
    for case in 0..20_000 {
        let mut bytes = good.clone();
        match case % 4 {
            0 => {
                for _ in 0..1 + next() % 4 {
                    let at = (next() as usize) % bytes.len();
                    bytes[at] = next() as u8;
                }
            }
            1 => bytes.truncate((next() as usize) % (bytes.len() + 1)),
            2 => bytes = (0..(next() % 60) as usize).map(|_| next() as u8).collect(),
            _ => {
                let at = (next() as usize) % bytes.len();
                bytes.insert(at, next() as u8);
            }
        }
        let _ = ModsSection::decode(&bytes);
    }
}

#[test]
fn sizes_read_as_a_person_writes_them() {
    assert_eq!(size_text(0), "0 bytes");
    assert_eq!(size_text(1_023), "1023 bytes");
    assert_eq!(size_text(1_536), "1.5 KiB");
    assert_eq!(size_text(3 << 20), "3.0 MiB");
    assert!(size_text(u64::MAX).ends_with("GiB"));
}
