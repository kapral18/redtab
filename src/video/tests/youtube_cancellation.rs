use super::*;

#[test]
fn when_a_resolving_player_is_dropped_it_should_cancel_without_waiting_for_the_worker() {
    let (_sender, resolving) = sync_channel(1);
    let cancelled = Arc::new(youtube::Cancellation::default());
    let mut player = Player::new(
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
        Size::new(20, 10),
        Some(resolving),
        None,
        Box::new(|_, _| Err(io::Error::other("unused"))),
        Resume::default(),
        Instant::now(),
    );
    player.resolve_cancel = Some(cancelled.clone());
    assert!(player.resolving());
    let started = Instant::now();

    drop(player);

    assert!(cancelled.is_cancelled());
    assert!(started.elapsed() < Duration::from_secs(2));
}
