use super::*;

#[test]
fn when_another_instance_holds_the_save_lock_corrupt_recovery_should_leave_the_file_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    std::fs::write(&path, "corrupt").unwrap();
    let held = lock(&path).unwrap();

    let (state, warning) = State::load(&path);

    assert!(state.read_only);
    assert!(warning.unwrap().contains("another redtab"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "corrupt");
    drop(held);
}

#[test]
fn the_directory_lock_lets_only_one_saver_in_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("state.json");
    let counter = dir.path().join("counter");
    std::fs::write(&counter, "0").unwrap();
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let (target, counter) = (target.clone(), counter.clone());
            std::thread::spawn(move || {
                for _ in 0..10 {
                    let held = dir_lock(&target).unwrap();
                    // Read, pause, write: without mutual exclusion increments are lost.
                    let n: u32 = std::fs::read_to_string(&counter).unwrap().parse().unwrap();
                    std::thread::yield_now();
                    std::fs::write(&counter, (n + 1).to_string()).unwrap();
                    drop(held);
                    // Let the other thread in, so neither waits out LOCK_WAIT.
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    let total = std::fs::read_to_string(&counter).unwrap();
    let lock_left = target.with_extension("json.lockdir").exists();

    assert_eq!(total, "20");
    assert!(!lock_left, "the lock is removed when released");
}

#[test]
fn an_old_directory_lock_is_never_broken_while_its_holder_lives() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("state.json");
    let lockdir = target.with_extension("json.lockdir");
    let stalled = dir_lock(&target).unwrap();
    age(&lockdir); // a holder stalled past STALE_LOCK, or a crash's leftover

    let started = std::time::Instant::now();
    let refused = dir_lock(&target).err().map(|e| e.to_string());
    let waited = started.elapsed();
    let still_held = lockdir.exists();
    drop(stalled);
    let after_release = dir_lock(&target).is_ok();

    let message = refused.expect("a second holder must be refused");
    assert!(
        message.contains("if no redtab is running, delete it"),
        "{message}"
    );
    assert!(still_held, "the holder's lock stays");
    assert!(
        waited < LOCK_WAIT / 2,
        "no wait for a lock this old: {waited:?}"
    );
    assert!(after_release);
}

/// A lock in use gives up after `LOCK_WAIT`, leaves the lock alone and does not
/// tell the reader to delete it.
#[test]
fn a_lock_in_use_gives_up_after_the_wait_and_stays() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("state.json");
    let held = dir_lock(&target).unwrap();

    let started = std::time::Instant::now();
    let refused = dir_lock(&target).err().map(|e| e.to_string());
    let waited = started.elapsed();
    let still_held = target.with_extension("json.lockdir").exists();
    drop(held);

    let message = refused.expect("a second holder must be refused");
    assert!(waited >= LOCK_WAIT && waited < LOCK_WAIT * 3, "{waited:?}");
    assert!(still_held);
    assert!(!message.contains("delete"), "{message}");
}

#[test]
fn flock_errors_choose_wait_fallback_or_fail() {
    assert_eq!(flock_failure(Some(libc::EWOULDBLOCK)), FlockFailure::Busy);
    assert_eq!(flock_failure(Some(libc::EINTR)), FlockFailure::Busy);
    for unsupported in [libc::ENOTSUP, libc::EOPNOTSUPP, libc::ENOLCK, libc::ENOSYS] {
        assert_eq!(flock_failure(Some(unsupported)), FlockFailure::Unsupported);
    }
    assert_eq!(flock_failure(Some(libc::EBADF)), FlockFailure::Error);
    assert_eq!(flock_failure(None), FlockFailure::Error);
}

#[test]
fn a_save_gives_up_when_another_instance_holds_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let held = lock(&path).unwrap();
    let mut state = State::default();
    state.toggle_bookmark(&post("a", "s"), 1);

    let started = std::time::Instant::now();
    let saved = state.save(&path, 1);
    let waited = started.elapsed();
    drop(held);
    let after = state.save(&path, 1);

    assert!(saved.unwrap_err().to_string().contains("still saving"));
    assert!(waited >= LOCK_WAIT && waited < LOCK_WAIT * 3, "{waited:?}");
    assert!(after.is_ok(), "the lock is free again");
}
