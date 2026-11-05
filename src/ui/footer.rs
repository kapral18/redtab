use super::*;

pub(super) fn feed_activity(
    feed: &crate::app::Feed,
    can_load: bool,
) -> Option<(&'static str, &'static str)> {
    if feed.loading_more() || feed.receiving() {
        Some(("Loading more posts…", "Loading more posts…"))
    } else if feed.loading() || (can_load && !feed.loaded && feed.error().is_none()) {
        Some(if feed.posts.is_empty() || !feed.shows_current_query() {
            ("Loading posts…", "posts")
        } else {
            ("Refreshing posts…", "refresh")
        })
    } else if feed.refreshing() {
        Some(("Refreshing posts…", "refresh"))
    } else {
        None
    }
}

#[derive(Default)]
pub(super) struct VisibleLoaders {
    pub(super) list: bool,
    pub(super) post: bool,
}

pub(super) fn loading_activity(app: &App, loaders: &VisibleLoaders) -> Option<&'static str> {
    if app.media_loading() {
        return Some("media");
    }
    if app.video_loading().is_some() {
        return Some("video");
    }
    let post = app
        .post_view
        .as_ref()
        .filter(|_| !loaders.post)
        .and_then(|view| {
            if view.loading || (view.page.is_none() && view.error.is_none()) {
                Some(if view.page.is_some() {
                    "refresh"
                } else {
                    "post"
                })
            } else {
                (!view.loading_more.is_empty()).then_some("replies")
            }
        });
    let list = if !loaders.list && (app.post_view.is_none() || app.detail_fits()) {
        match app.active_tab().kind {
            TabKind::Home | TabKind::Subreddit { .. } | TabKind::User { .. } => {
                feed_activity(&app.active_tab().feed, !app.reddit.needs_sign_in())
                    .map(|(_, activity)| activity)
            }
            TabKind::Search if !app.search.shows_rows() => {
                feed_activity(&app.active_tab().feed, !app.reddit.needs_sign_in())
                    .map(|(_, activity)| activity)
            }
            TabKind::AllSubs if app.all_subs.loading => Some("subs"),
            TabKind::AllSubs if app.all_subs.groups.iter().any(|group| group.loading) => {
                Some("communities")
            }
            _ => None,
        }
    } else {
        None
    };
    let active = if app.post_focused() {
        post.or(list)
    } else {
        list.or(post)
    };
    active.or_else(|| app.images_loading().then_some("images"))
}

