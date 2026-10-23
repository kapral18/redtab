use super::*;
use crate::app::experience::{Form, Screen};

pub(super) fn draw(f: &mut Frame, app: &App) {
    let outer = f.area();
    let width = outer.width.saturating_sub(2).min(110);
    let height = outer.height.saturating_sub(2);
    let area = Rect::new(
        outer.x + (outer.width - width) / 2,
        outer.y + (outer.height - height) / 2,
        width,
        height,
    );
    if area.width < 4 || area.height < 4 {
        return;
    }
    f.render_widget(Clear, area);
    let block = Block::bordered()
        .title(" Reddit ")
        .border_style(Style::new().fg(ACCENT))
        .style(Style::new().bg(MANTLE).fg(TEXT));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let error_rows = if app.experience.error.is_some() {
        inner.height.min(4)
    } else {
        0
    };
    let [body, error, hint] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(error_rows),
        Constraint::Length(1),
    ])
    .areas(inner);
    let Some(screen) = app.experience.screens.last() else {
        return;
    };
    match screen {
        Screen::Menu {
            title,
            choices,
            selected,
            ..
        } => {
            let rows = body.height.saturating_sub(2) as usize;
            let offset = selected.saturating_sub(rows.saturating_sub(1));
            let mut lines = vec![
                Line::styled(clean(title), Style::new().fg(ACCENT).bold()),
                Line::default(),
            ];
            lines.extend(choices.iter().enumerate().skip(offset).take(rows).map(
                |(index, choice)| {
                    Line::styled(
                        format!(
                            "{} {}",
                            if index == *selected { "▌" } else { " " },
                            clean(&choice.label)
                        ),
                        if index == *selected {
                            Style::new().bg(SURFACE0).fg(TEXT).bold()
                        } else {
                            Style::new().fg(SUBTEXT)
                        },
                    )
                },
            ));
            f.render_widget(Paragraph::new(lines), body);
        }
        Screen::Input(form) => draw_form(f, form, body),
        Screen::Loading(query) => {
            let label = if app.experience.pending.is_some() {
                "Loading from Reddit…"
            } else {
                "Request stopped. r retries; Ctrl+o opens Reddit."
            };
            f.render_widget(
                Paragraph::new(format!("{label}\n\n{}", query.url().unwrap_or_default()))
                    .wrap(Wrap { trim: false }),
                body,
            );
        }
        Screen::Confirm { label, .. } => {
            f.render_widget(
                Paragraph::new(format!("{}\n\nEnter confirms · Esc cancels", clean(label)))
                    .style(Style::new().fg(YELLOW))
                    .wrap(Wrap { trim: false }),
                body,
            );
        }
        Screen::Browse {
            query,
            page,
            selected,
            scroll,
        } => {
            let mut lines = vec![Line::styled(
                clean(&page.title),
                Style::new().fg(ACCENT).bold(),
            )];
            if page.entries.is_empty() {
                lines.push(Line::default());
                lines.extend(clean(&page.text).lines().map(|s| Line::from(s.to_string())));
                if page.text.is_empty() {
                    lines.push(Line::from("No items."));
                }
                f.render_widget(
                    Paragraph::new(lines)
                        .wrap(Wrap { trim: false })
                        .scroll(((*scroll).min(u16::MAX as usize) as u16, 0)),
                    body,
                );
            } else {
                let [head, list, preview] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Percentage(45),
                    Constraint::Min(2),
                ])
                .areas(body);
                f.render_widget(Paragraph::new(lines), head);
                let offset = selected.saturating_sub((list.height as usize).saturating_sub(1));
                let rows: Vec<_> = page
                    .entries
                    .iter()
                    .enumerate()
                    .skip(offset)
                    .take(list.height as usize)
                    .map(|(index, entry)| {
                        Line::styled(
                            format!(
                                "{} {}",
                                if index == *selected { "▌" } else { " " },
                                clean(&entry.title)
                            ),
                            if index == *selected {
                                Style::new().fg(TEXT).bg(SURFACE0).bold()
                            } else {
                                Style::new().fg(SUBTEXT)
                            },
                        )
                    })
                    .collect();
                f.render_widget(Paragraph::new(rows), list);
                let text = if !page.text.is_empty()
                    && !matches!(query, crate::reddit::experience::Query::Thread(_))
                {
                    page.text.clone()
                } else {
                    page.entries
                        .get(*selected)
                        .map(|entry| entry.body.clone())
                        .unwrap_or_default()
                };
                let caption = if matches!(query, crate::reddit::experience::Query::Multi { .. }) {
                    " a add community · d remove community · x delete feed "
                } else if matches!(query, crate::reddit::experience::Query::Thread(_)) {
                    " Loaded comments only "
                } else {
                    " Selected content "
                };
                f.render_widget(
                    Paragraph::new(clean(&text))
                        .wrap(Wrap { trim: false })
                        .scroll(((*scroll).min(u16::MAX as usize) as u16, 0))
                        .block(
                            Block::default()
                                .borders(Borders::TOP)
                                .title(caption)
                                .border_style(Style::new().fg(SURFACE1)),
                        ),
                    preview,
                );
            }
        }
    }
    if let Some(message) = &app.experience.error {
        f.render_widget(
            Paragraph::new(clean(message))
                .style(Style::new().fg(RED))
                .wrap(Wrap { trim: false }),
            error,
        );
    }
    let label = if app.experience.writing() {
        "Sending to Reddit… wait for confirmation"
    } else if app.experience.pending.is_some() {
        "Loading… Esc back · Ctrl+o open Reddit"
    } else if inner.width < 60 && matches!(screen, Screen::Input(_)) {
        "C-s send · Esc back"
    } else if inner.width < 60 {
        "Enter open · Esc back"
    } else if matches!(screen, Screen::Input(_)) {
        "Tab field · Ctrl+s submit · Ctrl+o Reddit · Esc back"
    } else {
        "Enter open · ; actions · L next · s sort · t time · Esc back · Ctrl+o Reddit"
    };
    f.render_widget(Paragraph::new(label).style(Style::new().fg(OVERLAY)), hint);
}

