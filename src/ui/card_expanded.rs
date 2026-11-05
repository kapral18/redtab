use super::*;

pub(super) fn fit_picture(
    pixels: (u32, u32),
    font: ratatui_image::FontSize,
    bounds: Size,
    viewport_height: u16,
) -> Size {
    let bounds = Size::new(
        bounds.width,
        bounds
            .height
            .min((u32::from(viewport_height) * 3 / 5) as u16),
    );
    if bounds.width == 0 || bounds.height == 0 {
        return Size::default();
    }
    picture_cells(
        pixels.0,
        pixels.1,
        font,
        bounds.width,
        bounds.height,
        bounds.height,
        f64::INFINITY,
    )
}

pub(super) fn failed_video_without_picture(app: &App, post: &Post, still: Option<&str>) -> bool {
    app.video_failed_without_frame(post)
        && still.is_none_or(|url| {
            post.media.video_url() == Some(url)
                || (!matches!(
                    app.card_images.get(url),
                    Some(CardImageSlot::Ready { .. } | CardImageSlot::Cached { .. })
                ) && !matches!(app.images.get(url), Some(ImageSlot::Ready { .. }))
                    && (matches!(app.card_images.get(url), Some(CardImageSlot::Failed(_)))
                        || matches!(app.images.get(url), Some(ImageSlot::Failed(_)))))
        })
}

pub(super) fn expanded_card(
    app: &App,
    post: &Post,
    width: usize,
    opts: &CardOpts,
    height: u16,
) -> PostCard {
    let bg = post_background(app, post, opts.selected);
    let marker = card_marker(app, opts.selected, bg);
    let article = matches!(post.media, Media::Link { .. }) && post.media.video_url().is_none();
    let image = (!post.spoiler && width >= 8 && app.inline_images_on())
        .then(|| {
            post.media
                .preview_image()
                .or_else(|| post.media.list_thumb())
                .map(std::borrow::Cow::into_owned)
        })
        .flatten();
    let decoded_size = image
        .as_deref()
        .and_then(|url| app.card_image_metadata.get(url).map(|m| m.size));
    let image_size = if article {
        decoded_size.or(post.preview_size)
    } else {
        post.preview_size.or(decoded_size)
    }
    .filter(|&(width, height)| width > 0 && height > 0);
    let (article_size, beside) = if article {
        article_picture(
            image_size,
            app.image_picker.font_size(),
            width as u16,
            height,
        )
    } else {
        (Size::default(), false)
    };
    let reserve = if article && image.is_none() && width >= 24 {
        6
    } else if article && beside {
        usize::from(article_size.width) + 2
    } else {
        0
    };
    let text_width = width.saturating_sub(reserve);
    let meta = post_meta(
        app,
        post,
        text_width,
        opts.show_subreddit,
        bg,
        vec![marker.clone()],
    );
    let mut lines = card_title(app, post, text_width, opts, height, bg);
    lines.insert(0, meta);
    if reserve > 0 {
        let monogram = if image.is_none() {
            post.media
                .tag()
                .unwrap_or_default()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(2)
                .collect::<String>()
                .to_ascii_uppercase()
        } else {
            String::new()
        };
        for (index, line) in lines.iter_mut().enumerate() {
            let mut padding = vec![Span::styled(" ".repeat(reserve), bg)];
            if index == 0 && !monogram.is_empty() {
                padding = vec![
                    Span::styled(
                        format!(" {monogram:<2} "),
                        Style::new().fg(TEXT).bg(SURFACE1),
                    ),
                    Span::styled("  ", bg),
                ];
            }
            line.spans.splice(1..1, padding);
        }
    }
    let failed_without_picture = failed_video_without_picture(app, post, image.as_deref());
    if failed_without_picture {
        lines.push(Line::from(vec![
            marker.clone(),
            Span::styled("▶ video unavailable · O opens media", bg.fg(OVERLAY)),
        ]));
    }
    let preview = !failed_without_picture
        && !post.spoiler
        && width >= 8
        && app.inline_images_on()
        && (image.is_some() || post.media.video_url().is_some());
    let fitted_size = if preview {
        if article {
            article_size
        } else {
            fit_picture(
                image_size.unwrap_or(if post.media.video_url().is_some() {
                    (16, 9)
                } else {
                    (1, 1)
                }),
                app.image_picker.font_size(),
                Size::new(
                    width.saturating_sub(4) as u16,
                    height.saturating_sub(lines.len() as u16 + 1),
                ),
                height,
            )
        }
    } else {
        Size::default()
    };
    let image = image.filter(|_| fitted_size.height > 0);
    let score = post.score.map_or("•".into(), format_count);
    let footer = Line::from(vec![
        marker.clone(),
        Span::styled(format!("▲ {score}  "), bg.fg(score_color(post.score))),
        Span::styled(
            format!("💬 {} comments", format_count(post.comments as i64)),
            bg.fg(BLUE),
        ),
    ])
    .style(bg);
    let lines_len = lines.len();
    PostCard {
        lines,
        footer,
        marker,
        style: bg,
        pixels: image_size.unwrap_or(if post.media.video_url().is_some() {
            (16, 9)
        } else {
            (1, 1)
        }),
        image,
        image_size: fitted_size,
        image_at: ratatui::layout::Position::new(
            if article && beside {
                2
            } else {
                2 + (width as u16).saturating_sub(4 + fitted_size.width) / 2
            },
            if article && beside {
                0
            } else {
                lines_len as u16
            },
        ),
    }
}

