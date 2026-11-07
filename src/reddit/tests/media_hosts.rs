use super::*;

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
