use super::*;
use crate::app::experience::{Form, Screen};

fn menu_rows(
    choices: &[crate::app::experience::Choice],
    selected: usize,
    width: usize,
) -> (Vec<Line<'static>>, std::ops::Range<usize>) {
    let mut lines = Vec::new();
    let mut section = "";
    let mut selected_rows = 0..0;
    for (index, choice) in choices.iter().enumerate() {
        if choice.section != section {
            section = choice.section;
            lines.push(Line::styled(
                section,
                Style::new()
                    .fg(if section == "Destructive" {
                        RED
                    } else {
                        OVERLAY
                    })
                    .bold(),
            ));
        }
        let first_row = lines.len();
        let active = index == selected;
        let style = choice_style(active);
        for (part, text) in textwrap::wrap(&clean(&choice.label), width.saturating_sub(2).max(1))
            .iter()
            .enumerate()
        {
            let mut spans = vec![Span::styled(
                format!("{} {text}", if active && part == 0 { "▌" } else { " " }),
                style,
            )];
            pad_to(&mut spans, width, style);
            lines.push(Line::from(spans));
        }
        if index == selected {
            selected_rows = first_row..lines.len();
        }
    }
    (lines, selected_rows)
}

fn preview_text<'a>(
    query: &crate::reddit::experience::Query,
    page: &'a crate::reddit::experience::Page,
    selected: usize,
) -> &'a str {
    if !page.text.is_empty() && !matches!(query, crate::reddit::experience::Query::Thread(_)) {
        ""
    } else {
        page.entries
            .get(selected)
            .map_or("", |entry| entry.body.as_str())
    }
}

fn preview_rows(
    query: &crate::reddit::experience::Query,
    page: &crate::reddit::experience::Page,
    width: u16,
) -> usize {
    let mut rows = 0;
    for index in 0..page.entries.len() {
        rows = rows.max(wrapped_rows(preview_text(query, page, index), width).min(10));
        if rows == 10 {
            break;
        }
    }
    rows
}

fn empty_browse_text(
    query: &crate::reddit::experience::Query,
    page: &crate::reddit::experience::Page,
    show_nsfw: bool,
) -> String {
    use crate::reddit::experience::Query;
    if !page.text.is_empty() {
        clean(&page.text)
    } else if !show_nsfw
        && matches!(
            query,
            Query::Search { .. } | Query::User { .. } | Query::Communities | Query::Multi { .. }
        )
    {
        format!("No visible items.\n\n{NSFW_EMPTY_HINT}")
    } else {
        "No items.".into()
    }
}

fn body_rows(screen: &Screen, width: u16, show_nsfw: bool) -> usize {
    match screen {
        Screen::Menu {
            choices, selected, ..
        } => menu_rows(choices, *selected, width as usize)
            .0
            .len()
            .clamp(1, 18),
        Screen::Input(form) => form_layout(form, width as usize).lines.len().max(1),
        Screen::Browse { query, page, .. } => {
            if page.entries.is_empty() {
                wrapped_rows(&empty_browse_text(query, page, show_nsfw), width).clamp(1, 18)
            } else {
                let summary = if matches!(query, crate::reddit::experience::Query::Thread(_)) {
                    0
                } else {
                    wrapped_rows(&page.text, width).min(8)
                };
                let preview = preview_rows(query, page, width);
                summary
                    + usize::from(summary > 0)
                    + page.entries.len().min(12)
                    + preview
                    + usize::from(preview > 0)
            }
        }
        Screen::Confirm { label, .. } => wrapped_rows(&clean(label), width).max(1),
        Screen::Loading(query) => 3 + wrapped_rows(&query.url().unwrap_or_default(), width).min(4),
    }
}

fn paginating(app: &App) -> bool {
    matches!(
        app.experience.pending.as_ref(),
        Some((
            _,
            crate::reddit::experience::Request::Read { after: Some(_), .. }
        ))
    )
}

