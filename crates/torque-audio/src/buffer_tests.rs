use super::*;
use crate::wav::tests::pcm_wav;

/// Files in memory, by exact lower-case name.
#[derive(Default)]
pub struct MemFiles(pub HashMap<Vec<u8>, Vec<u8>>);

impl SoundFiles for MemFiles {
    fn read(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.0.get(&name.to_ascii_lowercase()).cloned()
    }
}

#[test]
fn wave_lengths_follow_the_formula() {
    // Sizes, bits, channels and rates with lengths of 1,015.56, 510.20 and 54.06 ms.
    assert_eq!(wave_len(44_786, 16, 1, 22_050), 1_015);
    assert_eq!(wave_len(5_625, 8, 1, 11_025), 510);
    assert_eq!(wave_len(596, 8, 1, 11_025), 54);
    assert_eq!(wave_len(0, 16, 1, 22_050), 0);
    // Cut toward zero: 3 frames of 16-bit mono at 2,000 Hz are 1.5 ms.
    assert_eq!(wave_len(6, 16, 1, 2_000), 1);
}

/// Files whose budget pays `.1` units.
struct Budget(MemFiles, u64);

impl SoundFiles for Budget {
    fn read(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.0.read(name)
    }

    fn charge(&mut self, units: u64) -> bool {
        self.1 = self.1.saturating_sub(units);
        self.1 > 0
    }
}

#[test]
fn reads_are_charged_and_the_budget_ends_a_loop() {
    let mut files = MemFiles::default();
    for i in 0..100 {
        files.0.insert(
            format!("s/{i}.wav").into_bytes(),
            pcm_wav(1, 22_050, 16, 1_000),
        );
    }
    let mut b = Budget(files, 30_000);
    let mut cache = BufferCache::default();
    let mut loaded = 0;
    let mut stopped = false;
    for i in 0..100 {
        match cache.find(&mut b, format!("s/{i}.wav").as_bytes()) {
            Found::Loaded(_) => loaded += 1,
            Found::OverBudget => {
                stopped = true;
                break;
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(stopped && loaded > 0 && loaded < 20, "{loaded}");
}

#[test]
fn the_memory_budget_evicts_unheld_buffers_and_refusals_are_not_cached() {
    // Four files of 1,600 bytes of samples each under a budget of 4,000: two fit.
    let mut files = MemFiles::default();
    for i in 0..4 {
        files.0.insert(
            format!("b/{i}.wav").into_bytes(),
            pcm_wav(1, 22_050, 16, 800),
        );
    }
    let mut cache = BufferCache {
        budget: 4_000,
        ..BufferCache::default()
    };
    let get =
        |c: &mut BufferCache, f: &mut MemFiles, i: u32| c.find(f, format!("b/{i}.wav").as_bytes());
    let (Found::Loaded(a), Found::Loaded(b)) = (
        get(&mut cache, &mut files, 0),
        get(&mut cache, &mut files, 1),
    ) else {
        panic!("two fit");
    };
    assert_eq!(cache.pcm_bytes, 3_200);
    // Both held elsewhere: the third is refused, and not remembered.
    assert!(matches!(get(&mut cache, &mut files, 2), Found::Failed(_)));
    assert_eq!((cache.refused, cache.len()), (1, 2));
    drop(a);
    // Now the least recently used unheld one goes, and the same name loads.
    assert!(matches!(get(&mut cache, &mut files, 2), Found::Loaded(_)));
    assert_eq!((cache.evicted, cache.pcm_bytes), (1, 3_200));
    drop(b);
}

#[test]
fn a_full_table_gives_loaded_buffers_a_slot_and_counts_them() {
    let mut files = MemFiles::default();
    files
        .0
        .insert(b"x/a.wav".to_vec(), pcm_wav(1, 22_050, 16, 10));
    let mut cache = BufferCache::default();
    for i in 0..MAX_CACHED_BUFFERS {
        cache.find(&mut files, format!("x/missing{i}.wav").as_bytes());
    }
    assert_eq!(cache.len(), MAX_CACHED_BUFFERS);
    let Found::Loaded(_b) = cache.find(&mut files, b"x/a.wav") else {
        panic!("loaded");
    };
    assert_eq!(
        cache.len(),
        MAX_CACHED_BUFFERS,
        "a negative answer gave up its slot"
    );
    assert_eq!(cache.pcm_bytes, 20);
    // Repeated loads are the same counted buffer.
    for _ in 0..10 {
        cache.find(&mut files, b"X/A.WAV");
    }
    assert_eq!(cache.pcm_bytes, 20);
}

#[test]
fn a_negative_answer_is_kept_only_for_the_exact_path_tried() {
    let mut files = MemFiles::default();
    files
        .0
        .insert(b"a/x.wav".to_vec(), pcm_wav(1, 22_050, 16, 10));
    let mut cache = BufferCache::default();
    // A spelling the host does not resolve is missing, and does not poison the real path.
    assert!(matches!(
        cache.find(&mut files, b"a//x.wav"),
        Found::Missing
    ));
    assert!(matches!(
        cache.find(&mut files, b"./a/x.wav"),
        Found::Missing
    ));
    assert!(matches!(
        cache.find(&mut files, b"a/x.wav"),
        Found::Loaded(_)
    ));
    assert!(matches!(
        cache.find(&mut files, b"A/X.WAV"),
        Found::Loaded(_)
    ));
    assert_eq!(cache.len(), 3);
}

#[test]
fn find_falls_back_to_the_ogg_twin_and_caches() {
    let mut files = MemFiles::default();
    files
        .0
        .insert(b"a/x.wav".to_vec(), pcm_wav(1, 22_050, 16, 2_205));
    files.0.insert(b"a/y.ogg".to_vec(), b"not ogg".to_vec());
    files.0.insert(b"a/z.png".to_vec(), b"png".to_vec());
    let mut cache = BufferCache::default();
    match cache.find(&mut files, b"A/X.WAV") {
        Found::Loaded(b) => assert_eq!(b.wave_len_ms(), 100),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        cache.find(&mut files, b"a/y.wav"),
        Found::Failed(_)
    ));
    assert!(matches!(
        cache.find(&mut files, b"a/z.png"),
        Found::Failed(_)
    ));
    assert!(matches!(
        cache.find(&mut files, b"a/none.wav"),
        Found::Missing
    ));
    assert!(matches!(cache.find(&mut files, b""), Found::Missing));
    assert_eq!(ogg_twin(b"x.WAV"), Some(b"x.ogg".to_vec()));
    assert_eq!(ogg_twin(b".wav"), Some(b".ogg".to_vec()));
    assert_eq!(ogg_twin(b"wav"), None);
}
