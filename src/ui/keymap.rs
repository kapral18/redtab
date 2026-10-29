use super::*;

pub(super) fn draw(f: &mut Frame, app: &mut App) -> Option<Rect> {
    if app.keymap.prefix.is_empty() {
        return None;
    }
    let entries = crate::keys::map::hints(app);
    let available = popup(f.area(), 82, u16::MAX);
    let prefix = crate::keys::map::label(&app.keymap.prefix);
    let title = format!(
        "{} · {prefix} · Esc cancel",
        crate::keys::map::prefix_name(&app.keymap.prefix)
    );
    let title = fitting_label(
        &[&title, "Keys · Esc cancel", "Keys Esc", "Esc", "⎋"],
        available.width.saturating_sub(4) as usize,
    );
    let block = popup_block(title, available.width);
    let width = block.inner(available).width as usize;
    let mut lines = help::lines(entries, width);
    lines.push(Line::styled(
        "PgUp/PgDn scroll · Esc cancels",
        Style::new().fg(OVERLAY),
    ));
    let area = popup(f.area(), 82, lines.len().saturating_add(2).min(24) as u16);
    let inner = block.inner(area);
    app.keymap.hint_rows = inner.height as usize;
    app.keymap.scroll = app
        .keymap
        .scroll
        .min(lines.len().saturating_sub(inner.height as usize));
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(
            lines
                .into_iter()
                .skip(app.keymap.scroll)
                .collect::<Vec<_>>(),
        ),
        inner,
    );
    Some(area)
}