fn footer(app: &App, screen: &Screen, width: usize, loader_visible: bool) -> String {
    let scroll_controls = if let Screen::Browse {
        query,
        page,
        selected,
        ..
    } = screen
        && (page.entries.is_empty()
            || (!matches!(query, crate::reddit::experience::Query::Thread(_))
                && !page.text.is_empty())
            || !preview_text(query, page, *selected).is_empty())
    {
        Some(if page.entries.is_empty() {
            fitting_label(
                &[
                    "Ctrl+d/u scroll · Ctrl+e/y line · Esc back",
                    "C-d/u scroll · C-e/y line · Esc",
                    "C-e/y scroll · Esc",
                    "C-e/y · Esc",
                    "^e/y ⎋",
                    "⎋",
                ],
                width,
            )
        } else {
            fitting_label(
                &[
                    "Enter · C-d/u scroll · C-e/y line · Esc",
                    "⏎ · C-d/u C-e/y scroll · Esc",
                    "⏎ · C-e/y scroll · Esc",
                    "⏎ · C-e/y · Esc",
                    "C-e/y · Esc",
                    "^e/y ⎋",
                    "⎋",
                ],
                width,
            )
        })
    } else {
        None
    };
    let submit = if let Screen::Input(form) = screen {
        let key = if form
            .fields
            .get(form.active)
            .is_some_and(|field| !field.multiline)
            && form.active + 1 == form.fields.len()
        {
            "Enter"
        } else {
            "Ctrl+s"
        };
        Some(format!("{key} {}", form.submit_label()))
    } else {
        None
    };
    if width < 28 {
        let (hint, short) = if app.experience.writing() {
            ("Sending…".into(), "Sending…")
        } else if app.experience.pending.is_some() && loader_visible {
            (
                "Esc back".into(),
                fitting_label(&["Esc back", "Esc", "⎋"], width),
            )
        } else if app.experience.pending.is_some() {
            (
                "Loading · Esc".into(),
                fitting_label(&["… · Esc", "… Esc", "…⎋", "⎋"], width),
            )
        } else if let Some(controls) = scroll_controls {
            (controls.into(), controls)
        } else if let Some(submit) = &submit {
            (
                format!("{} · Esc", submit.replace("Ctrl+s", "C-s")),
                if submit.starts_with("Ctrl+s") {
                    fitting_label(&["C-s · Esc", "^S Esc", "^S⎋", "⎋"], width)
                } else {
                    enter_escape(width)
                },
            )
        } else if matches!(screen, Screen::Loading(_)) {
            (
                "r retry · Esc".into(),
                fitting_label(&["r · Esc", "r Esc", "r⎋", "⎋"], width),
            )
        } else {
            ("Enter · Esc".into(), enter_escape(width))
        };
        return if hint.width() <= width {
            hint
        } else {
            truncate(short, width)
        };
    }
    if app.experience.writing() {
        return "Sending… wait for confirmation".into();
    }
    let mut parts: Vec<&str> = if app.experience.pending.is_some() {
        if loader_visible {
            vec!["Esc back"]
        } else {
            vec!["Loading…", "Esc back"]
        }
    } else {
        match screen {
            Screen::Input(_) => vec![submit.as_deref().unwrap_or_default(), "Esc back"],
            Screen::Confirm { .. } => vec!["Enter confirm", "Esc cancel"],
            Screen::Loading(_) => vec!["r retry", "Esc back"],
            Screen::Menu { .. } => vec!["Enter select", "Esc back"],
            Screen::Browse { .. } => {
                if let Some(controls) = scroll_controls {
                    vec![controls]
                } else {
                    vec!["Enter open", "Esc back"]
                }
            }
        }
    };
    if parts.join(" · ").width() + 10 <= width {
        parts.push("F1 help");
    }
    let mut extras = vec!["Ctrl+o Reddit"];
    match screen {
        Screen::Input(form) if form.fields.len() > 1 => extras.insert(0, "Tab field"),
        Screen::Menu { .. } => extras.insert(0, "j/k move"),
        Screen::Browse {
            query,
            page,
            selected,
            ..
        } => {
            if page
                .entries
                .get(*selected)
                .is_some_and(|entry| entry.target.is_some())
            {
                extras.insert(0, "; actions");
            }
            if page.after.is_some() {
                extras.push("L next");
            }
            if matches!(
                query,
                crate::reddit::experience::Query::Search {
                    kind: crate::reddit::experience::SearchKind::Posts,
                    ..
                }
            ) {
                extras.push("s sort");
            }
            if matches!(query, crate::reddit::experience::Query::Search { .. }) {
                extras.push("t time");
            }
            if matches!(query, crate::reddit::experience::Query::Multi { .. }) {
                extras.extend(["a add", "d remove", "x delete"]);
            }
        }
        _ => {}
    }
    for extra in extras {
        if parts.join(" · ").width() + 3 + extra.width() <= width {
            parts.push(extra);
        }
    }
    parts.join(" · ")
}

