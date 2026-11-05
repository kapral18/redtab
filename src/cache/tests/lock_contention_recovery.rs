use super::*;

#[test]
fn held_cache_locks_do_not_stall_cache_work() {
    for shared in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let holder = std::sync::Arc::new(Cache::new(dir.path().to_path_buf()));
        let lock = holder.lock().unwrap();
        // Every worker of one process shares a cache; their waits must overlap,
        // not queue one 100 ms window after another.
        let cache = if shared {
            std::sync::Arc::clone(&holder)
        } else {
            std::sync::Arc::new(Cache::new(dir.path().to_path_buf()))
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let workers: Vec<_> = (0..6)
            .map(|_| {
                let (cache, tx) = (std::sync::Arc::clone(&cache), tx.clone());
                std::thread::spawn(move || tx.send(cache.reserve_request()).unwrap())
            })
            .collect();
        let deadline = std::time::Instant::now() + Duration::from_millis(350);
        let results: Vec<_> = (0..6)
            .map_while(|_| {
                rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                    .ok()
            })
            .collect();
        drop(lock);
        workers.into_iter().for_each(|w| w.join().unwrap());

        assert_eq!(
            results.len(),
            6,
            "a held cache lock (shared={shared}) must not delay cache work"
        );
        assert!(results.iter().all(Result::is_err));
    }
}

#[test]
fn a_briefly_held_external_lock_is_waited_for() {
    let dir = tempfile::tempdir().unwrap();
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let root = dir.path().to_path_buf();
    let other_instance = std::thread::spawn(move || {
        let file = fs::File::create(root.join("cache.lock")).unwrap();
        file.lock().unwrap();
        locked_tx.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        file.unlock().unwrap();
    });
    locked_rx.recv().unwrap();

    let reserved = Cache::new(dir.path().to_path_buf()).reserve_request();
    other_instance.join().unwrap();

    assert!(reserved.is_ok(), "{reserved:?}");
}

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
    for target in ["sequence", "payload"] {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_path_buf());
        let _lock = cache.lock().unwrap();
        fs::write(dir.path().join(".tmp-123-1"), b"abandoned space").unwrap();
        fs::write(dir.path().join(".tmp-user-note"), b"keep").unwrap();
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
        assert_eq!(
            fs::read(dir.path().join(".tmp-user-note")).unwrap(),
            b"keep"
        );
        assert_eq!(calls.get(), 2);
    }
}
