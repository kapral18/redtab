use super::*;

pub(super) struct FormLayout {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) caret: usize,
    pub(super) label: std::ops::Range<usize>,
}

pub(super) fn form_layout(form: &Form, width: usize) -> FormLayout {
    let mut lines = Vec::new();
    let mut active_line = 0;
    let mut active_label = 0..0;
    for (index, field) in form.fields.iter().enumerate() {
        let active = index == form.active;
        let label = format!(
            "{} {}{}",
            if active { "▌" } else { " " },
            field.label,
            if field.required { "" } else { " (optional)" }
        );
        let label_start = lines.len();
        wrap_into(
            &mut lines,
            &label,
            width,
            &[],
            Style::new().fg(if active { ACCENT } else { SUBTEXT }),
        );
        if active {
            active_label = label_start..lines.len();
        }
        let prefix = [Span::raw("  ")];
        let value_width = width.saturating_sub(2).max(1);
        let style = Style::new().fg(TEXT).bg(SURFACE0);
        let start = lines.len();
        let caret_ordinal = clean(&field.value[..field.cursor]).matches('▏').count();
        let mut markers = 0;
        // Reserve the caret cell in every field so Tab cannot reflow the form.
        let text = format!(
            "{}▏{}",
            clean(&field.value[..field.cursor]),
            clean(&field.value[field.cursor..])
        );
        wrap_into(&mut lines, &text, value_width, &prefix, style);
        for (row, line) in lines.iter_mut().enumerate().skip(start) {
            for span in &mut line.spans {
                let mut caret_at = None;
                for (at, _) in span.content.match_indices('▏') {
                    if markers == caret_ordinal {
                        if active {
                            active_line = row;
                        } else {
                            caret_at = Some(at);
                        }
                    }
                    markers += 1;
                }
                if let Some(at) = caret_at {
                    span.content
                        .to_mut()
                        .replace_range(at..at + '▏'.len_utf8(), "");
                }
            }
            pad_to(&mut line.spans, width, style);
        }
        if index + 1 < form.fields.len() {
            lines.push(Line::default());
        }
    }
    FormLayout {
        lines,
        caret: active_line,
        label: active_label,
    }
}

pub(super) fn draw_form(f: &mut Frame, form: &Form, area: Rect) {
    let FormLayout {
        lines,
        caret,
        label,
    } = form_layout(form, area.width as usize);
    let scroll = caret.saturating_sub(area.height.saturating_sub(1) as usize);
    if scroll > label.start && area.height >= 2 {
        let label_rows = label.len().min(area.height.saturating_sub(1) as usize) as u16;
        let [heading, value] =
            Layout::vertical([Constraint::Length(label_rows), Constraint::Min(1)]).areas(area);
        let labels: Vec<_> = lines
            .iter()
            .skip(label.start)
            .take(label_rows as usize)
            .cloned()
            .collect();
        f.render_widget(Paragraph::new(labels), heading);
        let offset = caret
            .saturating_sub(value.height.saturating_sub(1) as usize)
            .max(label.end);
        f.render_widget(
            Paragraph::new(lines.into_iter().skip(offset).collect::<Vec<_>>()),
            value,
        );
    } else {
        f.render_widget(
            Paragraph::new(lines).scroll((scroll.min(u16::MAX as usize) as u16, 0)),
            area,
        );
    }
}
