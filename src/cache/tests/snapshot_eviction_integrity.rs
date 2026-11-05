use super::*;

#[test]
fn a_new_cache_handle_reads_the_previous_process_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let key = Cache::key("instance", "post");
    let cache = Cache::new(dir.path().to_path_buf());
    cache
        .store(&key, b"saved body", 1000, cache.reserve_request().unwrap())
        .unwrap();

    let entry = Cache::new(dir.path().to_path_buf())
        .load(&key, 1001)
        .unwrap();

    assert_eq!(entry.bytes, b"saved body");
    assert_eq!(entry.fetched_at, 1000);
}

#[test]
fn eviction_keeps_newer_content_within_the_byte_budget() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache {
        writers: Mutex::new(()),
        root: dir.path().to_path_buf(),
        byte_limit: HEADER + b"older body".len() as u64,
    };
    let old = Cache::key("instance", "old");
    let new = Cache::key("instance", "new");
    cache
        .store(&old, b"older body", 1000, cache.reserve_request().unwrap())
        .unwrap();

    cache
        .store(&new, b"new body", 1001, cache.reserve_request().unwrap())
        .unwrap();

    assert!(cache.load(&old, 1001).is_none());
    assert_eq!(cache.load(&new, 1001).unwrap().bytes, b"new body");
}

#[test]
fn a_write_removes_entries_past_the_retention_window_but_not_at_it() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let now = 10_000_000;
    let expired = Cache::key("instance", "expired");
    let at_limit = Cache::key("instance", "at limit");
    cache
        .store(
            &expired,
            b"expired",
            now - RETENTION - 1,
            cache.reserve_request().unwrap(),
        )
        .unwrap();
    cache
        .store(
            &at_limit,
            b"at limit",
            now - RETENTION,
            cache.reserve_request().unwrap(),
        )
        .unwrap();

    cache
        .store(
            &Cache::key("instance", "new"),
            b"new",
            now,
            cache.reserve_request().unwrap(),
        )
        .unwrap();

    assert!(!cache.path(&expired).exists());
    assert!(cache.path(&at_limit).exists());
}

#[test]
fn a_write_past_the_file_limit_evicts_the_oldest_entries_to_seven_eighths() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let now = 10_000_000;
    entries_with_one_oldest(dir.path(), FILE_LIMIT, now);

    cache
        .store(
            &Cache::key("instance", "new"),
            b"new",
            now,
            cache.reserve_request().unwrap(),
        )
        .unwrap();

    assert_eq!(entry_count(dir.path()), FILE_LIMIT / 8 * 7);
    assert!(!dir.path().join(format!("{:064x}.cache", 7)).exists());
}

#[test]
fn a_write_reaching_the_file_limit_removes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let now = 10_000_000;
    entries_with_one_oldest(dir.path(), FILE_LIMIT - 1, now);

    cache
        .store(
            &Cache::key("instance", "new"),
            b"new",
            now,
            cache.reserve_request().unwrap(),
        )
        .unwrap();

    assert_eq!(entry_count(dir.path()), FILE_LIMIT);
    assert!(dir.path().join(format!("{:064x}.cache", 7)).exists());
}

#[test]
fn invalid_header_checksums_and_versions_are_cache_misses() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let key = Cache::key("instance", "post");
    for incompatible_version in [false, true] {
        cache
            .store(&key, b"body", 1000, cache.reserve_request().unwrap())
            .unwrap();
        let mut bytes = fs::read(cache.path(&key)).unwrap();
        if incompatible_version {
            bytes[..8].copy_from_slice(b"REDTAB99");
            let checksum = Sha256::digest(&bytes[..HEADER_FIELDS]);
            bytes[HEADER_FIELDS..HEADER as usize].copy_from_slice(&checksum);
        } else {
            bytes[HEADER_FIELDS] ^= 1;
        }
        fs::write(cache.path(&key), bytes).unwrap();
        assert!(cache.load(&key, 1001).is_none());
    }
}

#[test]
fn future_and_expired_snapshots_are_not_served() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let key = Cache::key("instance", "post");
    cache
        .store(&key, b"body", 1000, cache.reserve_request().unwrap())
        .unwrap();

    assert!(cache.load(&key, 999).is_none());
    assert_eq!(cache.load(&key, 2_593_000).unwrap().bytes, b"body");
    assert!(cache.load(&key, 2_593_001).is_none());
}

