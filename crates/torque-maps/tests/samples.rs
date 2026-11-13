//! Every `.dif`, `.ter` and `.dml` under the sample folder reads to its end (a `.dif` through the reading that also takes the
//! border word, `dif::parse_with_border_pad`), and every interior passes the check.
//! Skips when there is no sample folder (`TORQUE_MAPS_DIR`).

use crate::common;

use torque_maps::{dif, dml, ter};

#[test]
fn every_sample_file_reads_to_its_end() {
    let Some(root) = common::samples() else {
        println!("skipped: no sample maps (set TORQUE_MAPS_DIR)");
        return;
    };
    // (dif, ter, dml) read.
    let mut counts = [0usize; 3];
    let mut failures = Vec::new();
    let mut files = Vec::new();
    common::walk(&root, &mut files);
    for p in &files {
        let rel = p.strip_prefix(&root).unwrap_or(p);
        let shown = rel.display();
        let ext = common::ext(p);
        if !matches!(ext.as_str(), "dif" | "ter" | "dml") {
            continue;
        }
        let bytes = std::fs::read(p).unwrap();
        match ext.as_str() {
            "dif" => match dif::parse_with_border_pad(&bytes) {
                Ok(d) if d.trailing == 0 => {
                    for i in d.details.iter().chain(&d.sub_objects) {
                        if let Err(e) = i.check() {
                            failures.push(format!("{shown}: {e}"));
                        }
                        if i.hulls.hulls.is_empty() || i.surfaces.len() + i.null_surfaces.len() == 0
                        {
                            failures.push(format!("{shown}: no hulls or no surfaces"));
                        }
                    }
                    counts[0] += 1;
                }
                Ok(d) => failures.push(format!("{shown}: {} bytes left", d.trailing)),
                Err(e) => failures.push(format!("{shown}: {e}")),
            },
            "ter" => match ter::parse(&bytes) {
                Ok(t) if t.trailing == 0 => counts[1] += 1,
                Ok(t) => failures.push(format!("{shown}: {} bytes left", t.trailing)),
                Err(e) => failures.push(format!("{shown}: {e}")),
            },
            _ => match dml::parse(&bytes) {
                Ok(n) if !n.is_empty() => counts[2] += 1,
                Ok(_) => failures.push(format!("{shown}: no names")),
                Err(e) => failures.push(format!("{shown}: {e}")),
            },
        }
    }
    println!("dif/ter/dml read {counts:?}");
    assert!(failures.is_empty(), "{failures:#?}");
}
