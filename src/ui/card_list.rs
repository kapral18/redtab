use super::*;

pub(super) fn view_header(
    f: &mut Frame,
    area: Rect,
    title: &str,
    chips: Vec<Span<'static>>,
) -> Rect {
    let width = usize::from(area.width);
    let maximum = area.height.saturating_sub(1).min(3);
    let gap = if area.width >= 100 && maximum >= 3 {
        PANEL_GAP
    } else {
        0
    };
    let chip_rows = maximum.saturating_sub(1 + gap) as usize;
    let mut lines = vec![Line::from(fit_spans(
        vec![chip(title, CRUST, ACCENT)],
        width,
    ))];
    let chips = chips
        .into_iter()
        .enumerate()
        .flat_map(|(index, chip)| {
            if index == 0 {
                vec![chip]
            } else {
                vec![Span::raw(" "), chip]
            }
        })
        .collect();
    lines.extend(wrap_spans(chips, width, &[]).into_iter().take(chip_rows));
    // Request activity must not move the list under the selection.
    let height = maximum;
    let [head, rest] =
        Layout::vertical([Constraint::Length(height), Constraint::Min(1)]).areas(area);
    f.render_widget(Paragraph::new(lines), head);
    rest
}

pub(super) fn sort_chip(sort: Sort) -> Span<'static> {
    let (icon, color) = match sort {
        Sort::Best => ("✦", LAVENDER),
        Sort::Hot => ("🔥", PEACH),
        Sort::New => ("✦", GREEN),
        Sort::Top => ("▲", YELLOW),
        Sort::Rising => ("↗", TEAL),
        Sort::Controversial => ("⚡", RED),
    };
    chip(format!("{icon} {}", sort.as_str()), color, SURFACE0)
}

pub(super) fn list_view_chip(app: &App) -> Span<'static> {
    let key = if matches!(
        crate::keys::map::context(app),
        crate::keys::map::Context::List | crate::keys::map::Context::Sidebar
    ) && app.help_scroll.is_none()
    {
        "zc "
    } else {
        ""
    };
    chip(
        format!("{key}{}", app.state.list_view().label()),
        TEXT,
        SURFACE0,
    )
}

pub(super) struct ListSpec<'a> {
    pub(super) posts: &'a [&'a Post],
    pub(super) selected: usize,
    /// The search preview lists posts without choosing one.
    pub(super) show_selection: bool,
    pub(super) show_subreddit: bool,
    pub(super) offset: usize,
    pub(super) scroll: CardScroll,
    pub(super) append_area: Option<Rect>,
}