pub(super) fn draw(f: &mut Frame, app: &App) -> Option<Rect> {
    let screen = app.experience.screens.last()?;
    let loading_more = paginating(app);
    let mut loader_visible = false;
    let title = match screen {
        Screen::Menu { title, .. } => title.as_str(),
        Screen::Input(form) => &form.title,
        Screen::Browse { page, .. } => &page.title,
        Screen::Loading(_) => "Reddit",
        Screen::Confirm { .. } => "Confirm action",
    };
    let preferred = match screen {
        Screen::Menu { choices, .. } => choices
            .iter()
            .map(|c| c.label.width())
            .chain([title.width()])
            .max()
            .unwrap_or(36)
            .saturating_add(6)
            .clamp(44, 84),
        Screen::Browse { page, .. } => page
            .entries
            .iter()
            .map(|e| e.title.width())
            .chain(page.text.lines().map(UnicodeWidthStr::width))
            .chain([title.width()])
            .max()
            .unwrap_or(40)
            .saturating_add(6)
            .clamp(48, 84),
        Screen::Input(_) => 72,
        _ => 72,
    };
    let width = popup(f.area(), preferred as u16, u16::MAX)
        .width
        .saturating_sub(4);
    let error_message = app
        .experience
        .error
        .as_deref()
        .map(|message| format!("✗ {}", clean(message)));
    let wanted_error_rows = error_message
        .as_deref()
        .map_or(0, |message| wrapped_rows(message, width).min(4) + 1);
    let body_minimum = if matches!(screen, Screen::Input(_)) {
        2
    } else {
        1
    };
    let mut area = popup(
        f.area(),
        preferred as u16,
        (body_rows(screen, width, app.config.show_nsfw)
            + wanted_error_rows
            + 3
            + if loading_more { 4 } else { 0 })
        .min(u16::MAX as usize) as u16,
    );
    let minimum_height = body_minimum + 4; // Borders, feedback and primary controls.
    if wanted_error_rows > 0 && usize::from(area.height) < minimum_height {
        area.height = f.area().height.min(minimum_height as u16);
        area.y = f.area().y + (f.area().height - area.height) / 2;
    }
    let mut block = popup_block(&clean(title), area.width);
    if let Screen::Menu {
        choices, selected, ..
    } = screen
        && menu_rows(choices, *selected, width as usize).0.len()
            > area.height.saturating_sub(3) as usize
    {
        block = block.title_bottom(
            Line::from(format!(" {} / {} ", selected + 1, choices.len())).right_aligned(),
        );
    }
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return Some(area);
    }
    let body_minimum = body_minimum
        .min(usize::from(inner.height).saturating_sub(2))
        .max(1);
    let error_rows =
        wanted_error_rows.min(usize::from(inner.height).saturating_sub(body_minimum + 1));
    let [body, error, hint] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(error_rows as u16),
        Constraint::Length(1),
    ])
    .areas(inner);
    match screen {
        Screen::Menu {
            choices, selected, ..
        } => {
            let (lines, selected_rows) = menu_rows(choices, *selected, body.width as usize);
            let offset = selected_rows
                .end
                .saturating_sub(body.height as usize)
                .min(selected_rows.start);
            f.render_widget(
                Paragraph::new(lines.into_iter().skip(offset).collect::<Vec<_>>()),
                body,
            );
        }
        Screen::Input(form) => draw_form(f, form, body),
        Screen::Confirm { label, .. } => f.render_widget(
            Paragraph::new(clean(label))
                .style(Style::new().fg(YELLOW))
                .wrap(Wrap { trim: false }),
            body,
        ),
        Screen::Loading(query) => {
            let actions = matches!(query, crate::reddit::experience::Query::Inspect(_));
            let label = match (actions, app.experience.pending.is_some()) {
                (true, true) => "Loading actions…",
                (true, false) => "Could not load actions.",
                (false, true) => "Loading from Reddit…",
                (false, false) => "Could not load this page.",
            };
            if app.experience.pending.is_some() {
                let progress = Rect::new(body.x, body.y, body.width, body.height.min(1));
                loader_visible = loading::draw(
                    f,
                    progress,
                    loading::Kind::Initial,
                    label,
                    app.loading_frame,
                )
                .is_some();
                let destination = Rect::new(
                    body.x,
                    body.y + body.height.min(2),
                    body.width,
                    body.height.saturating_sub(2),
                );
                f.render_widget(
                    Paragraph::new(query.url().unwrap_or_default()).wrap(Wrap { trim: false }),
                    destination,
                );
            } else {
                f.render_widget(
                    Paragraph::new(format!("{label}\n\n{}", query.url().unwrap_or_default()))
                        .wrap(Wrap { trim: false }),
                    body,
                );
            }
        }
        Screen::Browse {
            query,
            page,
            selected,
            scroll,
        } => {
            loader_visible = draw_browse(f, app, query, page, *selected, *scroll, body);
        }
    }
    if let Some(message) = error_message
        && !error.is_empty()
    {
        let border = error.height > 1;
        let limit = usize::from(error.height) - usize::from(border);
        let mut lines: Vec<_> = textwrap::wrap(&message, usize::from(error.width))
            .into_iter()
            .map(|line| line.into_owned())
            .collect();
        if lines.len() > limit {
            lines.truncate(limit);
            if let Some(last) = lines.last_mut() {
                *last = truncate(&format!("{last}…"), usize::from(error.width));
            }
        }
        f.render_widget(
            Paragraph::new(lines.into_iter().map(Line::raw).collect::<Vec<_>>())
                .style(Style::new().fg(RED))
                .block(
                    Block::default()
                        .borders(if border { Borders::TOP } else { Borders::NONE })
                        .border_style(Style::new().fg(SURFACE1)),
                ),
            error,
        );
    }
    let pending_footer =
        app.experience.pending.is_some() && (app.experience.writing() || !loader_visible);
    if pending_footer && inner.width > 2 {
        let kind = if app.experience.writing() {
            loading::Kind::Action
        } else if loading_more {
            loading::Kind::More
        } else {
            loading::Kind::Initial
        };
        let text = footer(app, screen, inner.width.saturating_sub(2) as usize, false);
        f.render_widget(
            loading::compact(kind, &text, inner.width as usize, app.loading_frame),
            hint,
        );
    } else {
        f.render_widget(
            Paragraph::new(footer(app, screen, inner.width as usize, loader_visible))
                .style(Style::new().fg(OVERLAY)),
            hint,
        );
    }
    Some(area)
}

