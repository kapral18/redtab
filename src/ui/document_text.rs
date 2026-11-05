use super::*;

/// Masks `>!…!<` spoilers as `▒`, one per column with spaces kept (same width, but break
/// points still differ, e.g. at hyphens or between CJK), or strips the markers once revealed.
pub(super) fn spoiler_text(text: &str, reveal: bool) -> String {
    let text = crate::model::without_link_targets(text);
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    let push = |segment: &str, masked: bool, out: &mut String| {
        if !masked || reveal {
            out.push_str(segment);
            return;
        }
        for (i, line) in segment.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            // A hidden picture masks as a short bar, not as long as its URL.
            if line.contains(PICTURE) {
                out.push_str("▒▒▒▒▒▒");
                continue;
            }
            // Whole graphemes, so an emoji sequence masks to the columns it shows in.
            for part in line.split_inclusive(crate::model::text_format) {
                let marker = part
                    .chars()
                    .last()
                    .filter(|c| crate::model::text_format(*c));
                let content = marker.map_or(part, |c| &part[..part.len() - c.len_utf8()]);
                for g in content.graphemes(true) {
                    if g.chars().all(char::is_whitespace) {
                        out.push_str(g);
                    } else {
                        out.extend(std::iter::repeat_n('▒', g.width()));
                    }
                }
                if let Some(marker) = marker {
                    out.push(marker);
                }
            }
        }
    };
    let mut rest = text.as_ref();
    while let Some(at) = rest.find([SPOILER_START, SPOILER_END]) {
        push(&rest[..at], inside, &mut out);
        let marker = rest[at..].chars().next().unwrap_or(SPOILER_END);
        inside = marker == SPOILER_START;
        rest = &rest[at + marker.len_utf8()..];
    }
    push(rest, inside, &mut out);
    out
}

/// Most rows a picture in post or comment text takes.
pub(super) const TEXT_PICTURE_ROWS: u16 = 12;

/// Text pictures kept loaded before those far from view are dropped: each holds its pixels
/// and their encoding. Dropped ones load again when reached.
pub(super) const TEXT_PICTURES_KEPT: usize = 24;

/// What the post document needs to lay out pictures in its text: the loaded images (none
/// when images are off), and the most rows one may take, so it fits the text area whole.
pub(super) struct DocumentContext<'a> {
    pub(super) focused: bool,
    pub(super) phase: usize,
    pub(super) images: Option<&'a HashMap<String, ImageSlot>>,
    pub(super) font: ratatui_image::FontSize,
    pub(super) rows: u16,
}

/// A picture in the post document: its URL, first line, the columns before it and its
/// cells once it loaded.
#[derive(Clone, Debug)]
pub(super) struct DocPicture {
    pub(super) url: String,
    pub(super) line: usize,
    pub(super) x: u16,
    pub(super) size: Option<Size>,
    pub(super) main: bool,
}

/// The URL of a picture line (`PICTURE` then the URL, maybe after a quote bar).
pub(super) fn picture_line(line: &str) -> Option<&str> {
    line.find(PICTURE)
        .map(|at| &line[at + PICTURE.len_utf8()..])
}

