use super::*;

#[test]
fn when_a_terminal_image_is_received_it_should_allow_retry_before_the_old_worker_returns() {
    let (pool, work) = queued_image_pool();
    let url = "https://preview.redd.it/retry.png".to_owned();
    let job = || Job::Image {
        url: url.clone(),
        background: true,
    };
    pool.submit(job());
    let old_ticket = work.recv().unwrap().image_ticket.unwrap();
    assert!(!old_ticket.claim().swap(true, Ordering::AcqRel));
    let mut claim = ImageClaim {
        wanted: &pool.wanted,
        url: Some(&url),
        ticket: Some(old_ticket.claim()),
        claimed: true,
    };
    let (sender, receiver) = sync_channel(1);

    assert!(claim.finish(&sender, Msg::ImageSkipped { url: url.clone() }));
    assert!(matches!(next_message(&receiver), Msg::ImageSkipped { .. }));
    // The sender's guard is deliberately still alive when the receiver retries.
    pool.submit(job());
    let retry_ticket = work.recv().unwrap().image_ticket.unwrap();

    assert!(
        !retry_ticket.claim().load(Ordering::Acquire),
        "a retry must have unclaimed work, even before the old guard drops"
    );
    assert!(
        old_ticket.claim().load(Ordering::Acquire),
        "obsolete promotion copies must remain claimed"
    );
    drop(claim);
    assert!(Arc::ptr_eq(
        pool.wanted.image_tickets.lock().unwrap().get(&url).unwrap(),
        retry_ticket.claim(),
    ));
}

#[test]
fn when_every_queued_copy_of_an_image_is_dropped_unclaimed_its_ticket_should_be_forgotten() {
    let (pool, work) = queued_image_pool();
    let url = "https://preview.redd.it/skipped.png".to_owned();
    let job = || Job::Image {
        url: url.clone(),
        background: true,
    };
    pool.submit(job());
    pool.submit(job());
    let first = work.recv().unwrap();
    let second = work.recv().unwrap();

    drop(first);
    assert!(pool.wanted.image_tickets.lock().unwrap().contains_key(&url));
    drop(second);
    assert!(!pool.wanted.image_tickets.lock().unwrap().contains_key(&url));
}
