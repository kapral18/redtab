use super::*;

pub(super) fn menu_rows(
    choices: &[crate::app::experience::Choice],
    selected: usize,
    width: usize,
) -> (Vec<Line<'static>>, std::ops::Range<usize>) {
    let mut lines = Vec::new();
    let mut section = "";
    let mut selected_rows = 0..0;
    for (index, choice) in choices.iter().enumerate() {
        if choice.section != section {
            section = choice.section;
            lines.push(Line::styled(
                section,
                Style::new()
                    .fg(if section == "Destructive" {
                        RED
                    } else {
                        OVERLAY
                    })
                    .bold(),
            ));
        }
        let first_row = lines.len();
        let active = index == selected;
        let style = choice_style(active);
        let label = if choice.shortcut.is_empty() {
            choice.label.clone()
        } else {
            format!(
                "[{}] {}",
                crate::keys::map::label(choice.shortcut),
                choice.label
            )
        };
        for (part, text) in textwrap::wrap(&clean(&label), width.saturating_sub(2).max(1))
            .iter()
            .enumerate()
        {
            let mut spans = vec![Span::styled(
                format!("{} {text}", if active && part == 0 { "▌" } else { " " }),
                style,
            )];
            pad_to(&mut spans, width, style);
            lines.push(Line::from(spans));
        }
        if index == selected {
            selected_rows = first_row..lines.len();
        }
    }
    (lines, selected_rows)
}
