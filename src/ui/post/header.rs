use super::*;

/// Header card: never scrolls.
pub(super) fn draw_post_header(
    f: &mut Frame,
    app: &App,
    area: Rect,
    now: DateTime<Utc>,
    controls: bool,
    gallery_controls: bool,
) -> Option<(Rect, Option<Style>)> {
    let view = app.post_view.as_ref()?;
    let post = view.shown_post();
    let head_width = area.width.saturating_sub(4) as usize;
    let mut header = vec![Line::from({
        let mut s = vec![
            chip(post.source_label(), CRUST, ACCENT),
            Span::styled(format!("  u/{}", post.author), Style::new().fg(SUBTEXT)),
            Span::styled(
                Some(post.age_at(now))
                    .filter(|a| !a.is_empty())
                    .map(|a| format!(" · {a}"))
                    .unwrap_or_default(),
                Style::new().fg(OVERLAY),
            ),
        ];
        if let Some(flair) = &post.flair {
            s.push(Span::raw("  "));
            s.push(chip(truncate(flair, 28), MAUVE, SURFACE1));
        }
        if app.state.is_bookmarked(&post.id) {
            s.push(Span::styled("  ★", Style::new().fg(YELLOW)));
        }
        let mut flags = Vec::new();
        if post.nsfw {
            flags.push(chip("NSFW", CRUST, RED));
            flags.push(Span::raw(" "));
        }
        if post.spoiler {
            flags.push(chip("spoiler", CRUST, YELLOW));
            flags.push(Span::raw(" "));
        }
        if post.original_deleted || app.state.original_deleted(&post.id) {
            flags.push(chip("Original post deleted", YELLOW, SURFACE0));
            flags.push(Span::raw(" "));
        }
        flags.extend(s);
        fit_spans(flags, head_width)
    })];
    let title_style = if app.config.highlights(post) {
        Style::new().fg(YELLOW).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(TEXT).add_modifier(Modifier::BOLD)
    };
    let mut title_lines = Vec::new();
    wrap_into(&mut title_lines, &post.title, head_width, &[], title_style);
    let title_limit = area
        .height
        .saturating_div(3)
        .max(5)
        .saturating_sub(4)
        .clamp(1, 3) as usize;
    let overflow_title = (title_lines.len() > title_limit).then_some(title_style);
    if overflow_title.is_some() {
        title_lines.truncate(title_limit);
        if let Some(last) = title_lines.last_mut() {
            *last = Line::styled(truncate(&format!("{}…", last), head_width), title_style);
        }
    }
    header.extend(title_lines);
    let mut stats = vec![
        Span::styled(
            format!("▲ {}", post.score.map_or("•".into(), format_count)),
            Style::new()
                .fg(score_color(post.score))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("   💬 {}", format_count(post.comments as i64)),
            Style::new().fg(BLUE),
        ),
    ];
    match &post.media {
        Media::Gallery { images, .. } if !images.is_empty() => {
            stats.push(Span::styled(
                format!(
                    "   image {}/{}{}",
                    view.gallery_index % images.len() + 1,
                    images.len(),
                    if gallery_controls {
                        " · [i ]i browse"
                    } else {
                        ""
                    }
                ),
                Style::new().fg(TEAL),
            ));
        }
        Media::Link { url, .. } if post.media.youtube_url().is_none() => {
            stats.push(Span::raw("   "));
            stats.push(Span::styled(
                truncate(&format!("↗ {url}"), head_width.saturating_sub(24)),
                Style::new().fg(SKY).add_modifier(Modifier::UNDERLINED),
            ));
        }
        m => {
            if let Some(tag) = m.tag() {
                stats.push(Span::raw("   "));
                stats.push(chip(media_label(m, &tag), TEAL, SURFACE1));
                if controls {
                    stats.push(Span::styled("  O opens", Style::new().fg(OVERLAY)));
                }
            }
        }
    }
    header.push(Line::from(fit_spans(stats, head_width)));
    let header_height = header.len() as u16 + 2;
    let [head_area, rest] =
        Layout::vertical([Constraint::Length(header_height), Constraint::Min(1)]).areas(area);
    f.render_widget(
        Paragraph::new(header).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(SURFACE1))
                .padding(Padding::horizontal(PANEL_PAD))
                .title_bottom(Line::styled(
                    view.freshness
                        .map(|freshness| {
                            freshness.label(
                                now.timestamp(),
                                view.loading,
                                view.error.is_some(),
                                app.health == Health::Down,
                            )
                        })
                        .unwrap_or_default(),
                    Style::new().fg(OVERLAY),
                )),
        ),
        head_area,
    );
    Some((rest, overflow_title))
}