/// `wrap_into`, with each picture line laid out as the picture: rows for it by its shape
/// once loaded (at most `sizes.rows`), else one line saying where it stands.
pub(super) fn text_into(
    lines: &mut Vec<Line<'static>>,
    pictures: &mut Vec<DocPicture>,
    text: &str,
    width: usize,
    prefix: &[Span<'static>],
    style: Style,
    sizes: &DocumentContext<'_>,
) {
    let x = prefix.iter().map(|s| s.content.width()).sum::<usize>() as u16;
    let mut rest = text;
    while let Some(at) = rest.find(PICTURE) {
        let start = rest[..at].rfind('\n').map_or(0, |n| n + 1);
        let end = rest[at..].find('\n').map_or(rest.len(), |n| at + n);
        if start > 0 {
            wrap_into(lines, &rest[..start - 1], width, prefix, style);
        }
        let picture_text = crate::model::unformatted(&rest[start..end]);
        let url = picture_line(&picture_text).unwrap_or_default();
        // What stands before it on its line, a bullet or quote bar, stays before it: quote
        // bars on every row, the rest (a bullet) on the first.
        let lead = crate::model::unformatted(&rest[start..at]);
        let lead = lead.as_ref();
        let quoted = lead.starts_with('│');
        let lead_style = if quoted { style.fg(SUBTEXT) } else { style };
        let bars = lead.len() - lead.trim_start_matches(['│', ' ']).len();
        let lead_rest = format!("{}{}", &lead[..bars], " ".repeat(lead[bars..].width()));
        let first_prefix = [prefix, &[Span::styled(lead.to_string(), lead_style)]].concat();
        let rest_prefix = [prefix, &[Span::styled(lead_rest, lead_style)]].concat();
        let slot = sizes.images.map(|images| images.get(url));
        // A picture shown twice keeps one size, the first one, so it isn't encoded again
        // for each place every frame; where that doesn't fit it is only named.
        let first = pictures
            .iter()
            .find_map(|p| (!p.main && p.url == url).then_some(p.size).flatten());
        let cols = width.saturating_sub(lead.width()).max(1) as u16;
        let size = match (slot, first) {
            (Some(Some(ImageSlot::Ready { .. })), Some(first)) => {
                (first.width <= cols).then_some(first)
            }
            (Some(Some(ImageSlot::Ready { pixels, .. })), None) => Some(picture_cells(
                pixels.0,
                pixels.1,
                sizes.font,
                cols,
                sizes.rows,
                sizes.rows,
                MAX_ENLARGE,
            )),
            _ => None,
        };
        pictures.push(DocPicture {
            url: url.to_string(),
            line: lines.len(),
            x: x + lead.width() as u16,
            size,
            main: false,
        });
        match size {
            Some(size) => {
                for row in 0..size.height {
                    let prefix = if row == 0 {
                        &first_prefix
                    } else {
                        &rest_prefix
                    };
                    lines.push(Line::from(prefix.clone()));
                }
            }
            None => {
                let label = match slot {
                    None | Some(Some(ImageSlot::Ready { .. })) => "▣ image",
                    Some(Some(ImageSlot::Failed(_))) => "▣ image unavailable",
                    _ => "loading image",
                };
                let mut spans = first_prefix;
                if label == "loading image" {
                    spans.extend(loading::caption(loading::Kind::Media, label, sizes.phase).spans);
                } else {
                    spans.push(Span::styled(label, Style::new().fg(OVERLAY)));
                }
                lines.push(Line::from(spans));
            }
        }
        rest = rest.get(end + 1..).unwrap_or_default();
    }
    if !rest.is_empty() {
        wrap_into(lines, rest, width, prefix, style);
    }
}

pub(super) fn thread_prefix(depth: usize, visible: usize) -> Vec<Span<'static>> {
    let mut rails: Vec<_> = (0..visible)
        .map(|d| {
            Span::styled(
                "│ ",
                Style::new().fg(THREADS[(depth - visible + d) % THREADS.len()]),
            )
        })
        .collect();
    if depth > visible {
        if let Some(first) = rails.first_mut() {
            first.content = "⋮ ".into();
        } else {
            rails.push(Span::styled("↳", Style::new().fg(OVERLAY)));
        }
    }
    rails
}

impl DocPicture {
    /// Lines the picture takes in the document.
    pub(super) fn rows(&self) -> usize {
        self.size.map_or(1, |size| usize::from(size.height))
    }
}

/// A small picture grows at most this much, so a thumbnail-sized one stays sharp.
pub(super) const MAX_ENLARGE: f64 = 2.0;