pub(super) fn draw_card_list(
    f: &mut Frame,
    app: &App,
    area: Rect,
    spec: &mut ListSpec<'_>,
    pictures: &mut ListPictures,
) -> Option<usize> {
    if area.is_empty() || spec.posts.is_empty() {
        return None;
    }
    let width = area.width.saturating_sub(2) as usize;
    let show_selection = spec.show_selection;
    let card_at = |i, selected| {
        expanded_card(
            app,
            spec.posts[i],
            width,
            &CardOpts {
                selected: show_selection && i == selected,

                show_subreddit: spec.show_subreddit,
                reserve: 0,
            },
            area.height,
        )
    };
    let scroll = &mut spec.scroll;
    let offset = &mut spec.offset;
    let pending = std::mem::take(&mut scroll.pending);
    let mut anchor = scroll
        .post
        .as_ref()
        .and_then(|id| spec.posts.iter().position(|post| &post.id == id));
    let height = |i| card_at(i, spec.selected).height();
    let (first_visible, last_visible) = loop {
        if let Some(index) = anchor {
            *offset = index;
        } else {
            scroll.line = 0;
            *offset = (*offset).min(spec.selected);
            if height(spec.selected) > usize::from(area.height) {
                *offset = spec.selected;
            }
            let mut used = 0;
            for i in (*offset..=spec.selected).rev() {
                used += height(i) + 1;
                if used > usize::from(area.height) {
                    *offset = (i + 1).min(spec.selected);
                    break;
                }
            }
        }
        scroll.viewport = area.height as usize;
        let anchor_height = height(*offset);
        scroll.line = scroll.line.min(if scroll.height != anchor_height {
            anchor_height.saturating_sub(1)
        } else {
            anchor_height
        });
        let mut delta = pending;
        if pending == 0
            && spec.selected.abs_diff(*offset) <= scroll.viewport
            && let Some((id, row, fitted)) = &scroll.selected
            && (0..scroll.viewport as isize).contains(row)
            && spec.posts[spec.selected].id == *id
        {
            // A card that fitted and no longer fits below its row moves up until it does.
            let fits = scroll.viewport as isize - height(spec.selected) as isize;
            let target = if *fitted {
                (*row).min(fits.max(0))
            } else {
                *row
            };
            delta = card_row(*offset, scroll.line, spec.selected, height) - target;
        }
        (*offset, scroll.line) =
            shift_card_rows(*offset, scroll.line, delta, spec.posts.len(), height);
        let (available, _) = card_viewport_end(
            *offset,
            scroll.line,
            scroll.viewport,
            spec.posts.len(),
            height,
        );
        if available < scroll.viewport {
            let last = spec.posts.len() - 1;
            (*offset, scroll.line) = shift_card_rows(
                last,
                height(last),
                -(scroll.viewport as isize),
                spec.posts.len(),
                height,
            );
        }
        let (_, last_visible) = card_viewport_end(
            *offset,
            scroll.line,
            scroll.viewport,
            spec.posts.len(),
            height,
        );
        let first_visible =
            (*offset + usize::from(scroll.line == height(*offset))).min(last_visible);
        // Only scrolling moves the selection; data, picture and size changes move the view
        // to the selected post instead.
        if anchor.is_some()
            && pending == 0
            && !(first_visible..=last_visible).contains(&spec.selected)
        {
            anchor = None;
            continue;
        }
        break (first_visible, last_visible);
    };
    spec.selected = spec.selected.clamp(first_visible, last_visible);
    scroll.height = card_at(*offset, spec.selected).height();
    scroll.post = Some(spec.posts[*offset].id.clone());
    let selected_height = card_at(spec.selected, spec.selected).height() as isize;
    let row = card_row(*offset, scroll.line, spec.selected, |i| {
        card_at(i, spec.selected).height()
    });
    scroll.selected = Some((
        spec.posts[spec.selected].id.clone(),
        row,
        row >= 0 && row + selected_height <= scroll.viewport as isize,
    ));
    let mut y = area.y;
    let mut shown = 0;
    for i in *offset..spec.posts.len() {
        let card = card_at(i, spec.selected);
        let skip = if i == *offset { scroll.line } else { 0 };
        let rows = card.height() - skip;
        let at = Rect::new(
            area.x,
            y,
            area.width.saturating_sub(2),
            rows.min(usize::from(area.bottom() - y)) as u16,
        );
        f.render_widget(
            Paragraph::new(card.visible(skip, usize::from(at.height))),
            at,
        );
        if card.image_size.height > 0 {
            let image_start = usize::from(card.image_at.y);
            let image_y = y + image_start.saturating_sub(skip).min(area.height as usize) as u16;
            let skipped = skip.saturating_sub(image_start) as u16;
            if skipped < card.image_size.height && image_y < at.bottom() {
                pictures.cards.push(CardPicture {
                    url: card.image.clone(),
                    area: Rect::new(
                        area.x + card.image_at.x,
                        image_y,
                        card.image_size.width,
                        (card.image_size.height - skipped).min(at.bottom() - image_y),
                    ),
                    size: card.image_size,
                    skipped,
                    pixels: card.pixels,
                    indicator: match &spec.posts[i].media {
                        Media::Video { gif, url, .. } => Some(Indicator::Video {
                            gif: *gif,
                            play: url.is_some(),
                        }),
                        Media::Image { .. } if spec.posts[i].media.video_url().is_some() => {
                            Some(Indicator::Video {
                                gif: true,
                                play: true,
                            })
                        }
                        Media::Link { .. } if spec.posts[i].media.youtube_url().is_some() => {
                            Some(Indicator::Video {
                                gif: false,
                                play: true,
                            })
                        }
                        Media::Link { .. } => Some(Indicator::Link),
                        Media::Gallery { images, .. } if !images.is_empty() => {
                            Some(Indicator::Gallery(images.len()))
                        }
                        _ => None,
                    },
                    selected: spec.show_selection && i == spec.selected,
                    detail: false,
                    post_id: Some(spec.posts[i].id.clone()),
                    video_url: spec.posts[i].media.video_url().map(str::to_string),
                    video_frame: false,
                });
            }
        }
        shown += 1;
        if i + 1 == spec.posts.len()
            && usize::from(at.height) == rows
            && at.bottom() < area.bottom()
        {
            let tail_area = Rect::new(
                area.x,
                at.bottom(),
                area.width.saturating_sub(2),
                area.bottom() - at.bottom(),
            );
            spec.append_area = Some(tail_area);
        }
        y = y.saturating_add(at.height.saturating_add(1));
        if y >= area.bottom() {
            break;
        }
    }
    if scroll.height > scroll.viewport {
        draw_scrollbar(
            f,
            area,
            scroll
                .line
                .min(scroll.height.saturating_sub(scroll.viewport)),
            scroll.viewport,
            scroll.height,
        );
    } else {
        draw_scrollbar(f, area, *offset, shown, spec.posts.len());
    }
    Some(last_visible)
}

