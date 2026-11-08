use super::*;

#[test]
fn releasing_a_writer_unlocks_inherited_file_descriptors() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let lock = cache.lock().unwrap();
    let inherited = lock._file.try_clone().unwrap();

    drop(lock);

    cache.reserve_request().unwrap();
    drop(inherited);
}

#[test]
fn full_disk_recovers_space_from_an_abandoned_writer_before_retrying() {
    let target = "payload";
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let _lock = cache.lock().unwrap();
    fs::write(dir.path().join(".tmp-123-1"), b"abandoned space").unwrap();
    let calls = std::cell::Cell::new(0);
    cache
        .atomic_replace(&cache.path(target), |temp| {
            calls.set(calls.get() + 1);
            if dir.path().join(".tmp-123-1").exists() {
                return Err(std::io::Error::from_raw_os_error(libc::ENOSPC));
            }
            fs::write(temp, b"recovered")
        })
        .unwrap();
    assert_eq!(fs::read(cache.path(target)).unwrap(), b"recovered");
    assert_eq!(calls.get(), 2);
}
