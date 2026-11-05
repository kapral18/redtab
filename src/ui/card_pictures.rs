use super::*;

/// A thumbnail or icon to draw once the text is down.
pub(super) struct ImageSpot {
    pub(super) url: String,
    pub(super) area: Rect,
}

#[derive(Clone)]
pub(super) struct CardPicture {
    pub(super) url: Option<String>,
    pub(super) area: Rect,
    pub(super) size: Size,
    pub(super) skipped: u16,
    pub(super) pixels: (u32, u32),
    pub(super) indicator: Option<Indicator>,
    pub(super) selected: bool,
    pub(super) detail: bool,
    pub(super) post_id: Option<String>,
    pub(super) video_url: Option<String>,
    pub(super) video_frame: bool,
}

impl CardPicture {
    pub(super) fn overlay(&self) -> Option<ImageOverlay> {
        self.indicator.clone().map(|indicator| ImageOverlay {
            indicator,
            top: 0,
            rows: self.size.height,
        })
    }

    pub(super) fn status(&self, label: &str) -> String {
        let prefix = match &self.indicator {
            Some(Indicator::Video { gif: true, .. }) => "▶ GIF ·",
            Some(Indicator::Video { .. }) => "▶ video ·",
            Some(Indicator::Link) => "↗ link ·",
            Some(Indicator::Gallery(_)) => "▦ gallery ·",
            None => "▣",
        };
        format!("{prefix} {label}")
    }
}

#[derive(Default)]
pub(super) struct ListPictures {
    pub(super) thumbs: Vec<ImageSpot>,
    pub(super) cards: Vec<CardPicture>,
    pub(super) inline: Vec<ImageSpot>,
}

/// Draw loaded thumbnails and icons; start loading the missing ones.
pub(super) fn draw_spots(f: &mut Frame, app: &mut App, spots: &[ImageSpot], covered: Option<Rect>) {
    for spot in spots {
        if covered.is_some_and(|area| area.intersects(spot.area)) {
            continue;
        }
        if let Some(ImageSlot::Ready { protocol, .. }) = app.thumbs.get_mut(&spot.url) {
            let fill = StatefulImage::default()
                .resize(Resize::Scale(Some(ratatui_image::FilterType::Triangle)));
            f.render_stateful_widget(fill, spot.area, protocol.as_mut());
        } else if !matches!(app.thumbs.get(&spot.url), Some(ImageSlot::Failed(_))) {
            loading::draw(f, spot.area, loading::Kind::Media, "", app.loading_frame);
        }
    }
    let urls: Vec<&str> = spots.iter().map(|s| s.url.as_str()).collect();
    app.thumbs_drawn(&urls);
}

pub(super) fn row_thumb<'a>(app: &App, post: &'a Post) -> Option<std::borrow::Cow<'a, str>> {
    // A spoiler's picture is the spoiler.
    if !app.thumbnails_on() || post.spoiler {
        return None;
    }
    post.media
        .list_thumb()
        .filter(|url| app.usable_thumb(Some(url.as_ref())).is_some())
}

/// Thumbnail slot width for a list: every row gets it if any non-spoiler post has a
/// thumbnail, so columns align.
pub(super) fn thumb_reserve<'a>(app: &App, mut posts: impl Iterator<Item = &'a Post>) -> usize {
    if app.thumbnails_on() && posts.any(|p| !p.spoiler && p.media.list_thumb().is_some()) {
        THUMB_RESERVE
    } else {
        0
    }
}