fn draw_form(f: &mut Frame, form: &Form, area: Rect) {
    let mut lines = vec![
        Line::styled(clean(&form.title), Style::new().fg(ACCENT).bold()),
        Line::default(),
    ];
    let mut active_line = 0;
    for (index, field) in form.fields.iter().enumerate() {
        let active = index == form.active;
        lines.push(Line::styled(
            format!("{} {}", if active { "▌" } else { " " }, field.label),
            Style::new().fg(if active { ACCENT } else { SUBTEXT }),
        ));
        let text = if active {
            format!(
                "{}▏{}",
                clean(&field.value[..field.cursor]),
                clean(&field.value[field.cursor..])
            )
        } else {
            clean(&field.value)
        };
        let start = lines.len();
        wrap_into(
            &mut lines,
            &text,
            area.width as usize,
            &[],
            Style::new().fg(TEXT),
        );
        if active {
            let prefix = clean(&field.value[..field.cursor]);
            let mut before = Vec::new();
            wrap_into(&mut before, &prefix, area.width as usize, &[], Style::new());
            active_line = start + before.len().saturating_sub(1);
        }
        lines.push(Line::default());
    }
    let scroll = active_line
        .saturating_sub(area.height.saturating_sub(2) as usize)
        .min(u16::MAX as usize) as u16;
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
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
    fn when_a_reddit_menu_is_rendered_in_a_narrow_terminal_it_should_keep_navigation_visible() {
        let fixture = Fixture::new(|_| Reply::json(json!({})));
        let reddit = fixture.saved_reddit(&saved_export("reader", "reader"));
        let (pool, _) = crate::worker::Pool::start(reddit.clone(), 1, None);
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new(
            crate::config::Config {
                image_preview: false,
                ..Default::default()
            },
            pool,
            reddit,
            directory.path().join("state.json"),
            ratatui_image::picker::Picker::halfblocks(),
        );
        app.reddit_menu();
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal.draw(|frame| super::draw(frame, &app)).unwrap();
        let actual: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(actual.contains("My profile"));
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
