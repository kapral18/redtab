use super::*;

pub(super) fn app(subscriptions: &[&str]) -> App {
    let reddit = Reddit::test_at("http://127.0.0.1:9", false);
    app_with_client(subscriptions, reddit)
}

pub(super) fn app_with_client(subscriptions: &[&str], reddit: Reddit) -> App {
    let (pool, _rx) = Pool::start(reddit.clone(), 3, None);
    let state = crate::test_support::state_path();
    let mut app = App::new(
        Config {
            media: crate::config::MediaSettings {
                image_preview: false,
                ..Default::default()
            },
            ..Config::default()
        },
        pool,
        reddit,
        state,
        Picker::halfblocks(),
    );
    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok((subscriptions
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<String>>())
        .into_iter()
        .map(|name| crate::model::Subscription::Community {
            name,
            icon: crate::model::SubscriptionIcon::Unknown,
        })
        .collect()),
    });
    app.config.select_account(Some("t"));
    app.settings.store.select_account(Some("t"));
    app.active = crate::app::ALL_SUBS_TAB;
    app
}

pub(super) fn saved_account(id: &str, expired: bool) -> crate::reddit::SavedAccount {
    crate::reddit::SavedAccount {
        id: id.into(),
        name: format!("u/{id}"),
        expired,
        source: None,
    }
}

pub(super) fn screen(app: &mut App, w: u16, h: u16) -> String {
    buffer_text(&screen_buffer(app, w, h))
}

pub(super) fn screen_buffer(app: &mut App, w: u16, h: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| draw(f, app)).unwrap();
    while app.card_renderer.pending() {
        app.card_renderer.wait_for_completion();
        app.card_renderer.poll();
        terminal.draw(|f| draw(f, app)).unwrap();
    }
    terminal.backend().buffer().clone()
}

pub(super) fn first_screen(app: &mut App, w: u16, h: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| draw(f, app)).unwrap();
    buffer_text(terminal.backend().buffer())
}

pub(super) fn popup_snapshot(app: &mut App, width: u16, height: u16) -> (Rect, String) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut area = None;
    terminal.draw(|f| area = draw_overlay(f, app)).unwrap();
    (area.unwrap(), buffer_text(terminal.backend().buffer()))
}

pub(super) fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn description_popup(
    query: crate::reddit::experience::Query,
    summary: &str,
    bodies: &[&str],
) -> App {
    use crate::app::experience::Screen;
    use crate::reddit::experience::{Entry, Page};
    let mut app = app(&[]);
    app.overlay = Some(Overlay::Experience);
    app.experience.screens = vec![Screen::Browse {
        pagination: Default::default(),
        query,
        page: Page {
            title: "Collection".into(),
            text: summary.into(),
            entries: bodies
                .iter()
                .enumerate()
                .map(|(index, body)| Entry {
                    icon: Default::default(),
                    title: format!("Entry {index}"),
                    body: (*body).into(),
                    target: None,
                    query: Some(crate::reddit::experience::Query::Profile(format!(
                        "reader{index}"
                    ))),
                    post: None,
                })
                .collect(),
            after: None,
            url: "/user/reader".into(),
        },
        selected: 0,
        scroll: 0,
    }];
    app
}

#[allow(deprecated)]
pub(crate) fn kitty() -> Picker {
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    picker
}

/// Length of the `x` run in the title line of post `id`.
pub(super) fn title_run(screen: &str, id: &str) -> usize {
    let line = screen
        .lines()
        .find(|l| l.contains(&format!("{id} x")))
        .unwrap();
    line.chars().filter(|&c| c == 'x').count()
}

/// (x, y) of every cell holding a kitty image placeholder.
pub(super) fn placeholder_cells(app: &mut App, w: u16, h: u16) -> Vec<(u16, u16)> {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| draw(f, app)).unwrap();
    while app.card_renderer.pending() {
        app.card_renderer.wait_for_completion();
        app.card_renderer.poll();
        terminal.draw(|f| draw(f, app)).unwrap();
    }
    let buf = terminal.backend().buffer().clone();
    let mut cells = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if buf[(x, y)].symbol().contains('\u{10EEEE}') {
                cells.push((x, y));
            }
        }
    }
    cells
}

pub(super) fn ready(app: &App, w: u32, h: u32) -> ImageSlot {
    let image = image::DynamicImage::new_rgb8(w, h);
    ImageSlot::Ready {
        protocol: Box::new(app.image_picker.new_resize_protocol(image)),
        refresh: crate::app::ImageRefresh::Idle,
        pixels: (w, h),
    }
}

pub(super) fn press(app: &mut App, c: char) {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    crate::keys::handle(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
}

pub(super) fn press_keys(app: &mut App, keys: &str) {
    for c in keys.chars() {
        if c.is_ascii_control() {
            key(
                app,
                KeyCode::Char(char::from_u32(c as u32 + 96).unwrap()),
                KeyModifiers::CONTROL,
            );
        } else {
            press(app, c);
        }
    }
}

pub(super) fn key(
    app: &mut App,
    code: crossterm::event::KeyCode,
    mods: crossterm::event::KeyModifiers,
) {
    crate::keys::handle(app, crossterm::event::KeyEvent::new(code, mods));
}

pub(super) fn form_app(label: &str) -> App {
    use crate::app::experience::Screen;
    let mut app = app(&[]);
    app.reddit_menu();
    let Screen::Menu {
        choices, selected, ..
    } = app.experience.screens.last_mut().unwrap()
    else {
        panic!("expected menu")
    };
    *selected = choices.iter().position(|c| c.label == label).unwrap();
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    app
}

pub(crate) fn auth_content(app: &mut App, width: u16, height: u16) -> String {
    let buf = screen_buffer(app, width, height);
    (0..height)
        .filter(|&y| buf[(2, y)].symbol() == "│" && buf[(width - 3, y)].symbol() == "│")
        .map(|y| {
            (4..width - 4)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .trim()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("")
}

pub(super) fn managed_settings_app() -> (App, crate::config::managed::tests::Fixture) {
    let fixture = crate::config::managed::tests::Fixture::new();
    let mut app = app(&[]);
    app.settings = crate::app::settings::Settings::new(fixture.store());
    app.settings.chezmoi = fixture.tool.clone();
    app.overlay = Some(Overlay::Search {
        input: "retained draft".into(),
    });
    app.show_settings();
    app.settings.selected = crate::config::schema::SETTINGS
        .iter()
        .position(|field| field.key == "media.image_preview")
        .unwrap();
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    (app, fixture)
}

#[cfg(unix)]
pub(super) fn opened_browser_url(app: &mut App, keys: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("opened-url");
    app.config.commands.browser = vec![
        "/bin/sh".into(),
        "-c".into(),
        "printf '%s' \"$1\" > \"$0\"".into(),
        file.to_string_lossy().into_owned(),
    ];
    press_keys(app, keys);
    let deadline = Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if let Ok(text) = std::fs::read_to_string(&file)
            && !text.is_empty()
        {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "native browser probe did not receive a URL"
        );
        std::thread::yield_now();
    }
}