pub(super) fn draw_card_pictures(
    f: &mut Frame,
    app: &mut App,
    spots: &[CardPicture],
    covered: Option<Rect>,
) {
    let prepare = app.card_preparation_allowed();
    app.card_video_drawn(
        spots
            .iter()
            .find(|spot| spot.selected)
            .map(|spot| spot.size),
    );
    let mut spots = spots.to_vec();
    let mut loading_rows = Vec::new();
    if let Some(label) = app.video_loading() {
        for spot in spots.iter_mut().filter(|spot| {
            spot.selected && (spot.detail || app.post_view.is_none()) && !spot.area.is_empty()
        }) {
            loading_rows.push((
                Rect::new(spot.area.x, spot.area.bottom() - 1, spot.area.width, 1),
                label,
            ));
            spot.area.height -= 1;
        }
    }
    let urls: Vec<&str> = spots
        .iter()
        .filter_map(|spot| {
            spot.url
                .as_deref()
                .filter(|url| !spot.selected || !app.video_uses_playback(url))
        })
        .collect();
    app.card_pictures_drawn(&urls);
    app.card_renderer.begin();
    for spot in &spots {
        if spot.area.is_empty() {
            continue;
        }
        let preview = if spot.selected {
            app.video_preview(!spot.detail)
        } else {
            spot.post_id
                .as_deref()
                .zip(spot.video_url.as_deref())
                .and_then(|(post, source)| app.inactive_card_frame(post, source))
                .map(|frame| (frame, false))
        };
        if let Some((
            video::Frame {
                pixels: video::Pixels::Raw(image),
                ..
            },
            playing,
        )) = preview
        {
            let image = image.clone();
            let mut spot = spot.clone();
            spot.video_frame = true;
            if playing && let Some(Indicator::Video { play, .. }) = &mut spot.indicator {
                *play = false;
            }
            if let Err(error) = draw_card_image(
                f,
                &image,
                &mut None,
                &spot,
                CardRender {
                    picker: &app.image_picker,
                    renderer: &mut app.card_renderer,
                    prepare: Preparation::Refresh,
                    fingerprint: None,
                    covered,
                    phase: app.loading_frame,
                },
            ) {
                draw_visible_status(
                    f,
                    spot.area,
                    covered,
                    &format!("▶ video unavailable: {error}"),
                );
            }
            continue;
        }
        let Some(url) = spot
            .url
            .as_deref()
            .filter(|url| !spot.selected || !app.video_uses_playback(url))
        else {
            let label = if !spot.selected {
                spot.status("select to preview")
            } else if !spot.detail && app.post_view.is_some() {
                spot.status("open to preview")
            } else if !app.video_controls_available() && !app.config.media.video_autoplay {
                spot.status("l return to preview")
            } else if app
                .selected_video()
                .is_some_and(|player| player.error().is_some())
            {
                spot.status("unavailable · O opens media")
            } else if app.video_loading().is_some() {
                continue;
            } else {
                spot.status("p play/pause")
            };
            draw_visible_status(f, spot.area, covered, &label);
            continue;
        };
        let preparation = if !prepare {
            Preparation::Hold
        } else if app.card_image_is_retained(url) {
            Preparation::Missing
        } else {
            Preparation::Refresh
        };
        let Some(slot) = app.card_images.get_mut(url) else {
            draw_visible_loading(
                f,
                spot.area,
                covered,
                &spot.status("loading image…"),
                app.loading_frame,
            );
            continue;
        };
        match slot {
            CardImageSlot::Ready { image, protocol }
            | CardImageSlot::Cached {
                image, protocol, ..
            } => {
                if let Err(error) = draw_card_image(
                    f,
                    image,
                    protocol,
                    spot,
                    CardRender {
                        picker: &app.image_picker,
                        renderer: &mut app.card_renderer,
                        prepare: preparation,
                        fingerprint: app
                            .card_image_metadata
                            .get(url)
                            .map(|metadata| metadata.fingerprint),
                        covered,
                        phase: app.loading_frame,
                    },
                ) {
                    draw_visible_status(
                        f,
                        spot.area,
                        covered,
                        &spot.status(&format!("image unavailable: {error}")),
                    );
                    *slot = CardImageSlot::Failed(error);
                }
            }
            CardImageSlot::Loading => draw_visible_loading(
                f,
                spot.area,
                covered,
                &spot.status("loading image…"),
                app.loading_frame,
            ),
            CardImageSlot::Failed(error) => draw_visible_status(
                f,
                spot.area,
                covered,
                &spot.status(&format!("image unavailable: {error}")),
            ),
        }
    }
    app.card_renderer.finish();
    for (area, label) in loading_rows {
        draw_visible_loading(f, area, covered, label, app.loading_frame);
    }
}

pub(super) struct CardRender<'a> {
    pub(super) picker: &'a ratatui_image::picker::Picker,
    pub(super) renderer: &'a mut crate::card_image::Renderer,
    pub(super) prepare: Preparation,
    pub(super) fingerprint: Option<[u8; 32]>,
    pub(super) covered: Option<Rect>,
    pub(super) phase: usize,
}

