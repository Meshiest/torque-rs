use super::*;

fn need(id: &str, byte: u8) -> RequiredMod {
    RequiredMod {
        id: id.to_string(),
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
fn hostile_sections_never_panic() {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
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