/// Cells a `width`×`height` px picture takes: wider than tall, it fills `cols` unless
/// that is more than `wide_max` rows; otherwise it fits `tall_max` rows (centred by the
/// caller). Never enlarged more than `enlarge` times. Rows round up, so fitting the
/// picture again into the cells it got gives the same cells.
pub(super) fn picture_cells(
    width: u32,
    height: u32,
    font: ratatui_image::FontSize,
    cols: u16,
    wide_max: u16,
    tall_max: u16,
    enlarge: f64,
) -> Size {
    let (fw, fh) = (f64::from(font.width.max(1)), f64::from(font.height.max(1)));
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let max_rows = if width > height { wide_max } else { tall_max };
    let scale = (f64::from(cols) * fw / w)
        .min(f64::from(max_rows) * fh / h)
        .min(enlarge);
    let wide = ((w * scale / fw).round() as u16).clamp(1, cols.max(1));
    // Rows follow the width as rounded: a narrow picture rounded down is that much shorter.
    let scale = scale.min(f64::from(wide) * fw / w);
    Size::new(
        wide,
        ((h * scale / fh).ceil() as u16).clamp(1, max_rows.max(1)),
    )
}

/// Text comparison ignores styling and the selected-comment bar.
pub(super) fn line_text<'a>(line: &'a Line<'_>) -> impl Iterator<Item = char> + 'a {
    line.spans
        .iter()
        .flat_map(|span| span.content.chars())
        .enumerate()
        .map(|(i, c)| if i == 0 && c == '▌' { ' ' } else { c })
}

pub(super) fn line_hash(line: &Line) -> u64 {
    let mut hasher = DefaultHasher::new();
    for c in line_text(line) {
        c.hash(&mut hasher);
    }
    hasher.finish()
}

/// Find an unambiguous unchanged line within the old viewport, preserving its
/// distance from the top. Repeated punctuation never selects a different passage.
pub(super) fn edit_anchor(
    old: &[Line],
    at: usize,
    new: &[Line],
    height: usize,
    bounds: std::ops::Range<usize>,
) -> Option<(usize, usize)> {
    let mut occurrences: HashMap<u64, (usize, usize, usize)> = HashMap::with_capacity(old.len());
    for line in old {
        occurrences.entry(line_hash(line)).or_default().0 += 1;
    }
    for (index, line) in new.iter().enumerate() {
        if let Some((_, count, target)) = occurrences.get_mut(&line_hash(line)) {
            *count += 1;
            *target = index;
        }
    }
    old.iter()
        .enumerate()
        .skip(at)
        .take(height)
        .find_map(|(was, line)| {
            if !line_text(line).any(|c| !c.is_whitespace() && c != '│') {
                return None;
            }
            let &(1, 1, target) = occurrences.get(&line_hash(line))? else {
                return None;
            };
            if !bounds.contains(&target) {
                return None;
            }
            line_text(line)
                .eq(line_text(&new[target]))
                .then_some((was, target))
        })
}

/// Where line `at` of `old` is in `new`: the same line, counted among its equals
/// from the top. None when it changed (a spoiler line that wraps otherwise).
pub(super) fn same_line(old: &[Line], at: usize, new: &[Line]) -> Option<usize> {
    let line = old.get(at)?;
    let nth = old[..at].iter().filter(|l| *l == line).count();
    new.iter()
        .enumerate()
        .filter(|(_, l)| *l == line)
        .nth(nth)
        .map(|(i, _)| i)
}

/// Scroll for a document area `height` lines tall, and whether the hold on the selected
/// comment still applies; otherwise the selection is brought into view. Never past the end.
pub(super) fn settle_scroll(
    view: &PostView,
    spans: &[(usize, usize)],
    total: usize,
    height: usize,
) -> (usize, bool) {
    let mut scroll = view.scroll;
    let mut held = false;
    if let Some(hold) = view.hold_view
        && let Some(&(start, end)) = spans.get(view.cursor)
    {
        let at = if hold.top {
            0
        } else {
            start.saturating_add_signed(hold.offset)
        };
        held = at < end && at + height > start;
        if held {
            scroll = at;
        }
    }
    if !view.free_scroll
        && !held
        && let Some(&(start, end)) = spans.get(view.cursor)
    {
        if start < scroll {
            scroll = start;
        } else if end > scroll + height {
            scroll = end.saturating_sub(height).min(start);
        }
    }
    // Stop with the last line at the bottom: scrolling past it would blank the pane.
    (scroll.min(total.saturating_sub(height)), held)
}
