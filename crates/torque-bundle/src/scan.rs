//! The leak scan: no file of a bundle may hold a folder of the computer that built it (the workspace, the build folder, the
//! cargo and rustup homes, the user's home). Each folder is looked for with either slash, with doubled backslashes (as a
//! path prints in Rust's debug form), in UTF-8 and in UTF-16, without regard to ASCII case.

use std::path::Path;

use memchr::memmem;

/// A folder to look for, by what it is (never printed: the label is).
#[derive(Clone, Debug)]
pub struct Needle {
    pub label: String,
    forms: Vec<Vec<u8>>,
}

/// Folders shorter than this are not looked for: a short one would match by chance.
const MIN_LEN: usize = 6;

impl Needle {
    /// `folder`, under `label`, or none when it is too short to look for.
    pub fn new(label: &str, folder: &Path) -> Option<Needle> {
        let text = folder.to_string_lossy();
        let text = text.trim_end_matches(['/', '\\']);
        let text = text.strip_prefix(r"\\?\").unwrap_or(text);
        if text.len() < MIN_LEN {
            return None;
        }
        let mut forms = Vec::new();
        for spelling in [
            text.replace('\\', "/"),
            text.replace('/', "\\"),
            text.replace(['/', '\\'], "\\\\"),
        ] {
            let lower = spelling.to_ascii_lowercase();
            let utf16: Vec<u8> = lower.encode_utf16().flat_map(u16::to_le_bytes).collect();
            for form in [lower.into_bytes(), utf16] {
                if !forms.contains(&form) {
                    forms.push(form);
                }
            }
        }
        Some(Needle {
            label: label.to_string(),
            forms,
        })
    }
}

/// The labels of the `needles` found in `bytes`.
pub fn scan(bytes: &[u8], needles: &[Needle]) -> Vec<String> {
    let lower = bytes.to_ascii_lowercase();
    needles
        .iter()
        .filter(|n| {
            n.forms
                .iter()
                .any(|form| memmem::find(&lower, form).is_some())
        })
        .map(|n| n.label.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn needle() -> Needle {
        Needle::new("the workspace", Path::new(r"C:\Work\Repo")).unwrap()
    }

    #[test]
    fn every_spelling_of_a_folder_is_found() {
        let n = [needle()];
        for hit in [
            r"panicked at C:\Work\Repo\src\lib.rs".as_bytes().to_vec(),
            b"c:/work/repo/src".to_vec(),
            br#"Path("C:\\Work\\Repo\\x")"#.to_vec(),
            r"C:\WORK\REPO"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect(),
        ] {
            assert_eq!(scan(&hit, &n), ["the workspace"], "{hit:?}");
        }
        assert!(scan(b"/workspace/src/lib.rs C:\\Work\\Other", &n).is_empty());
    }

    #[test]
    fn short_folders_are_not_looked_for() {
        assert!(Needle::new("root", Path::new("C:\\")).is_none());
        assert!(Needle::new("verbatim", Path::new(r"\\?\C:\Work\Repo\")).is_some());
    }
}