#[test]
fn another_instance_or_settings_cannot_read_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    cache
        .store(
            &Cache::key("http://one\nnsfw=off", "feed"),
            b"safe posts",
            1000,
            cache.reserve_request().unwrap(),
        )
        .unwrap();

    assert!(
        cache
            .load(&Cache::key("http://two\nnsfw=off", "feed"), 1001)
            .is_none()
    );
    assert!(
        cache
            .load(&Cache::key("http://one\nnsfw=on", "feed"), 1001)
            .is_none()
    );
    assert!(
        cache
            .load(&Cache::key("http://one\nnsfw=off", "search"), 1001)
            .is_none()
    );
}

#[test]
fn symlink_entries_never_read_or_overwrite_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let key = Cache::key("instance", "post");
    let target = dir.path().join("user-file");
    fs::write(&target, b"keep me").unwrap();
    std::os::unix::fs::symlink(&target, cache.path(&key)).unwrap();
    assert!(cache.load(&key, 1000).is_none());

    cache
        .store(&key, b"cached body", 1000, cache.reserve_request().unwrap())
        .unwrap();

    assert_eq!(fs::read(target).unwrap(), b"keep me");
    assert_eq!(cache.load(&key, 1000).unwrap().bytes, b"cached body");
}

#[test]
fn fifo_payloads_and_sequence_files_should_be_cache_misses_without_a_writer() {
    use std::os::unix::ffi::OsStrExt;

    for key in ["payload", "sequence"] {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_path_buf());
        cache
            .store("payload", b"body", 1000, cache.reserve_request().unwrap())
            .unwrap();
        let path = cache.path(key);
        fs::remove_file(&path).unwrap();
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let reader =
            std::thread::spawn(move || tx.send(cache.load("payload", 1000).is_none()).unwrap());

        let result = rx.recv_timeout(Duration::from_secs(1));
        let mut held = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .unwrap();
        held.write_all(&[0; HEADER as usize]).unwrap();
        reader.join().unwrap();

        assert_eq!(result, Ok(true), "{key} FIFO must be a cache miss");
    }
}

#[test]
fn fifo_cache_locks_should_fail_without_waiting_for_a_reader() {
    use std::os::unix::ffi::OsStrExt;

    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let path = dir.path().join("cache.lock");
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let (tx, rx) = std::sync::mpsc::channel();
    let writer = std::thread::spawn(move || tx.send(cache.reserve_request().is_err()).unwrap());

    let result = rx.recv_timeout(Duration::from_secs(1));
    let _reader = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .unwrap();
    writer.join().unwrap();

    assert_eq!(
        result,
        Ok(true),
        "FIFO locks must fail before waiting for a peer"
    );
}

#[test]
fn an_older_request_finishing_later_cannot_overwrite_a_newer_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let key = Cache::key("instance", "post");
    let now = crate::app::now();
    let old = cache.reserve_request().unwrap();
    let new = cache.reserve_request().unwrap();
    cache.store(&key, b"new response", now, new).unwrap();

    Cache::new(dir.path().to_path_buf())
        .store(&key, b"old response", now + 1, old)
        .unwrap();

    let actual = cache.load(&key, now + 1).unwrap();
    assert_eq!(actual.bytes, b"new response");
    assert_eq!(actual.fetched_at, now);
}

#[test]
fn the_next_write_removes_abandoned_temporary_files_only() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    fs::write(dir.path().join(".tmp-123-1"), b"interrupted write").unwrap();
    fs::write(dir.path().join(".tmp-user-note"), b"keep").unwrap();

    cache
        .store(
            &Cache::key("instance", "post"),
            b"body",
            1000,
            cache.reserve_request().unwrap(),
        )
        .unwrap();

    assert!(!dir.path().join(".tmp-123-1").exists());
    assert_eq!(
        fs::read(dir.path().join(".tmp-user-note")).unwrap(),
        b"keep"
    );
}

#[test]
fn request_order_survives_restart_and_clock_correction() {
    let dir = tempfile::tempdir().unwrap();
    let key = Cache::key("instance", "post");
    let now = crate::app::now();
    let cache = Cache::new(dir.path().to_path_buf());
    let old = cache.reserve_request().unwrap();
    cache
        .store(&key, b"before clock correction", now + 1000, old)
        .unwrap();
    assert!(cache.load(&key, now).is_none());
    let restarted = Cache::new(dir.path().to_path_buf());
    let new = restarted.reserve_request().unwrap();
    assert_eq!(new.generation, old.generation);
    assert!(new.sequence > old.sequence);

    restarted
        .store(&key, b"after clock correction", now, new)
        .unwrap();
    // Even after the wall clock catches up, an older in-flight response cannot win.
    cache
        .store(&key, b"old request finishing late", now + 1001, old)
        .unwrap();

    assert_eq!(
        restarted.load(&key, now + 1001).unwrap().bytes,
        b"after clock correction"
    );
}