pub(super) fn draw_card_image(
    f: &mut Frame,
    image: &std::sync::Arc<image::DynamicImage>,
    protocol: &mut Option<CardImageProtocol>,
    spot: &CardPicture,
    render: CardRender<'_>,
) -> Result<(), String> {
    let CardRender {
        picker,
        renderer,
        prepare,
        fingerprint,
        covered,
        phase,
    } = render;
    if protocol.as_ref().is_none_or(|protocol| {
        protocol.size != spot.size
            || protocol.pixels != spot.pixels
            || protocol.overlay != spot.overlay()
    }) {
        *protocol = Some(CardImageProtocol {
            size: spot.size,
            pixels: spot.pixels,
            overlay: spot.overlay(),
            tiles: HashMap::new(),
        });
    }
    let Some(protocol) = protocol else {
        return Ok(());
    };
    let native = picker.protocol_type() == ratatui_image::picker::ProtocolType::Kitty;
    let request_key = |top, rows, left, cols| {
        let mut request = CardImageKey::new(
            spot.url.as_deref().unwrap_or("card-video"),
            picker,
            spot.size,
            spot.pixels,
            top,
            rows,
            left / TILE_COLS,
        );
        request.left = left;
        request.cols = cols;
        request.overlay = spot.overlay();
        request.post_id = spot.video_frame.then(|| spot.post_id.clone()).flatten();
        request.video = spot.video_frame;
        request.fingerprint = (!spot.video_frame).then_some(fingerprint).flatten();
        request
    };
    let preupload_at = visible_rects(spot.area, covered)
        .first()
        .map(|area| ratatui::layout::Position::new(area.x, area.y));
    let mut prepared = Vec::new();
    // A fitted still is bounded by the viewport. Finish its canvas while any part
    // is visible so scrolling into the other strips does not restart preparation.
    if native && !spot.video_frame && preupload_at.is_some() {
        for top in (0..spot.size.height).step_by(usize::from(TILE_ROWS)) {
            for left in (0..spot.size.width).step_by(usize::from(TILE_COLS)) {
                let request = request_key(
                    top,
                    TILE_ROWS.min(spot.size.height - top),
                    left,
                    TILE_COLS.min(spot.size.width - left),
                );
                if let Some(Ok(tile)) = renderer.tile(request, image, picker, prepare) {
                    prepared.push(tile);
                }
            }
        }
    }
    let first = spot.skipped / TILE_ROWS;
    let end = (spot.skipped + spot.area.height).div_ceil(TILE_ROWS);
    let mut visible = Vec::new();
    for row in first..end {
        for column in 0..spot.size.width.div_ceil(TILE_COLS) {
            let start = spot.skipped.max(row * TILE_ROWS);
            let y = spot.area.y + start - spot.skipped;
            let rows = ((row + 1) * TILE_ROWS - start).min(spot.area.bottom() - y);
            let cols = TILE_COLS.min(spot.size.width - column * TILE_COLS);
            let tile_area = Rect::new(spot.area.x + column * TILE_COLS, y, cols, rows);
            for area in visible_rects(tile_area, covered) {
                let top = start + area.y - y;
                let left = column * TILE_COLS + area.x - tile_area.x;
                let key = (top, left, area.height, area.width);
                visible.push(key);
                let source_top = if native { row * TILE_ROWS } else { top };
                let source_left = if native { column * TILE_COLS } else { left };
                let request = request_key(
                    source_top,
                    if native {
                        TILE_ROWS.min(spot.size.height - source_top)
                    } else {
                        area.height
                    },
                    source_left,
                    if native { cols } else { area.width },
                );
                if let Some(result) = renderer.tile(request, image, picker, prepare) {
                    protocol.tiles.insert(key, result?);
                }
                if let Some(encoded) = protocol.tiles.get(&key) {
                    encoded.render_region(
                        area,
                        ratatui::layout::Position::new(left - source_left, top - source_top),
                        f.buffer_mut(),
                    );
                } else {
                    loading::draw(
                        f,
                        area,
                        loading::Kind::Media,
                        &spot.status("preparing image…"),
                        phase,
                    );
                }
            }
        }
    }
    if let Some(position) = preupload_at {
        let mut uploads = String::new();
        for tile in prepared {
            if let Some(upload) = tile.take_upload() {
                uploads.push_str(&upload);
            }
        }
        if !uploads.is_empty()
            && let Some(cell) = f.buffer_mut().cell_mut(position)
        {
            uploads.push_str(cell.symbol());
            cell.set_symbol(&uploads).set_diff_option(
                ratatui::buffer::CellDiffOption::ForcedWidth(std::num::NonZeroU16::MIN),
            );
        }
    }
    protocol.tiles.retain(|key, _| visible.contains(key));
    Ok(())
}

/// The uncovered top, bottom, left and right strips retain their source coordinates.
pub(super) fn visible_rects(area: Rect, covered: Option<Rect>) -> Vec<Rect> {
    let Some(covered) = covered.filter(|covered| area.intersects(*covered)) else {
        return (!area.is_empty()).then_some(area).into_iter().collect();
    };
    let cut = area.intersection(covered);
    [
        Rect::new(area.x, area.y, area.width, cut.y - area.y),
        Rect::new(
            area.x,
            cut.bottom(),
            area.width,
            area.bottom() - cut.bottom(),
        ),
        Rect::new(area.x, cut.y, cut.x - area.x, cut.height),
        Rect::new(cut.right(), cut.y, area.right() - cut.right(), cut.height),
    ]
    .into_iter()
    .filter(|rect| !rect.is_empty())
    .collect()
}

pub(super) fn draw_visible_status(f: &mut Frame, area: Rect, covered: Option<Rect>, label: &str) {
    for visible in visible_rects(area, covered) {
        draw_picture_status(f, visible, label);
    }
}

pub(super) fn draw_visible_loading(
    f: &mut Frame,
    area: Rect,
    covered: Option<Rect>,
    label: &str,
    phase: usize,
) {
    for visible in visible_rects(area, covered) {
        loading::draw(f, visible, loading::Kind::Media, label, phase);
    }
}

pub(super) fn draw_picture_status(f: &mut Frame, area: Rect, label: &str) {
    if area.is_empty() {
        return;
    }
    let lines: Vec<Line> = textwrap::wrap(label, usize::from(area.width))
        .into_iter()
        .take(usize::from(area.height))
        .map(|line| Line::raw(line.into_owned()))
        .collect();
    let rows = lines.len() as u16;
    f.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(Style::new().fg(SUBTEXT)),
        Rect::new(area.x, area.y + (area.height - rows) / 2, area.width, rows),
    );
}
