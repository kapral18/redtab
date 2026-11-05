use super::*;

mod lock_contention_recovery;
mod snapshot_eviction_integrity;

/// `count` entries named like cache files, all written at `now` except entry 7,
/// written 10 s earlier.
fn entries_with_one_oldest(dir: &Path, count: usize, now: i64) {
    for i in 0..count {
        let written = if i == 7 { now - 10 } else { now };
        fs::File::create(dir.join(format!("{i:064x}.cache")))
            .unwrap()
            .set_modified(UNIX_EPOCH + Duration::from_secs(written as u64))
            .unwrap();
    }
}

fn entry_count(dir: &Path) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| name.len() == 64 + ".cache".len())
        .count()
}
