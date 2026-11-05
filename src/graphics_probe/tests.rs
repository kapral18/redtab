use super::*;

#[test]
fn when_status_precedes_graphics_replies_the_probe_should_keep_the_native_protocol() {
    use ratatui_image::picker::{ProtocolType, cap_parser::QueryStdioOptions};
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    struct Buffered(std::io::BufReader<UnixStream>);
    impl std::os::fd::AsRawFd for Buffered {
        fn as_raw_fd(&self) -> std::os::fd::RawFd {
            self.0.get_ref().as_raw_fd()
        }
    }
    impl std::io::Read for Buffered {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(bytes)
        }
    }
    let (input, mut terminal) = UnixStream::pair().unwrap();
    let input = Buffered(std::io::BufReader::new(input));
    terminal.write_all(&[b'x'; 260]).unwrap();
    terminal
        .write_all(b"\x1b[0n\x1b_Gi=31;OK\x1b\\\x1b[6;18;8t")
        .unwrap();
    let mut query = Vec::new();

    let actual =
        query_image_protocol(&input, &mut query, false, QueryStdioOptions::default()).unwrap();

    assert_eq!(actual.0, Some(ProtocolType::Kitty));
    assert_eq!(
        actual.1.map(|font| (font.width, font.height)),
        Some((8, 18))
    );
}

#[test]
fn when_the_terminal_stays_silent_the_probe_should_return_at_its_deadline() {
    use ratatui_image::picker::cap_parser::QueryStdioOptions;
    use std::os::unix::net::UnixStream;
    let (input, peer) = UnixStream::pair().unwrap();
    let (cancel, wait) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        let _ = wait.recv_timeout(Duration::from_secs(1));
        drop(peer);
    });
    let started = Instant::now();

    let actual = query_image_protocol(
        &input,
        &mut Vec::new(),
        false,
        QueryStdioOptions {
            timeout: Duration::from_millis(20),
            ..QueryStdioOptions::default()
        },
    )
    .unwrap();
    let elapsed = started.elapsed();
    let _ = cancel.send(());
    watchdog.join().unwrap();

    assert!(actual.0.is_none() && actual.1.is_none());
    assert!(
        elapsed < Duration::from_millis(500),
        "silent terminal blocked for {elapsed:?}"
    );
}

fn graphics_font() -> ratatui_image::FontSize {
    ratatui_image::FontSize::new(8, 16)
}

#[test]
fn when_tmux_reports_no_graphics_or_the_probe_fails_it_should_keep_native_kitty_pixels() {
    use ratatui_image::picker::ProtocolType;
    for report in [
        None,
        Some(""),
        Some("invalid"),
        Some("halfblocks 10 20"),
        Some("kitty 0 20"),
        Some("kitty x 20"),
    ] {
        assert_eq!(
            image_protocol_choice(report, true, Some(graphics_font()))
                .map(|(p, f)| (p, f.width, f.height)),
            Some((ProtocolType::Kitty, 8, 16)),
            "{report:?}"
        );
    }
}

#[test]
fn when_the_probe_reports_native_graphics_it_should_keep_its_measured_cell_size() {
    use ratatui_image::picker::ProtocolType;
    for (report, native_tmux, expected) in [
        ("kitty 9 18", true, ProtocolType::Kitty),
        ("iterm2 9 18", false, ProtocolType::Iterm2),
        ("iterm2 9 18", true, ProtocolType::Kitty),
        ("sixel 9 18", false, ProtocolType::Sixel),
        ("sixel 9 18", true, ProtocolType::Kitty),
    ] {
        assert_eq!(
            image_protocol_choice(Some(report), native_tmux, None)
                .map(|(p, f)| (p, f.width, f.height)),
            Some((expected, 9, 18)),
            "{report}"
        );
    }
}

#[test]
fn when_client_support_or_cell_size_is_unverified_it_should_not_force_kitty() {
    assert!(image_protocol_choice(None, false, Some(graphics_font())).is_none());
    assert!(image_protocol_choice(None, true, None).is_none());
    for font in [
        ratatui_image::FontSize::new(0, 18),
        ratatui_image::FontSize::new(8, 0),
    ] {
        assert!(image_protocol_choice(None, true, Some(font)).is_none());
    }
}

#[test]
fn when_tmux_clients_confirm_graphics_the_fallback_should_use_their_measured_cells() {
    use ratatui_image::picker::ProtocolType;
    let (native, font) = tmux_client_graphics(
        "$1 xterm-ghostty 8 18\n$1 xterm-kitty 8 18\n$2 xterm-256color 16 32\n",
        &["$1"],
    );

    let actual = image_protocol_choice(None, native, font);

    assert_eq!(
        actual.map(|(protocol, font)| (protocol, font.width, font.height)),
        Some((ProtocolType::Kitty, 8, 18))
    );
    for clients in [
        "$1 xterm-ghostty 8 18\n$1 xterm-256color 8 18",
        "",
        "$1 xterm-ghostty 0 0",
        "$1 xterm-ghostty",
        "$1 xterm-ghostty 8 18\n$1 xterm-kitty 10 20",
    ] {
        let (native, font) = tmux_client_graphics(clients, &["$1"]);
        assert!(
            image_protocol_choice(None, native, font).is_none(),
            "{clients}"
        );
    }
}

#[test]
fn when_no_client_can_receive_a_tmux_windows_uploads_it_should_not_force_native_pixels() {
    let (native, font) = tmux_client_graphics("$1 xterm-ghostty 8 18\n", &["$12"]);

    assert!(image_protocol_choice(None, native, font).is_none());
    for clients in [
        "$1 xterm-ghostty 8 18\n$2 xterm-256color 8 18\n",
        "$1 xterm-ghostty 8 18\n$2 xterm-kitty 10 20\n",
    ] {
        let (native, font) = tmux_client_graphics(clients, &["$12"]);
        assert!(image_protocol_choice(None, native, font).is_none());
    }
    let (native, font) = tmux_client_graphics("$1 xterm-ghostty 8 18\n", &[]);
    assert!(image_protocol_choice(None, native, font).is_none());
}