pub(super) fn draw_pagination_error(f: &mut Frame, mut area: Rect, label: &str) {
    if area.is_empty() {
        return;
    }
    if area.height > 1 {
        area.y += 1;
        area.height -= 1;
    }
    let mut panel = Block::default().style(Style::new().bg(MANTLE));
    if area.height >= 4 && area.width >= 8 {
        panel = panel
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(SURFACE1));
    }
    let inner = panel.inner(area);
    f.render_widget(panel, area);
    centered(
        f,
        inner,
        vec![Line::styled(label.to_string(), Style::new().fg(RED).bold())],
    );
}

/// Scrolling list of post cards separated by a blank line.
pub(super) fn draw_post_list(
    f: &mut Frame,
    app: &App,
    area: Rect,
    spec: &mut ListSpec<'_>,
    pictures: &mut ListPictures,
) -> Option<usize> {
    if app.state.list_view() == ListView::Card && area.height >= 3 {
        return draw_card_list(f, app, area, spec, pictures);
    }
    spec.scroll.viewport = area.height as usize;
    const ITEM: usize = 3;
    let offset = &mut spec.offset;
    let fits = ((area.height as usize + 1) / ITEM).max(1);
    if spec.selected < *offset {
        *offset = spec.selected;
    } else if spec.selected >= *offset + fits {
        *offset = spec.selected + 1 - fits;
    }
    let shown = spec
        .posts
        .len()
        .saturating_sub(*offset)
        .min((area.height as usize).div_ceil(ITEM));
    let width = area.width.saturating_sub(2) as usize;
    let reserve = thumb_reserve(app, spec.posts.iter().copied());
    let mut lines = Vec::new();
    let mut tail_y = None;
    for (i, post) in spec.posts.iter().enumerate().skip(*offset).take(shown) {
        let y = area.y + lines.len() as u16;
        if let Some(url) = row_thumb(app, post)
            && y + THUMB_ROWS <= area.bottom()
        {
            pictures.thumbs.push(ImageSpot {
                url: url.to_string(),
                area: Rect {
                    x: area.x + 1,
                    y,
                    width: THUMB_COLS,
                    height: THUMB_ROWS,
                },
            });
        }
        let opts = CardOpts {
            selected: spec.show_selection && i == spec.selected,

            show_subreddit: spec.show_subreddit,
            reserve,
        };
        let [title, meta] = post_card(app, post, width, &opts);
        lines.push(title);
        lines.push(meta);
        lines.push(Line::raw(""));
        if i + 1 == spec.posts.len() {
            tail_y = Some(y.saturating_add(2));
        }
    }
    f.render_widget(Paragraph::new(lines), area);
    if let Some(y) = tail_y.filter(|y| *y < area.bottom()) {
        let tail_area = Rect::new(area.x, y, area.width.saturating_sub(2), area.bottom() - y);
        spec.append_area = Some(tail_area);
    }
    draw_scrollbar(f, area, *offset, shown, spec.posts.len());
    (shown > 0).then(|| *offset + shown - 1)
}

pub(super) fn draw_scrollbar(
    f: &mut Frame,
    area: Rect,
    offset: usize,
    visible: usize,
    total: usize,
) {
    if total <= visible || area.height < 3 {
        return;
    }
    let h = area.height as usize;
    let thumb = (h * visible / total).max(1);
    let top = (h - thumb) * offset / (total - visible).max(1);
    let x = area.x + area.width.saturating_sub(1);
    for y in 0..h {
        let (sym, color) = if y >= top && y < top + thumb {
            ("┃", OVERLAY)
        } else {
            ("│", SURFACE0)
        };
        f.render_widget(
            Span::styled(sym, Style::new().fg(color)),
            Rect {
                x,
                y: area.y + y as u16,
                width: 1,
                height: 1,
            },
        );
    }
}

pub(super) fn new_posts_chip(app: &App) -> Span<'static> {
    let label = if app.post_focused() {
        "New posts".to_string()
    } else {
        format!("New posts · {} refresh", search::reading_key(app, "r"))
    };
    chip(label, ACCENT, SURFACE0)
}
