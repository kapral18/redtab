use super::*;

pub(super) fn video_status(app: &App) -> Line<'static> {
    use crate::keys::map::{self, Action};
    let dim = Style::new().fg(OVERLAY);
    let mut spans = vec![Span::raw(" ")];
    let controls = map::available(app, Action::Pause);
    if !controls {
        spans.push(key_chip("l"));
        spans.push(Span::styled(" return · ", dim));
    }
    let player = app.selected_video();
    match (player, player.and_then(|player| player.error())) {
        (_, Some(e)) => spans.push(Span::styled(
            format!("✗ video unavailable: {e}"),
            Style::new().fg(RED),
        )),
        (None, _) if !app.config.media.video_autoplay && controls => {
            spans.push(key_chip("p"));
            spans.push(Span::styled(" play · autoplay off", dim));
        }
        (None, _) if !app.config.media.video_autoplay => {
            spans.push(Span::styled("autoplay off", dim))
        }
        (None, _) => spans.extend(
            loading::caption(loading::Kind::Media, "loading video…", app.loading_frame).spans,
        ),
        (Some(player), None) => {
            let at = video::clock_text(player.position(Instant::now()));
            let icon = if player.paused() { "⏸" } else { "▶" };
            let loading = app.video_loading();
            let time = if let Some(label) = loading {
                label.to_string()
            } else if let Some(total) = player.duration() {
                format!("{at} / {}", video::clock_text(total))
            } else {
                at
            };
            if loading.is_some() {
                spans
                    .extend(loading::caption(loading::Kind::Media, &time, app.loading_frame).spans);
            } else {
                spans.push(Span::styled(
                    format!("{icon} {time}"),
                    Style::new().fg(TEXT),
                ));
            }
            let wanted = app.video_sound_wanted();
            spans.push(Span::styled(
                match (player.sound(), wanted) {
                    (true, _) => "   sound on",
                    (false, true) => "   no sound",
                    (false, false) => "   sound off",
                },
                dim,
            ));
            let pause = if player.paused() { "play" } else { "pause" };
            let sound = if wanted { "mute" } else { "sound" };
            for (_, key, label) in [
                (Action::Pause, "p", pause),
                (Action::Mute, "m", sound),
                (Action::SeekForward, "H/L", "seek"),
            ]
            .into_iter()
            .filter(|(action, _, _)| map::available(app, *action))
            {
                spans.push(Span::raw("   "));
                spans.push(key_chip(key));
                spans.push(Span::styled(format!(" {label}"), dim));
            }
        }
    }
    Line::from(spans)
}

pub(super) fn draw_inline_image(
    f: &mut Frame,
    app: &mut App,
    url: &str,
    area: Rect,
    covered: Option<Rect>,
) {
    draw_image_slot(
        f,
        app.images.get_mut(url),
        app.image_picker.font_size(),
        covered.is_some_and(|popup| popup.intersects(area)),
        area,
        app.loading_frame,
    );
}

pub(super) fn draw_image_slot(
    f: &mut Frame,
    slot: Option<&mut ImageSlot>,
    font: ratatui_image::FontSize,
    covered: bool,
    area: Rect,
    phase: usize,
) {
    // Stateful protocols cannot crop arbitrary holes; only intersecting slots wait.
    if covered {
        return;
    }
    match slot {
        Some(ImageSlot::Ready {
            protocol, pixels, ..
        }) => {
            // Sized by its shape as the layout was, centred; a small one may grow.
            let size = picture_cells(
                pixels.0,
                pixels.1,
                font,
                area.width,
                area.height,
                area.height,
                MAX_ENLARGE,
            );
            let at = centred(Rect::new(area.x, area.y, size.width, size.height), area);
            let scaled = StatefulImage::default()
                .resize(Resize::Scale(Some(ratatui_image::FilterType::Triangle)));
            f.render_stateful_widget(scaled, at, protocol.as_mut());
        }
        Some(ImageSlot::Failed(e)) => {
            draw_picture_status(f, area, &format!("▣ image unavailable: {e}"))
        }
        Some(ImageSlot::Loading) | None => {
            loading::draw(f, area, loading::Kind::Media, "loading image…", phase);
        }
    }
}