fn draw_browse(
    f: &mut Frame,
    app: &App,
    query: &crate::reddit::experience::Query,
    page: &crate::reddit::experience::Page,
    selected: usize,
    scroll: usize,
    body: Rect,
) -> bool {
    let loading_more = paginating(app);
    let show_nsfw = app.config.show_nsfw;
    if loading_more && page.entries.is_empty() {
        return loading::draw(
            f,
            body,
            loading::Kind::More,
            "Loading results",
            app.loading_frame,
        )
        .is_some();
    }
    if page.entries.is_empty() {
        let text = empty_browse_text(query, page, show_nsfw);
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .scroll((scroll.min(u16::MAX as usize) as u16, 0)),
            body,
        );
        return false;
    }
    let summary = if matches!(query, crate::reddit::experience::Query::Thread(_)) {
        ""
    } else {
        &page.text
    };
    let preview = preview_text(query, page, selected);
    let loader_rows = if loading_more {
        body.height.saturating_sub(2).min(3)
    } else {
        0
    };
    let available = body.height.saturating_sub(loader_rows);
    let has_preview = preview_rows(query, page, body.width) > 0;
    let summary_rows = wrapped_rows(summary, body.width).min(8);
    let text_reserve = if summary_rows > 0 {
        usize::from(available >= 2)
    } else if has_preview {
        usize::from(available.saturating_sub(1).min(2))
    } else {
        0
    };
    let list_limit = if has_preview && available >= 6 {
        usize::from(available / 2)
    } else {
        12
    };
    let list_rows = page
        .entries
        .len()
        .min(list_limit)
        .min(usize::from(available).saturating_sub(text_reserve));
    let remaining = usize::from(available).saturating_sub(list_rows);
    let summary_rows = summary_rows.min(remaining);
    let gap = usize::from(summary_rows > 0 && remaining > summary_rows);
    let [summary_area, _, list, loader, preview_area] = Layout::vertical([
        Constraint::Length(summary_rows as u16),
        Constraint::Length(gap as u16),
        Constraint::Length(list_rows as u16),
        Constraint::Length(loader_rows),
        Constraint::Min(0),
    ])
    .areas(body);
    f.render_widget(
        Paragraph::new(clean(summary))
            .wrap(Wrap { trim: false })
            .scroll((scroll.min(u16::MAX as usize) as u16, 0)),
        summary_area,
    );
    let offset = selected.saturating_sub((list.height as usize).saturating_sub(1));
    let rows: Vec<_> = page
        .entries
        .iter()
        .enumerate()
        .skip(offset)
        .take(list.height as usize)
        .map(|(index, entry)| {
            let style = choice_style(index == selected);
            let mut spans = vec![Span::styled(
                truncate(
                    &format!(
                        "{} {}",
                        if index == selected { "▌" } else { " " },
                        clean(&entry.title)
                    ),
                    list.width as usize,
                ),
                style,
            )];
            pad_to(&mut spans, list.width as usize, style);
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(rows), list);
    if has_preview && !preview_area.is_empty() {
        f.render_widget(
            Paragraph::new(clean(preview))
                .wrap(Wrap { trim: false })
                .scroll((scroll.min(u16::MAX as usize) as u16, 0))
                .block(
                    Block::default()
                        .borders(if preview_area.height > 1 {
                            Borders::TOP
                        } else {
                            Borders::NONE
                        })
                        .border_style(Style::new().fg(SURFACE1)),
                ),
            preview_area,
        );
    }
    loading_more
        && loading::draw(
            f,
            loader,
            loading::Kind::More,
            "Loading results",
            app.loading_frame,
        )
        .is_some()
}

