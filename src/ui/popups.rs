use super::*;

pub(super) fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(if area.width >= 8 {
        POPUP_OUTER_X * 2
    } else {
        0
    }));
    let h = height.min(area.height.saturating_sub(if area.height >= 6 {
        POPUP_OUTER_Y * 2
    } else {
        0
    }));
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

pub(super) fn enter_escape(width: usize) -> &'static str {
    fitting_label(&["Enter · Esc", "⏎ · Esc", "⏎⎋", "⎋"], width)
}

pub(super) fn dialog_controls(action: &str, width: usize) -> String {
    let full = format!("Enter {action} · Esc cancel");
    if full.width() <= width {
        full
    } else {
        enter_escape(width).into()
    }
}

pub(super) fn identity_lines(text: &str, width: usize, color: Color) -> Vec<Line<'static>> {
    let options = textwrap::Options::new(width.max(1))
        .word_separator(textwrap::WordSeparator::AsciiSpace)
        .word_splitter(textwrap::WordSplitter::NoHyphenation);
    textwrap::wrap(text, options)
        .into_iter()
        .map(|row| Line::styled(row.into_owned(), Style::new().fg(color)))
        .collect()
}

pub(super) fn popup_block(title: &str, width: u16) -> Block<'static> {
    let title = truncate(title, usize::from(width.saturating_sub(4)));
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .style(Style::new().bg(MANTLE).fg(TEXT))
        .padding(Padding::horizontal(PANEL_PAD))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("{title} "),
                Style::new().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ]))
}

pub(super) fn draw_picker(f: &mut Frame, app: &App, input: &str, selected: usize) -> Rect {
    let entries = app.picker_entries(input);
    let mut area = popup(f.area(), 56, entries.len().clamp(1, 12) as u16 + 6);
    let compact = area.height.saturating_sub(2) < 5;
    if compact {
        area = popup(f.area(), 56, entries.len().clamp(1, 12) as u16 + 3);
    }
    f.render_widget(Clear, area);
    let mut block = popup_block("Jump", area.width);
    if compact {
        let keys = if entries.is_empty() {
            "Esc close"
        } else {
            enter_escape(usize::from(area.width.saturating_sub(4)))
        };
        block = block.title_bottom(Line::styled(format!(" {keys} "), Style::new().fg(SUBTEXT)));
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [input_area, list_area, hint_area] = Layout::vertical([
        Constraint::Length(if compact { 1 } else { 3 }),
        Constraint::Min(0),
        Constraint::Length(if compact { 0 } else { 1 }),
    ])
    .areas(inner);
    let room = usize::from(input_area.width).saturating_sub(if compact { 4 } else { 6 });
    let mut input = Paragraph::new(Line::from(vec![
        Span::styled(" › ", Style::new().fg(ACCENT).bold()),
        Span::styled(input_tail(input, room), Style::new().fg(TEXT)),
        Span::styled("▏", Style::new().fg(ACCENT)),
    ]))
    .style(Style::new().bg(SURFACE0));
    if !compact {
        input = input.block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(SURFACE1)),
        );
    }
    f.render_widget(input, input_area);
    let height = list_area.height as usize;
    let start = selected.saturating_sub(height.saturating_sub(1));
    let width = list_area.width as usize;
    let lines: Vec<Line> = if entries.is_empty() {
        vec![Line::styled(
            truncate("No matching tab or valid subreddit", width),
            Style::new().fg(OVERLAY),
        )]
    } else {
        entries
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(i, entry)| {
                let bg = choice_style(i == selected);
                let mut spans = vec![Span::styled(
                    if i == selected { "▌" } else { " " },
                    bg.fg(ACCENT),
                )];
                match entry {
                    PickerEntry::Tab { index, title } => {
                        let icon = match app.tabs[*index].kind {
                            TabKind::Home => "⌂",
                            TabKind::AllSubs => "▦",
                            TabKind::Bookmarks => "★",
                            TabKind::Search => "⌕",
                            TabKind::Subreddit { .. } => "#",
                            TabKind::User { .. } => "@",
                        };
                        spans.push(Span::styled(format!(" {icon} "), bg.fg(OVERLAY)));
                        let badge = app.badge(&app.tabs[*index]);
                        let label = match badge {
                            UnreadBadge::Unknown => "?".into(),
                            UnreadBadge::Count(n) if n > 0 => n.to_string(),
                            _ => String::new(),
                        };
                        let badge_width = if label.is_empty() {
                            0
                        } else {
                            label.width() + 2
                        };
                        spans.push(Span::styled(
                            truncate(
                                title,
                                width.saturating_sub(line_width(&spans) + badge_width),
                            ),
                            bg.fg(TEXT),
                        ));
                        if !label.is_empty() {
                            pad_to(&mut spans, width.saturating_sub(badge_width), bg);
                            spans.push(match badge {
                                UnreadBadge::Unknown => chip(label, OVERLAY, SURFACE0),
                                _ => chip(label, CRUST, ACCENT),
                            });
                        }
                    }
                    PickerEntry::OpenUser(name) => {
                        spans.push(Span::styled(format!(" @ open u/{name}"), bg.fg(SKY)));
                    }
                    PickerEntry::Open(name) => {
                        spans.push(Span::styled(" + ", bg.fg(GREEN)));
                        spans.push(Span::styled(format!("open r/{name}"), bg.fg(GREEN)));
                    }
                }
                let mut spans = fit_spans(spans, width);
                pad_to(&mut spans, width, bg);
                Line::from(spans)
            })
            .collect()
    };
    f.render_widget(Paragraph::new(lines), list_area);
    let mut controls = Vec::new();
    for (key, label) in [("⏎", "open"), ("Esc", "close"), ("↑↓", "move")] {
        if entries.is_empty() && key != "Esc" {
            continue;
        }
        let pair = [
            key_chip(key),
            Span::styled(format!(" {label}  "), Style::new().fg(OVERLAY)),
        ];
        if line_width(&controls) + line_width(&pair) <= hint_area.width as usize {
            controls.extend(pair);
        }
    }
    let controls = if hint_area.width < 20 {
        Line::from(if entries.is_empty() {
            "Esc close"
        } else {
            enter_escape(usize::from(hint_area.width))
        })
    } else {
        Line::from(controls)
    };
    f.render_widget(controls, hint_area);
    area
}