#[test]
fn counter_recovery_invalidates_outstanding_reservations() {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    let key = Cache::key("instance", "post");
    let first = cache.reserve_request().unwrap();
    cache.store(&key, b"saved response", 1000, first).unwrap();
    let outstanding = cache.reserve_request().unwrap();
    fs::write(cache.path("sequence"), b"interrupted metadata").unwrap();

    let recovered = Cache::new(dir.path().to_path_buf())
        .reserve_request()
        .unwrap();
    cache
        .store(
            &key,
            b"old completion before replacement",
            1000,
            outstanding,
        )
        .unwrap();
    assert!(cache.load(&key, 1000).is_none());
    cache.store(&key, b"replacement", 1000, recovered).unwrap();
    cache
        .store(&key, b"older completion", 1000, outstanding)
        .unwrap();

    assert_ne!(recovered.generation, outstanding.generation);
    assert_eq!(cache.load(&key, 1000).unwrap().bytes, b"replacement");
}

#[test]
fn concurrent_reservations_and_writes_keep_the_newest_request() {
    let dir = tempfile::tempdir().unwrap();
    let key = Cache::key("instance", "post");
    let cache = Cache::new(dir.path().to_path_buf());
    let initial = cache.reserve_request().unwrap();
    cache
        .store(&key, &initial.sequence.to_le_bytes(), 1000, initial)
        .unwrap();
    let mut orders = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    let order = match cache.reserve_request() {
                        Ok(order) => order,
                        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => return None,
                        Err(e) => panic!("reserve: {e}"),
                    };
                    match cache.store(&key, &order.sequence.to_le_bytes(), 1000, order) {
                        Ok(()) => Some(order.sequence),
                        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => None,
                        Err(e) => panic!("store: {e}"),
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    orders.push(initial.sequence);
    let accepted = orders.len();
    orders.sort_unstable();
    orders.dedup();
    assert_eq!(orders.len(), accepted, "accepted reservations are unique");
    let newest = *orders.last().unwrap();
    let cache = Cache::new(dir.path().to_path_buf());
    assert_eq!(cache.load(&key, 1000).unwrap().bytes, newest.to_le_bytes());
    assert!(cache.reserve_request().unwrap().sequence > newest);
}

#[test]
fn an_impossible_payload_sequence_is_ignored_and_repaired() {
    for bad_sequence in [u64::MAX, 0] {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_path_buf());
        let key = Cache::key("instance", "feed");
        let first = cache.reserve_request().unwrap();
        cache.store(&key, b"stale content", 1000, first).unwrap();
        let mut bytes = fs::read(cache.path(&key)).unwrap();
        bytes[16..24].copy_from_slice(&bad_sequence.to_le_bytes());
        let checksum = Sha256::digest(&bytes[..HEADER_FIELDS]);
        bytes[HEADER_FIELDS..HEADER as usize].copy_from_slice(&checksum);
        fs::write(cache.path(&key), bytes).unwrap();
        assert!(cache.load(&key, 1001).is_none());
        let next = cache.reserve_request().unwrap();
        cache.store(&key, b"repaired content", 1001, next).unwrap();
        let saved = cache.load(&key, 1001).unwrap();
        assert_eq!(saved.bytes, b"repaired content");
        assert_eq!(saved.order, next);
    }
}

#[test]
fn invalid_counter_values_retire_outstanding_reservations() {
    for (previous, bad, reseal) in [(1, 0, true), (1, u64::MAX, true), (2, 1, false)] {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path().to_path_buf());
        let key = Cache::key("instance", "post");
        if previous == 2 {
            cache.reserve_request().unwrap();
        }
        let old = cache.reserve_request().unwrap();
        let mut counter = fs::read(cache.path("sequence")).unwrap();
        counter[16..24].copy_from_slice(&bad.to_le_bytes());
        if reseal {
            let checksum = Sha256::digest(&counter[..HEADER_FIELDS]);
            counter[HEADER_FIELDS..].copy_from_slice(&checksum);
        }
        fs::write(cache.path("sequence"), counter).unwrap();
        let recovered = cache.reserve_request().unwrap();
        cache
            .store(&key, b"newer response", 1000, recovered)
            .unwrap();
        cache.store(&key, b"old completion", 1000, old).unwrap();
        assert_eq!(cache.load(&key, 1000).unwrap().bytes, b"newer response");
        assert_ne!(old.generation, recovered.generation);
    }
}