struct FormLayout {
    lines: Vec<Line<'static>>,
    caret: usize,
    label: std::ops::Range<usize>,
}

fn form_layout(form: &Form, width: usize) -> FormLayout {
    let mut lines = Vec::new();
    let mut active_line = 0;
    let mut active_label = 0..0;
    for (index, field) in form.fields.iter().enumerate() {
        let active = index == form.active;
        let label = format!(
            "{} {}{}",
            if active { "▌" } else { " " },
            field.label,
            if field.required { "" } else { " (optional)" }
        );
        let label_start = lines.len();
        wrap_into(
            &mut lines,
            &label,
            width,
            &[],
            Style::new().fg(if active { ACCENT } else { SUBTEXT }),
        );
        if active {
            active_label = label_start..lines.len();
        }
        let prefix = [Span::raw("  ")];
        let value_width = width.saturating_sub(2).max(1);
        let style = Style::new().fg(TEXT).bg(SURFACE0);
        let start = lines.len();
        let caret_ordinal = clean(&field.value[..field.cursor]).matches('▏').count();
        let mut markers = 0;
        // Reserve the caret cell in every field so Tab cannot reflow the form.
        let text = format!(
            "{}▏{}",
            clean(&field.value[..field.cursor]),
            clean(&field.value[field.cursor..])
        );
        wrap_into(&mut lines, &text, value_width, &prefix, style);
        for (row, line) in lines.iter_mut().enumerate().skip(start) {
            for span in &mut line.spans {
                let mut caret_at = None;
                for (at, _) in span.content.match_indices('▏') {
                    if markers == caret_ordinal {
                        if active {
                            active_line = row;
                        } else {
                            caret_at = Some(at);
                        }
                    }
                    markers += 1;
                }
                if let Some(at) = caret_at {
                    span.content
                        .to_mut()
                        .replace_range(at..at + '▏'.len_utf8(), "");
                }
            }
            pad_to(&mut line.spans, width, style);
        }
        if index + 1 < form.fields.len() {
            lines.push(Line::default());
        }
    }
    FormLayout {
        lines,
        caret: active_line,
        label: active_label,
    }
}

