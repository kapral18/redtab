use super::*;

pub(super) struct PostCard {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) footer: Line<'static>,
    pub(super) marker: Span<'static>,
    pub(super) style: Style,
    pub(super) image: Option<String>,
    pub(super) image_size: Size,
    pub(super) image_at: ratatui::layout::Position,
    pub(super) pixels: (u32, u32),
}

impl PostCard {
    pub(super) fn height(&self) -> usize {
        self.lines
            .len()
            .max(usize::from(self.image_at.y) + usize::from(self.image_size.height))
            + 1
    }

    pub(super) fn visible(&self, skip: usize, rows: usize) -> Vec<Line<'static>> {
        (skip..skip + rows)
            .map(|line| {
                if let Some(header) = self.lines.get(line) {
                    header.clone()
                } else if line + 1 == self.height() {
                    self.footer.clone()
                } else {
                    Line::from(self.marker.clone()).style(self.style)
                }
            })
            .collect()
    }
}

pub(super) struct CardOpts {
    pub(super) selected: bool,
    pub(super) show_subreddit: bool,
    /// Columns after the selection bar kept free of text, for a thumbnail.
    pub(super) reserve: usize,
}

/// Two lines per post: score gutter + title, comment gutter + meta chips.
pub(super) fn post_card(
    app: &App,
    post: &Post,
    width: usize,
    opts: &CardOpts,
) -> [Line<'static>; 2] {
    let bg = post_background(app, post, opts.selected);
    let unseen = !app.state.is_read(&post.id);
    let read = app.state.is_read(&post.id);
    let bar_color = if app.focus == Focus::List {
        ACCENT
    } else {
        OVERLAY
    };
    let bar = Span::styled(
        selection_marker(opts.selected, app.focus == Focus::List),
        bg.fg(bar_color),
    );

    let score = post.score.map_or("•".into(), format_count);
    let slot = Span::styled(" ".repeat(opts.reserve), bg);
    let mut title = vec![
        bar.clone(),
        slot.clone(),
        Span::styled(
            format!("{score:>6} "),
            bg.fg(score_color(post.score)).add_modifier(Modifier::BOLD),
        ),
        Span::styled(if unseen { "● " } else { "  " }, bg.fg(ACCENT)),
    ];
    if app.state.is_bookmarked(&post.id) {
        title.push(Span::styled("★ ", bg.fg(YELLOW)));
    }
    if post.stickied {
        title.push(Span::styled("📌 ", bg));
    }
    let mut title_style = bg.fg(TEXT).add_modifier(Modifier::BOLD);
    if read {
        title_style = bg.fg(OVERLAY);
    }
    if app.config.highlights(post) {
        title_style = title_style.fg(YELLOW);
    }
    let room = width.saturating_sub(line_width(&title));
    title.push(Span::styled(truncate(&post.title, room), title_style));
    pad_to(&mut title, width, bg);

    let comments = format!("💬 {}", format_count(post.comments as i64));
    let meta = vec![
        bar,
        slot,
        Span::styled(
            format!(
                "{}{comments} ",
                " ".repeat(7usize.saturating_sub(comments.width()))
            ),
            bg.fg(BLUE),
        ),
        Span::styled("  ", bg),
    ];
    [
        Line::from(title),
        post_meta(app, post, width, opts.show_subreddit, bg, meta),
    ]
}

pub(super) fn post_background(app: &App, post: &Post, selected: bool) -> Style {
    if selected {
        Style::new().bg(SURFACE0)
    } else if matches!(app.active_tab().kind, TabKind::Home | TabKind::User { .. })
        && app.joined(&post.subreddit) == Some(false)
    {
        Style::new().bg(MANTLE)
    } else {
        Style::new()
    }
}