pub(super) fn article_picture(
    pixels: Option<(u32, u32)>,
    font: ratatui_image::FontSize,
    width: u16,
    height: u16,
) -> (Size, bool) {
    let cols = width.saturating_sub(4);
    let rows = (height.saturating_sub(3)).min((u32::from(height) * 3 / 10) as u16);
    let beside = width >= 48
        && pixels.is_none_or(|(w, _)| {
            f64::from(w) * 1.5 < f64::from(cols) * f64::from(font.width.max(1)) * 0.6
        });
    if cols == 0 || rows == 0 {
        return (Size::default(), beside);
    }
    let (w, h) = pixels.unwrap_or((160, 90));
    let natural_cols = ((f64::from(w) * 1.5 / f64::from(font.width.max(1))).floor() as u16).max(1);
    (
        picture_cells(
            w,
            h,
            font,
            cols.min(natural_cols)
                .min(if beside { (width / 3).min(32) } else { cols }),
            rows.min(if beside { 8 } else { rows }),
            rows.min(if beside { 8 } else { rows }),
            1.5,
        ),
        beside,
    )
}

pub(super) fn shift_card_rows(
    mut index: usize,
    mut line: usize,
    delta: isize,
    count: usize,
    height: impl Fn(usize) -> usize,
) -> (usize, usize) {
    if delta < 0 {
        let mut remaining = delta.unsigned_abs();
        while remaining > line && index > 0 {
            remaining -= line + 1;
            index -= 1;
            line = height(index);
        }
        line = line.saturating_sub(remaining);
    } else {
        line = line.saturating_add(delta as usize);
    }
    while index + 1 < count && line > height(index) {
        line -= height(index) + 1;
        index += 1;
    }
    (index, line.min(height(index)))
}

/// Row of card `target` when card `index` is drawn first with `line` rows hidden.
pub(super) fn card_row(
    index: usize,
    line: usize,
    target: usize,
    height: impl Fn(usize) -> usize,
) -> isize {
    let rows = |cards: std::ops::Range<usize>| cards.map(|i| height(i) as isize + 1).sum::<isize>();
    if target >= index {
        rows(index..target) - line as isize
    } else {
        -rows(target..index) - line as isize
    }
}

pub(super) fn card_viewport_end(
    index: usize,
    line: usize,
    viewport: usize,
    count: usize,
    height: impl Fn(usize) -> usize,
) -> (usize, usize) {
    let mut used = 0;
    let mut last = index;
    for i in index..count {
        let rows = height(i).saturating_sub(if i == index { line } else { 0 });
        if rows > 0 {
            last = i;
            used += rows;
        }
        if i + 1 < count && used < viewport {
            used += 1;
        }
        if used >= viewport {
            break;
        }
    }
    (used.min(viewport), last)
}
