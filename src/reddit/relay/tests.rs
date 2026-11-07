use super::*;

#[test]
fn request_slots_should_bound_active_streams_and_release_capacity() {
    let active = Arc::new(AtomicUsize::new(0));
    let mut slots: Vec<_> = (0..MAX_REQUESTS)
        .map(|_| RequestSlot::acquire(&active, MAX_REQUESTS).unwrap())
        .collect();
    assert!(RequestSlot::acquire(&active, MAX_REQUESTS).is_none());
    slots.pop();
    let replacement = RequestSlot::acquire(&active, MAX_REQUESTS).unwrap();
    assert!(RequestSlot::acquire(&active, MAX_REQUESTS).is_none());
    drop(replacement);
    drop(slots);
    assert_eq!(active.load(Ordering::Relaxed), 0);
}

#[test]
fn playlists_route_segments_keys_maps_and_alternate_audio() {
    let text = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,URI=\"/audio.m3u8\"\n#EXT-X-KEY:METHOD=AES-128,URI=\"https://v.redd.it/key\"\n#EXT-X-MAP:URI=\"init.mp4\"\n../segment.ts\n";
    let base = Url::parse("https://v.redd.it/post/master.m3u8").unwrap();
    let rewritten = rewrite_playlist(text, &base, |u| {
        Ok(local_url("http://127.0.0.1:1/key/", u.as_str()))
    })
    .unwrap();
    assert_eq!(
        rewritten,
        "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,URI=\"http://127.0.0.1:1/key/https/v.redd.it/audio.m3u8\"\n#EXT-X-KEY:METHOD=AES-128,URI=\"http://127.0.0.1:1/key/https/v.redd.it/key\"\n#EXT-X-MAP:URI=\"http://127.0.0.1:1/key/https/v.redd.it/post/init.mp4\"\nhttp://127.0.0.1:1/key/https/v.redd.it/segment.ts\n"
    );
    for invalid in [
        "",
        "#EXTM3U\n#EXT-X-KEY:URI=https://v.redd.it/key",
        "#EXTM3U\n{$host}/segment.ts",
    ] {
        assert!(rewrite_playlist(invalid, &base, |u| Ok(u.to_string())).is_err());
    }
}