pub(super) fn draw_footer(
    f: &mut Frame,
    app: &App,
    area: Rect,
    loaders: &VisibleLoaders,
    panes: &[(Rect, &'static str, bool)],
) {
    let progress = app.membership_progress();
    let cooldown = app
        .reddit
        .pause()
        .map(|pause| format!("Reddit paused {}s · {}", pause.seconds, pause.cause));
    let playback_error = app
        .selected_video()
        .filter(|_| app.video_controls_available())
        .and_then(|player| player.error());
    let feedback = app.status.is_some()
        || progress.is_some()
        || playback_error.is_some()
        || matches!(
            app.health,
            Health::SignedOut | Health::Down | Health::Paused
        );
    let help_width = if playback_error.is_some() && area.width < 40 {
        area.width.min(2)
    } else if area.width >= 180
        || (area.width >= 64
            && app.settings.store.persistence == crate::config::store::Persistence::Unsaved)
    {
        26
    } else if area.width >= 20 {
        10
    } else {
        2.min(area.width)
    };
    let available = usize::from(area.width.saturating_sub(help_width));
    // Reserve progress space even while idle, so hints never reflow as work changes.
    // Errors and connection/membership feedback take the full content width.
    let activity_width = if !feedback || app.status.as_ref().is_some_and(|status| !status.error) {
        (available / 3).min(24)
    } else {
        0
    };
    let content_width = available.saturating_sub(activity_width);
    let mut line = if let Some(status) = &app.status {
        let (icon, color) = if status.error {
            ("✗", RED)
        } else {
            if app.media_loading() {
                (loading::glyph(app.loading_frame), YELLOW)
            } else {
                ("✓", TEAL)
            }
        };
        Line::from(vec![
            Span::styled(
                format!(" {icon} "),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if status.error
                    && (status.text.contains("Reddit rate limit reached")
                        || status.text.contains("Reddit requests paused"))
                {
                    cooldown.clone().unwrap_or_else(|| status.text.clone())
                } else {
                    status.text.clone()
                },
                Style::new().fg(color),
            ),
        ])
    } else if let Some(progress) = progress {
        Line::from(vec![Span::styled(
            format!(" {} {progress}", loading::glyph(app.loading_frame)),
            loading::style(loading::Kind::Action),
        )])
    } else if let Some(error) = playback_error {
        let handoff = if app
            .current_post()
            .is_some_and(|post| post.media.youtube_url().is_some())
        {
            "O browser"
        } else {
            "O media"
        };
        let label = if content_width < 28 {
            "✗ p retry".into()
        } else {
            format!("✗ p retry · {error} · {handoff}")
        };
        Line::from(Span::styled(label, Style::new().fg(RED)))
    } else if app.health == Health::SignedOut {
        Line::from(Span::styled(
            format!(
                " Reddit sign-in required · {} connect",
                search::reading_key(app, "SPC p a")
            ),
            Style::new().fg(YELLOW),
        ))
    } else if app.health == Health::Down {
        Line::from(vec![
            Span::styled(" ✗ ", Style::new().fg(RED)),
            Span::styled("Reddit is not answering", Style::new().fg(RED)),
        ])
    } else if app.health == Health::Paused {
        Line::from(vec![
            Span::styled(" ◐ ", Style::new().fg(YELLOW)),
            Span::styled(
                cooldown.unwrap_or_else(|| "Reddit requests paused".into()),
                Style::new().fg(YELLOW),
            ),
        ])
    } else {
        let owned = crate::keys::footer_hints(app);
        let hints: Vec<_> = owned
            .iter()
            .map(|(key, label)| (key.as_str(), label.as_str()))
            .collect();
        // Keep whole key + label pairs; do not build pairs past the available width.
        let mut fitted = vec![Span::raw(" ")];
        let mut used = 1;
        for (key, label) in &hints {
            let pair = [
                key_chip(key),
                Span::styled(format!(" {label}  "), Style::new().fg(OVERLAY)),
            ];
            let width = line_width(&pair);
            if used + width > content_width {
                continue;
            }
            used += width;
            fitted.extend(pair);
        }
        Line::from(fitted)
    };
    line.spans = fit_spans(line.spans, content_width);
    f.render_widget(
        line,
        Rect::new(area.x, area.y, content_width as u16, area.height),
    );
    let activity = loading_activity(app, loaders).filter(|_| activity_width > 0);
    // With two or more panes visible, the idle activity slot names the focused pane.
    if activity.is_none()
        && activity_width > 0
        && panes.len() >= 2
        && let Some(&(_, name, _)) = panes.iter().find(|pane| pane.2)
    {
        f.render_widget(
            Line::from(fit_spans(
                vec![chip(name.to_uppercase(), CRUST, ACCENT)],
                activity_width,
            ))
            .alignment(Alignment::Right),
            Rect::new(
                area.x + content_width as u16,
                area.y,
                activity_width.saturating_sub(1) as u16,
                area.height,
            ),
        );
    }
    if let Some(activity) = activity {
        let kind = match activity {
            "Loading more posts…" | "replies" => loading::Kind::More,
            "images" => loading::Kind::Media,
            "media" => loading::Kind::Action,
            "refresh" | "communities" => loading::Kind::Background,
            _ => loading::Kind::Initial,
        };
        let label = match activity {
            _ if activity_width < 7 => "",
            "Loading more posts…" if activity_width < 13 => "more",
            "Loading more posts…" if activity_width < 22 => "more posts",
            _ => activity,
        };
        let activity = Span::styled(
            if activity_width < 3 {
                loading::glyph(app.loading_frame).to_string()
            } else {
                format!(" {} {label}", loading::glyph(app.loading_frame))
            },
            loading::style(kind),
        );
        f.render_widget(
            Line::from(fit_spans(vec![activity], activity_width)),
            Rect::new(
                area.x + content_width as u16,
                area.y,
                activity_width as u16,
                area.height,
            ),
        );
    }
    let help = if help_width >= 26 {
        Line::from(vec![
            key_chip("F2"),
            Span::styled(
                if app.settings.store.persistence == crate::config::store::Persistence::Unsaved {
                    " unsaved  "
                } else {
                    " settings "
                },
                Style::new().fg(SUBTEXT),
            ),
            key_chip("F1"),
            Span::styled(" help", Style::new().fg(SUBTEXT)),
        ])
    } else if help_width >= 10 {
        Line::from(vec![
            key_chip("F1"),
            Span::styled(" help", Style::new().fg(SUBTEXT)),
        ])
    } else {
        Line::from("F1")
    };
    f.render_widget(
        help,
        Rect::new(
            area.right().saturating_sub(help_width),
            area.y,
            help_width,
            area.height,
        ),
    );
}
