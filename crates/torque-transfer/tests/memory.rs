//! Peak memory of a 16 MiB upload: at most 2 MiB plus what the memory staging may hold. A counting allocator measures every byte
//! the client's reads, the chunk layer's queues and holds, the server's staging and its re-hash take above what was allocated
//! when the upload began (the content itself, which the client's source holds, is allocated before).

mod common;

use std::time::Duration;

use common::{Sim, TempFolder, content, sha256};
use peak_alloc::PeakAlloc;
use torque_transfer::{Quota, StagingConfig, UploadSpec};

#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

const MEMORY_STAGING: usize = 1 << 20;

#[test]
fn a_16_mib_upload_peaks_at_2_mib_plus_the_memory_staging_cap() {
    let folder = TempFolder::new("memory-peak");
    let bytes = content(16 << 20, 5);
    let want = sha256(&bytes);
    let spec = UploadSpec {
        max_bytes: 16 << 20,
        quota: Quota {
            bytes: 1 << 30,
            per: Duration::from_secs(86_400),
            files: 100,
        },
        rate: 8 << 20,
        server_rate: 64 << 20,
        total: Duration::from_secs(3_600),
        ..UploadSpec::default()
    };
    let staging = StagingConfig {
        memory_per_upload: MEMORY_STAGING,
        memory_total: MEMORY_STAGING,
        disk_total: 256 << 20,
        per_subject: 1 << 30,
        folder: Some(folder.0.clone()),
    };
    let mut sim = Sim::new(spec, staging, 5, 11);
    // `begin` moves the content into the client's source and hashes it a block at a time: the source is the client's own file, so
    // the measure starts after it.
    sim.begin(bytes).expect("accepted");
    ALLOC.reset_peak_usage();
    let baseline = ALLOC.current_usage();
    assert!(sim.run_until(20_000, |s| !s.finished.is_empty() && !s.ended.is_empty()));
    assert_eq!(sim.finished[0].2, want);
    let peak = ALLOC.peak_usage().saturating_sub(baseline);
    println!("peak above the baseline: {peak} bytes");
    assert!(
        peak <= (2 << 20) + MEMORY_STAGING,
        "peak {peak} bytes is above 2 MiB plus {MEMORY_STAGING}"
    );
}
