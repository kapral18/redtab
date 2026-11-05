use super::*;

#[derive(Default)]
pub(super) struct PostDocument {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) spans: Vec<(usize, usize)>,
    pub(super) pictures: Vec<DocPicture>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DocumentImage {
    Loading,
    Ready(u32, u32),
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DocumentKey {
    pub(super) focused: bool,
    pub(super) width: usize,
    pub(super) second: i64,
    pub(super) selected: Option<usize>,
    pub(super) collapsed: HashSet<String>,
    pub(super) reveal: bool,
    pub(super) hidden: bool,
    pub(super) phase: usize,
    pub(super) font: (u16, u16),
    pub(super) rows: u16,
    pub(super) images: Option<Vec<(String, DocumentImage)>>,
    pub(super) main: Option<(String, Option<Size>)>,
    pub(super) title: Option<Style>,
    pub(super) error: Option<String>,
    pub(super) loading_more: HashSet<String>,
}

#[derive(Debug)]
pub(crate) struct CachedDocument {
    pub(super) key: DocumentKey,
    pub(super) spans: Vec<(usize, usize)>,
    pub(super) pictures: Vec<DocPicture>,
}

pub(super) fn document_key(
    view: &PostView,
    width: usize,
    now: DateTime<Utc>,
    sizes: &DocumentContext<'_>,
    main: Option<(&str, Option<Size>)>,
    title: Option<Style>,
) -> DocumentKey {
    let images = sizes.images.map(|images| {
        let mut state: Vec<_> = images
            .iter()
            .map(|(url, slot)| match slot {
                ImageSlot::Ready { pixels, .. } => {
                    (url.clone(), DocumentImage::Ready(pixels.0, pixels.1))
                }
                ImageSlot::Failed(_) => (url.clone(), DocumentImage::Failed),
                ImageSlot::Loading => (url.clone(), DocumentImage::Loading),
            })
            .collect();
        state.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        state
    });
    DocumentKey {
        focused: sizes.focused,
        width,
        second: now.timestamp(),
        selected: (!view.free_scroll).then_some(view.cursor),
        collapsed: view.collapsed.clone(),
        reveal: view.reveal_spoilers,
        hidden: view.spoiler_hidden(),
        phase: sizes.phase,
        font: (sizes.font.width, sizes.font.height),
        rows: sizes.rows,
        images,
        main: main.map(|(url, size)| (url.into(), size)),
        title,
        error: view.error.clone(),
        loading_more: view.loading_more.clone(),
    }
}

/// Selftext, comment spans, pictures and More-row geometry for the displayed document.
pub(super) fn post_document(
    view: &PostView,
    width: usize,
    now: DateTime<Utc>,
    mut more_lines: Option<&mut Vec<(String, usize)>>,
    sizes: &DocumentContext<'_>,
    main_image: Option<(&str, Option<Size>)>,
    overflow_title: Option<Style>,
) -> PostDocument {
    let mut pictures = Vec::new();
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(style) = overflow_title {
        wrap_into(&mut lines, &view.shown_post().title, width, &[], style);
        lines.push(Line::default());
    }
    // Early-return documents contain no More rows.
    if (view.page.is_none() || view.spoiler_hidden())
        && let Some(rows) = &mut more_lines
    {
        rows.clear();
    }
    if let Some(e) = view.error.as_ref().filter(|_| view.page.is_none()) {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!("✗ Could not load the post: {e}"),
            Style::new().fg(RED),
        ));
        if sizes.focused {
            lines.push(Line::styled("press r to retry", Style::new().fg(OVERLAY)));
        }
        return PostDocument {
            lines,
            ..PostDocument::default()
        };
    }
    let Some(page) = view.page.as_ref() else {
        lines.push(Line::raw(""));

        return PostDocument {
            lines,
            ..PostDocument::default()
        };
    };
    lines.push(Line::raw(""));
    let reveal = view.reveal_spoilers;
    if view.spoiler_hidden() {
        // The comments stay hidden too: they discuss what the post hides.
        let comments = match page.post.comments {
            0 => String::new(),
            1 => " and its comment".into(),
            n => format!(" and its {} comments", format_count(n as i64)),
        };
        lines.push(Line::styled(
            if sizes.focused {
                format!("⚠ Spoiler · zs reveals the post{comments}")
            } else {
                format!("⚠ Spoiler · post{comments} hidden")
            },
            Style::new().fg(YELLOW).add_modifier(Modifier::BOLD),
        ));
        return PostDocument {
            lines,
            ..PostDocument::default()
        };
    }
    if let Some((url, size)) = main_image {
        let picture = DocPicture {
            url: url.to_string(),
            line: lines.len(),
            x: size.map_or(0, |size| (width as u16).saturating_sub(size.width) / 2),
            size,
            main: true,
        };
        lines.extend(std::iter::repeat_n(Line::raw(""), picture.rows() + 1));
        pictures.push(picture);
    }
    if !page.body.is_empty() {
        let body = spoiler_text(&page.body, reveal);
        let style = Style::new().fg(TEXT);
        text_into(&mut lines, &mut pictures, &body, width, &[], style, sizes);
        lines.push(Line::raw(""));
    }
    let label = format!(" 💬 {} comments ", format_count(page.post.comments as i64));
    let rule = width.saturating_sub(label.width()) / 2;
    lines.push(Line::from(vec![
        Span::styled("─".repeat(rule), Style::new().fg(SURFACE1)),
        Span::styled(label, Style::new().fg(SUBTEXT).add_modifier(Modifier::BOLD)),
        Span::styled("─".repeat(rule), Style::new().fg(SURFACE1)),
    ]));
    lines.push(Line::raw(""));
    let rows = view.rows();
    if rows.is_empty() {
        lines.push(Line::styled("   no comments yet", Style::new().fg(OVERLAY)));
    }
    let mut spans = Vec::with_capacity(rows.len());
    let mut more_count = 0;
    for (i, row) in rows.iter().enumerate() {
        let start = lines.len();
        let selected = i == view.cursor && !view.free_scroll;
        let bg = if selected {
            Style::new().bg(SURFACE0)
        } else {
            Style::new()
        };
        let bar = Span::styled(
            selection_marker(selected, sizes.focused),
            bg.fg(if sizes.focused { ACCENT } else { OVERLAY }),
        );
        let mut prefix = vec![bar.clone()];
        let visible_depth = row.depth.min(width.saturating_sub(18) / 2);
        prefix.extend(thread_prefix(row.depth, visible_depth));
        let body_width = width.saturating_sub(visible_depth * 2 + 2).max(1);
        match &row.kind {
            RowKind::Comment { comment, hidden } => {
                let mut head = prefix.clone();
                let author_color = if comment.is_op {
                    PEACH
                } else if comment.is_mod {
                    GREEN
                } else {
                    THREADS[row.depth % THREADS.len()]
                };
                head.push(Span::styled(
                    format!("u/{}", comment.author),
                    bg.fg(author_color).add_modifier(Modifier::BOLD),
                ));
                if comment.is_op {
                    head.push(Span::styled(" ", bg));
                    head.push(chip("OP", CRUST, ACCENT));
                }
                if comment.is_mod {
                    head.push(Span::styled(" ", bg));
                    head.push(chip("MOD", CRUST, GREEN));
                }
                if let Some(flair) = &comment.flair {
                    head.push(Span::styled(
                        format!(" {}", truncate(flair, 20)),
                        bg.fg(OVERLAY).add_modifier(Modifier::ITALIC),
                    ));
                }
                head.push(Span::styled(
                    format!("  ▲ {}", comment.score.map_or("•".into(), format_count)),
                    bg.fg(score_color(comment.score)),
                ));
                let age = comment.age_at(now);
                if !age.is_empty() {
                    head.push(Span::styled(format!(" · {age}"), bg.fg(OVERLAY)));
                }
                if *hidden > 0 {
                    head.push(Span::styled("  ", bg));
                    head.push(chip(format!("+{hidden} folded"), YELLOW, SURFACE1));
                }
                if selected {
                    pad_to(&mut head, width, bg);
                }
                let content = head.split_off(prefix.len());
                let mut header = wrap_spans(content, width, &prefix);
                if selected {
                    for line in &mut header {
                        pad_to(&mut line.spans, width, bg);
                    }
                }
                lines.extend(header);
                // The bar spans the whole selected comment, so it shows with the header off screen.
                let mut body_prefix = vec![if selected {
                    Span::styled("▌", Style::new().fg(ACCENT))
                } else {
                    Span::raw(" ")
                }];
                body_prefix.extend(thread_prefix(row.depth, visible_depth));
                if *hidden > 0 {
                    let body = crate::model::unformatted(&spoiler_text(&comment.body, reveal))
                        .into_owned();
                    let first = body
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .map(|l| {
                            if picture_line(l).is_some() {
                                "▣ image"
                            } else {
                                l
                            }
                        })
                        .unwrap_or_default()
                        .to_string();
                    let mut l = body_prefix.clone();
                    l.push(Span::styled(
                        truncate(&first, body_width),
                        Style::new().fg(OVERLAY),
                    ));
                    lines.push(Line::from(l));
                } else {
                    text_into(
                        &mut lines,
                        &mut pictures,
                        &spoiler_text(&comment.body, reveal),
                        body_width,
                        &body_prefix,
                        Style::new().fg(TEXT),
                        sizes,
                    );
                }
            }
            RowKind::More { parent, more } => {
                if let Some(rows) = &mut more_lines {
                    if let Some((id, line)) = rows.get_mut(more_count) {
                        if id.as_str() != *parent {
                            id.clear();
                            id.push_str(parent);
                        }
                        *line = start;
                    } else {
                        rows.push(((*parent).to_owned(), start));
                    }
                    more_count += 1;
                }
                let mut line = prefix.clone();
                let text = if view.loading_more.contains(*parent) {
                    let mut progress = loading::compact(
                        loading::Kind::More,
                        "Loading replies",
                        body_width,
                        sizes.phase,
                    );
                    for span in &mut progress.spans {
                        span.style = span.style.patch(bg);
                    }
                    line.extend(progress.spans);
                    if selected {
                        pad_to(&mut line, width, bg);
                    }
                    lines.push(Line::from(fit_spans(line, width)));
                    let end = lines.len();
                    lines.push(Line::raw(""));
                    spans.push((start, end));
                    continue;
                } else {
                    match more.count {
                        Some(n) => {
                            format!("↳ {n} more {}", if n == 1 { "reply" } else { "replies" })
                        }
                        None => "↳ more replies".to_string(),
                    }
                };
                let text = if sizes.focused {
                    format!("{text} · Enter")
                } else {
                    text
                };
                line.push(Span::styled(text, bg.fg(SKY)));
                if selected {
                    pad_to(&mut line, width, bg);
                }
                lines.push(Line::from(fit_spans(line, width)));
            }
        }
        let end = lines.len();
        lines.push(Line::raw(""));
        spans.push((start, end));
    }
    if let Some(rows) = more_lines {
        rows.truncate(more_count);
    }
    PostDocument {
        lines,
        spans,
        pictures,
    }
}
