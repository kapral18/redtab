use super::*;

#[test]
fn terminal_draw_should_hold_graphics_and_placeholders_until_complete() {
    let mut bytes = Vec::<u8>::new();
    let backend = ratatui::backend::CrosstermBackend::new(&mut bytes);
    let mut terminal = ratatui::Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 4, 2)),
        },
    )
    .unwrap();
    draw_synchronized(&mut terminal, |frame| {
        frame.buffer_mut()[(0, 0)].set_symbol("frame");
    })
    .unwrap();
    drop(terminal);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.starts_with("\x1b[?2026h"), "{text:?}");
    assert!(text.contains("frame"));
    assert!(text.rfind("\x1b[?2026l").unwrap() > text.find("frame").unwrap());
}

#[test]
fn a_failed_terminal_draw_should_end_synchronization() {
    #[derive(Default)]
    struct Writer {
        bytes: Vec<u8>,
        fail_draw: bool,
        fail_once: bool,
    }
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if std::mem::take(&mut self.fail_once) {
                return Err(std::io::Error::other("draw failed"));
            }
            self.bytes.extend_from_slice(bytes);
            if self.fail_draw && self.bytes.ends_with(b"\x1b[?2026h") {
                self.fail_draw = false;
                self.fail_once = true;
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer {
        fail_draw: true,
        ..Writer::default()
    };
    let backend = ratatui::backend::CrosstermBackend::new(&mut writer);
    let mut terminal = ratatui::Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 4, 2)),
        },
    )
    .unwrap();
    assert!(
        draw_synchronized(&mut terminal, |frame| {
            frame.buffer_mut()[(0, 0)].set_symbol("frame");
        })
        .is_err()
    );
    drop(terminal);
    assert!(writer.bytes.ends_with(b"\x1b[?2026l"));
}

#[test]
fn when_worker_results_arrive_in_a_burst_input_should_get_another_turn() {
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    let (pool, _worker_messages) = Pool::start(reddit.clone(), 3, None);
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(
        Config::default(),
        pool,
        reddit,
        dir.path().join("state.json"),
        Picker::halfblocks(),
    );
    let (sender, messages) = std::sync::mpsc::channel();
    for _ in 0..32 {
        sender
            .send(worker::Msg::CacheWarning("fixture".into()))
            .unwrap();
    }

    let drained = drain_messages(&mut app, &messages);
    assert!(drained.changed && drained.more);

    assert!(
        (24..32).contains(&messages.try_iter().count()),
        "completion processing must yield after at most eight results"
    );
}

#[test]
fn when_first_run_signin_is_needed_it_should_open_the_startup_subreddit_once_after_connecting() {
    let directory = tempfile::tempdir().unwrap();
    let make_app = |reddit: Reddit| {
        let (pool, _messages) = Pool::start(reddit.clone(), 1, None);
        App::new(
            Config::default(),
            pool,
            reddit,
            directory.path().join("state.json"),
            Picker::halfblocks(),
        )
    };
    let mut app = make_app(Reddit::disconnected(false).unwrap());
    let mut pending = Some("rust".into());
    assert!(!open_startup_subreddit(&mut app, &mut pending));
    assert_eq!(app.tabs.len(), 4);
    // Initial sign-in installs a new account-owned App. The CLI intent outlives it.
    app = make_app(Reddit::test_at("http://127.0.0.1:9", false));
    assert!(open_startup_subreddit(&mut app, &mut pending));
    assert_eq!(app.active_tab().subreddit(), Some("rust"));
    assert!(pending.is_none());
    assert!(!open_startup_subreddit(&mut app, &mut pending));
    assert_eq!(app.tabs.len(), 5);
}
