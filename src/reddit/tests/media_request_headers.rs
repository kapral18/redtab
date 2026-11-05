use super::*;

#[test]
fn media_requests_are_not_page_loads() {
    // i.redd.it and v.redd.it send a browser page load to a reddit.com viewer page.
    let fixture = Fixture::new(|r| {
        let page_load = r
            .headers
            .get("accept")
            .is_some_and(|a| a.contains("text/html"))
            || r.headers.contains_key("sec-fetch-mode");
        if page_load {
            Reply {
                status: 302,
                body: String::new(),
                headers: vec![("Location".into(), "https://www.reddit.com/media".into())],
            }
        } else {
            Reply {
                status: 200,
                body: "pixels".into(),
                headers: vec![],
            }
        }
    });
    let reddit = Reddit::test_at(&fixture.base, false);
    assert_eq!(
        reddit.image(&format!("{}a.jpeg", fixture.base)).unwrap(),
        b"pixels"
    );
}

#[test]
fn disallowed_media_hosts_and_protocols_are_refused() {
    for url in [
        "http://i.redd.it/a.gif",
        "https://evil.test/a.gif",
        "https://i.redd.it.evil.test/a.gif",
        "file:///tmp/a",
        "https://user:password@i.redd.it/a",
        "https://i.redd.it:99/a",
    ] {
        assert!(!transport::media_allowed(&Url::parse(url).unwrap()));
    }
}