fn draw_form(f: &mut Frame, form: &Form, area: Rect) {
    let FormLayout {
        lines,
        caret,
        label,
    } = form_layout(form, area.width as usize);
    let scroll = caret.saturating_sub(area.height.saturating_sub(1) as usize);
    if scroll > label.start && area.height >= 2 {
        let label_rows = label.len().min(area.height.saturating_sub(1) as usize) as u16;
        let [heading, value] =
            Layout::vertical([Constraint::Length(label_rows), Constraint::Min(1)]).areas(area);
        let labels: Vec<_> = lines
            .iter()
            .skip(label.start)
            .take(label_rows as usize)
            .cloned()
            .collect();
        f.render_widget(Paragraph::new(labels), heading);
        let offset = caret
            .saturating_sub(value.height.saturating_sub(1) as usize)
            .max(label.end);
        f.render_widget(
            Paragraph::new(lines.into_iter().skip(offset).collect::<Vec<_>>()),
            value,
        );
    } else {
        f.render_widget(
            Paragraph::new(lines).scroll((scroll.min(u16::MAX as usize) as u16, 0)),
            area,
        );
    }
}

fn clean(text: &str) -> String {
    spoiler_text(text, false)
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .filter(|c| *c != PICTURE)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reddit::tests::{Fixture, Reply, saved_export};
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;

    #[test]
    fn a_profile_with_three_links_should_render_as_a_compact_dialog() {
        use crate::reddit::experience::{Entry, Page, Query};
        let fixture = Fixture::new(|_| Reply::json(json!({})));
        let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
        let (pool, _) = crate::worker::Pool::start(reddit.clone(), 1, None);
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new(
            crate::config::Config {
                media: crate::config::MediaSettings {
                    image_preview: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            pool,
            reddit,
            directory.path().join("state.json"),
            ratatui_image::picker::Picker::halfblocks(),
        );
        app.experience.screens.push(Screen::Browse {
            query: Query::Profile("reader".into()),
            page: Page {
                title: "u/reader".into(),
                text: "Post karma: 23499    Comment karma: 43077".into(),
                entries: ["Overview", "Posts", "Comments"]
                    .map(|title| Entry {
                        title: title.into(),
                        body: String::new(),
                        target: None,
                        query: Some(Query::Profile("reader".into())),
                        post: None,
                    })
                    .into(),
                after: None,
                url: "/user/reader".into(),
            },
            selected: 2,
            scroll: 0,
        });
        let mut terminal = Terminal::new(TestBackend::new(120, 60)).unwrap();
        terminal
            .draw(|f| {
                super::draw(f, &app);
            })
            .unwrap();
        let rows: Vec<String> = terminal
            .backend()
            .buffer()
            .content()
            .chunks(120)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect();
        let occupied: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| !row.trim().is_empty())
            .collect();
        assert!(
            occupied.last().unwrap().0 - occupied.first().unwrap().0 < 12,
            "{rows:?}"
        );
        assert!(rows.iter().any(|row| row.contains("Post karma: 23499")));
        assert!(rows.iter().any(|row| row.contains("Comments")));
        assert!(!rows.iter().any(|row| row.contains("Selected content")));
    }

    #[test]
    fn failed_action_loading_should_show_retry_controls_without_submission_advice() {
        use crate::reddit::experience::{Query, Request, Target};
        let fixture = Fixture::new(|_| Reply::json(json!({})));
        let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
        let (pool, _) = crate::worker::Pool::start(reddit.clone(), 1, None);
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new(
            crate::config::Config {
                media: crate::config::MediaSettings {
                    image_preview: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            pool,
            reddit,
            directory.path().join("state.json"),
            ratatui_image::picker::Picker::halfblocks(),
        );
        let query = Query::Inspect(Target {
            id: "t1_comment".into(),
            author: "reader".into(),
            subreddit: "rust".into(),
            permalink: "/comments/post/_/comment".into(),
        });
        app.overlay = Some(Overlay::Experience);
        app.experience.screens.push(Screen::Loading(query.clone()));
        app.experience.pending = Some((91, Request::Read { query, after: None }));
        app.on_experience(91, Err("Reddit denied this request (HTTP 403)".into()));
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();

        terminal
            .draw(|frame| {
                super::draw(frame, &app);
            })
            .unwrap();

        let actual: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(actual.contains("Could not load actions"));
        assert!(actual.contains("HTTP 403"));
        assert!(actual.contains("F1 help"));
        assert!(actual.contains("r retry"));
        assert!(actual.contains("Esc back"));
        assert!(!actual.contains("submitting again"));
        assert!(!actual.contains("s sort"));
    }

    #[test]
    fn when_a_reddit_menu_is_rendered_in_a_narrow_terminal_it_should_keep_navigation_visible() {
        let fixture = Fixture::new(|_| Reply::json(json!({})));
        let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
        let (pool, _) = crate::worker::Pool::start(reddit.clone(), 1, None);
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new(
            crate::config::Config {
                media: crate::config::MediaSettings {
                    image_preview: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            pool,
            reddit,
            directory.path().join("state.json"),
            ratatui_image::picker::Picker::halfblocks(),
        );
        app.reddit_menu();
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal
            .draw(|frame| {
                super::draw(frame, &app);
            })
            .unwrap();
        let actual: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(actual.contains("Unread notifications"));
        assert!(actual.contains("Esc back"));
    }

    #[test]
    fn when_editing_raw_markdown_spoiler_delimiters_it_should_keep_the_literal_source() {
        assert_eq!(clean(">!secret!<"), ">!secret!<");
    }

    #[test]
    fn when_preview_text_has_spoilers_or_terminal_controls_it_should_mask_or_remove_them() {
        assert_eq!(
            clean(&format!("a\u{1b}{}秘密{}b", SPOILER_START, SPOILER_END)),
            "a▒▒▒▒b"
        );
    }
}
