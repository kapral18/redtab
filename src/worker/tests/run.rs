use super::*;

#[test]
fn fetches_no_longer_wanted_are_skipped() {
    // The fixture client refuses Reddit media hosts: a fetch comes back as an error.
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let url = "https://i.redd.it/a.jpeg";
    let none = Wanted::default();
    let all = Wanted::default();
    all.thumbs.lock().unwrap().insert(url.into());
    all.images.lock().unwrap().insert(url.into());
    let thumb = || Job::Thumb { url: url.into() };
    let image = || Job::Image {
        url: url.into(),
        background: false,
    };

    let thumb_skipped = run(&reddit, &thumb(), &none, &mut |_| {});
    let thumb_fetched = run(&reddit, &thumb(), &all, &mut |_| {});
    let image_skipped = run(&reddit, &image(), &none, &mut |_| {});
    let image_fetched = run(&reddit, &image(), &all, &mut |_| {});

    assert!(matches!(thumb_skipped, Msg::ThumbSkipped { url: u } if u == url));
    assert!(matches!(thumb_fetched, Msg::Thumb { result: Err(_), .. }));
    assert!(
        matches!(image_skipped, Msg::ImageSkipped { url: u } if u == url),
        "a closed post's image is not fetched"
    );
    assert!(
        matches!(image_fetched, Msg::Image { result: Err(_), .. }),
        "the open post's image is fetched"
    );
}

#[test]
fn when_cache_lookup_misses_a_queued_card_image_should_still_allow_foreground_promotion() {
    let dir = tempfile::tempdir().unwrap();
    assert_queued_image_can_promote(Some(Cache::new(dir.path().to_path_buf())));
}