pub(super) fn post_meta(
    app: &App,
    post: &Post,
    width: usize,
    show_subreddit: bool,
    bg: Style,
    mut meta: Vec<Span<'static>>,
) -> Line<'static> {
    if post.nsfw {
        meta.push(Span::styled(" ", bg));
        meta.push(chip("nsfw", CRUST, RED));
    }
    if post.spoiler {
        meta.push(Span::styled(" ", bg));
        meta.push(chip("spoiler", CRUST, YELLOW));
    }
    if post.original_deleted || app.state.original_deleted(&post.id) {
        meta.push(Span::styled(" ", bg));
        meta.push(chip("Original post deleted", YELLOW, SURFACE0));
    }
    if show_subreddit {
        meta.push(Span::styled(
            format!("{}  ", post.source_label()),
            bg.fg(LAVENDER),
        ));
    }
    meta.push(Span::styled(format!("u/{}", post.author), bg.fg(SUBTEXT)));
    let age = post.age_at(now());
    if !age.is_empty() {
        meta.push(Span::styled(format!(" · {age}"), bg.fg(OVERLAY)));
    }
    if let Some(flair) = &post.flair {
        meta.push(Span::styled(" ", bg));
        meta.push(chip(truncate(flair, 22), MAUVE, SURFACE1));
    }
    if let Some(tag) = post.media.tag() {
        meta.push(Span::styled(" ", bg));
        meta.push(chip(media_label(&post.media, &tag), TEAL, SURFACE1));
    }
    let mut meta_line = fit_spans(meta, width);
    pad_to(&mut meta_line, width, bg);
    Line::from(meta_line).style(bg)
}

pub(super) fn media_label(media: &Media, tag: &str) -> String {
    let icon = match media {
        Media::Image { .. } if media.video_url().is_some() => "▶",
        Media::Image { .. } => "▣",
        Media::Video { .. } => "▶",
        Media::Link { .. } if media.youtube_url().is_some() => "▶",
        Media::Gallery { .. } => "▦",
        Media::Link { .. } => "↗",
        Media::None => "",
    };
    format!("{icon} {}", truncate(tag, 24))
}

pub(super) fn card_marker(app: &App, selected: bool, bg: Style) -> Span<'static> {
    Span::styled(
        format!("{} ", selection_marker(selected, app.focus == Focus::List)),
        bg.fg(if app.focus == Focus::List {
            ACCENT
        } else {
            OVERLAY
        }),
    )
}

pub(super) fn card_title(
    app: &App,
    post: &Post,
    width: usize,
    opts: &CardOpts,
    height: u16,
    bg: Style,
) -> Vec<Line<'static>> {
    let mut style = bg.fg(TEXT).add_modifier(Modifier::BOLD);
    if app.state.is_read(&post.id) {
        style = style.fg(SUBTEXT);
    }
    if app.config.highlights(post) {
        style = style.fg(YELLOW);
    }
    let saved = if app.state.is_bookmarked(&post.id) {
        "★ "
    } else {
        ""
    };
    let pinned = if post.stickied { "📌 " } else { "" };
    let unseen = if !app.state.is_read(&post.id) {
        "● "
    } else {
        ""
    };
    let title = format!("{saved}{pinned}{unseen}{}", post.title);
    let title_rows = height.saturating_sub(4).clamp(1, 3) as usize;
    let wrapped = textwrap::wrap(&title, width.saturating_sub(4).max(1));
    let clipped = wrapped.len() > title_rows;
    let mut unseen_pending = !unseen.is_empty();
    wrapped
        .into_iter()
        .take(title_rows)
        .enumerate()
        .map(|(i, text)| {
            let text = if clipped && i + 1 == title_rows {
                truncate(&format!("{text}…"), width.saturating_sub(4))
            } else {
                text.into_owned()
            };
            let mut spans = vec![card_marker(app, opts.selected, bg)];
            if unseen_pending && let Some((before, after)) = text.split_once('●') {
                spans.push(Span::styled(before.to_string(), style));
                spans.push(Span::styled("●", style.fg(ACCENT)));
                spans.push(Span::styled(after.to_string(), style));
                unseen_pending = false;
            } else {
                spans.push(Span::styled(text, style));
            }
            Line::from(spans).style(bg)
        })
        .collect()
}

/// Every island reserves room for all the posts it may show, so loading never moves the grid.
pub(super) fn card_height(app: &App) -> u16 {
    let shown = app.config.all_subs_posts_per_group.max(1) as u16;
    // borders + top/bottom padding + 2 lines per post + a gap between posts
    2 + 2 + shown * 2 + shown.saturating_sub(1)
}

pub(super) const CARD_ROW_GAP: u16 = 1;
