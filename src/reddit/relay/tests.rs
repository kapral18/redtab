use super::*;

#[test]
fn relay_rejects_unbounded_headers_and_closes_incomplete_connections() {
    let relay = Relay::start(Transport::new().unwrap()).unwrap();
    let address = Url::parse(&relay.base)
        .unwrap()
        .socket_addrs(|| None)
        .unwrap()[0];
    let mut oversized = TcpStream::connect(address).unwrap();
    oversized
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let prefix = "GET / HTTP/1.1\r\nX-Large: ";
    oversized
        .write_all(format!("{prefix}{}", "x".repeat(16384 - prefix.len())).as_bytes())
        .unwrap();
    let mut reader = BufReader::new(oversized);
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        if line == "\r\n" {
            break;
        }
        headers.push_str(&line);
    }
    assert!(headers.contains("Connection: close"));
    let mut bytes = [0; 256];

    let mut incomplete = TcpStream::connect(address).unwrap();
    incomplete
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    incomplete.write_all(b"GET / HTTP/1.1\r\n").unwrap();
    assert!(incomplete.read(&mut bytes).unwrap() > 0);
}

#[test]
fn relay_head_errors_keep_length_without_sending_a_body() {
    let relay = Relay::start(Transport::new().unwrap()).unwrap();
    let url = Url::parse(&relay.url("https://evil.test/file.mp4")).unwrap();
    let address = url.socket_addrs(|| None).unwrap()[0];
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let bytes = format!("HEAD {} HTTP/1.1\r\nHost: localhost\r\n\r\n", url.path());
    stream.write_all(bytes.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.contains("Content-Length: 17\r\n"));
    assert!(response.ends_with("\r\n\r\n"));
}

#[test]
fn request_slots_should_bound_active_streams_and_release_capacity() {
    let active = Arc::new(AtomicUsize::new(0));
    let mut slots: Vec<_> = (0..MAX_REQUESTS)
        .map(|_| RequestSlot::acquire(&active).unwrap())
        .collect();
    assert!(RequestSlot::acquire(&active).is_none());
    slots.pop();
    let replacement = RequestSlot::acquire(&active).unwrap();
    assert!(RequestSlot::acquire(&active).is_none());
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
