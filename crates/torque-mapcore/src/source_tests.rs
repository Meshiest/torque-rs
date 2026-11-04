use super::*;
use std::collections::BTreeMap;

/// A source over a few files, as a test importer would use one.
struct Files(BTreeMap<String, Vec<u8>>);

impl AssetSource for Files {
    fn read(&self, path: &str, max_bytes: usize) -> Option<Vec<u8>> {
        self.0
            .get(&path.to_ascii_lowercase())
            .filter(|b| b.len() <= max_bytes)
            .cloned()
    }
    fn exists(&self, path: &str) -> bool {
        self.0.contains_key(&path.to_ascii_lowercase())
    }
}

#[test]
fn a_source_reads_within_its_cap() {
    let mut m = BTreeMap::new();
    m.insert("maps/a.bsp".to_string(), vec![1, 2, 3]);
    m.insert("materials/b.vtf".to_string(), vec![4]);
    let s: &dyn AssetSource = &Files(m);
    assert_eq!(s.read("Maps/A.bsp", 3), Some(vec![1, 2, 3]));
    assert_eq!(s.read("maps/a.bsp", 2), None);
    assert!(s.exists("materials/b.vtf") && !s.exists("x"));
}
