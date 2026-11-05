use super::*;

#[test]
fn a_first_page_spends_one_request_and_the_icon_is_its_own() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let fixture = Fixture::new(move |r| {
        log.lock().push(r.url.clone());
        if r.url.starts_with("/r/rust/about.json") {
            Reply::json(
                json!({"data": {"community_icon": "https://styles.redditmedia.com/i.png", "icon_img": ""}}),
            )
        } else {
            Reply::json(empty_listing())
        }
    });
    let reddit = Reddit::test_at(&fixture.base, false);
    reddit
        .listing("rust", Sort::Hot, TimeRange::Day, None)
        .unwrap();
    let urls = seen.lock().clone();
    assert_eq!(urls.len(), 1, "{urls:?}");
    assert_eq!(
        reddit.icon("rust").unwrap().as_deref(),
        Some("https://styles.redditmedia.com/i.png")
    );
}

#[test]
fn a_missing_icon_is_an_answer_and_a_refused_or_failed_one_is_not() {
    let fixture = Fixture::new(|r| match r.url.split('/').nth(2) {
        Some("gone") => Reply::status(404),
        Some("blocked") => Reply::status(403),
        _ => Reply::status(500),
    });
    let reddit = Reddit::test_at(&fixture.base, false);
    assert_eq!(reddit.icon("gone").unwrap(), None);
    assert!(reddit.icon("blocked").is_err());
    assert!(reddit.icon("broken").is_err());
}