pub(super) fn draw_sort(f: &mut Frame, app: &App, selected: usize) -> Rect {
    let sorts = app.active_tab().sorts();
    let home = matches!(app.active_tab().kind, TabKind::Home);
    let area = popup(f.area(), 34, sorts.len() as u16 + 2);
    let visible = area.height.saturating_sub(2) as usize;
    let offset = selected.saturating_sub(visible.saturating_sub(1));
    f.render_widget(Clear, area);
    let lines: Vec<Line> = sorts
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(i, s)| {
            let bg = choice_style(i == selected);
            let name = s.as_str();
            let mut spans = vec![
                Span::styled(if i == selected { "▌" } else { " " }, bg.fg(ACCENT)),
                Span::styled(
                    format!(" {} ", if *s == Sort::Hot { "H" } else { &name[..1] }),
                    bg.fg(LAVENDER).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                sort_chip(*s),
            ];
            if home && *s == Sort::Best {
                spans.push(Span::styled(" recommendations", bg.fg(TEXT)));
            }
            let mut spans = fit_spans(spans, area.width.saturating_sub(4) as usize);
            pad_to(&mut spans, area.width.saturating_sub(4) as usize, bg);
            Line::from(spans)
        })
        .collect();
    let block = popup_block("Sort", area.width).title_bottom(Line::styled(
        format!(
            " {} ",
            dialog_controls("apply", usize::from(area.width.saturating_sub(4)))
        ),
        Style::new().fg(SUBTEXT),
    ));
    f.render_widget(Paragraph::new(lines).block(block), area);
    area
}

pub(super) fn draw_membership(
    f: &mut Frame,
    target: &crate::model::MembershipTarget,
    join: bool,
) -> Rect {
    let action = target.action(join);
    let available = popup(f.area(), 54, u16::MAX);
    let width = usize::from(available.width.saturating_sub(4)).max(1);
    let mut lines = identity_lines(&target.label(), width, TEXT);
    for line in &mut lines {
        line.style = line.style.bold();
    }
    let hint = fitting_label(
        &[
            if matches!(target, crate::model::MembershipTarget::User(_)) {
                "o open profile"
            } else {
                "o open subreddit"
            },
            "o open",
        ],
        width,
    );
    let mut area = popup(
        f.area(),
        54,
        (lines.len() + usize::from(!hint.is_empty()) + 2).min(u16::MAX as usize) as u16,
    );
    let minimum = lines.len().saturating_add(2).min(u16::MAX as usize) as u16;
    if area.height < minimum {
        area.height = f.area().height.min(minimum);
        area.y = f.area().y + (f.area().height - area.height) / 2;
    }
    let block = popup_block(&format!("{action} on Reddit"), area.width).title_bottom(Line::styled(
        format!(" {} ", dialog_controls(&action.to_ascii_lowercase(), width)),
        Style::new().fg(SUBTEXT),
    ));
    let inner = block.inner(area);
    if lines.len() > usize::from(inner.height) {
        lines.truncate(usize::from(inner.height));
        if let Some(last) = lines.last_mut() {
            *last = Line::styled(truncate(&format!("{last}…"), width), last.style);
        }
    }
    if lines.len() < usize::from(inner.height) && !hint.is_empty() {
        lines.push(Line::styled(hint, Style::new().fg(SUBTEXT)));
    }
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(block), area);
    area
}

pub(super) fn draw_search(f: &mut Frame, app: &App, input: &str) -> Rect {
    let area = popup(f.area(), 60, 3);
    f.render_widget(Clear, area);
    let title = format!("Search {}", app.active_tab().title());
    // Borders and padding (4), "🔍 " (3: the emoji is 2 wide), the caret (1).
    let room = (area.width as usize).saturating_sub(4 + "🔍 ".width() + 1);
    let shown = input_tail(input, room);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("🔍 ", Style::new().fg(SKY)),
            Span::styled(shown, Style::new().fg(TEXT)),
            Span::styled("▏", Style::new().fg(ACCENT)),
        ]))
        .block(popup_block(&title, area.width).title_bottom(Line::styled(
            format!(
                " {} ",
                dialog_controls("search", usize::from(area.width.saturating_sub(4)))
            ),
            Style::new().fg(SUBTEXT),
        ))),
        area,
    );
    area
}

/// Input fitted to `room` columns; a longer input shows its end after "…".
pub(super) fn input_tail(input: &str, room: usize) -> String {
    if room == 0 {
        return String::new();
    }
    if input.width() <= room {
        return input.to_string();
    }
    let mut tail: Vec<&str> = Vec::new();
    let mut used = 1;
    for g in input.graphemes(true).rev() {
        if used + g.width() > room {
            break;
        }
        used += g.width();
        tail.push(g);
    }
    tail.reverse();
    format!("…{}", tail.concat())
}
