use super::*;

pub(super) struct DocumentInputs<'a> {
    pub(super) width: usize,
    pub(super) now: DateTime<Utc>,
    pub(super) sizes: &'a DocumentContext<'a>,
    pub(super) main_image: Option<(&'a str, Option<Size>)>,
    pub(super) overflow_title: Option<Style>,
}

/// Places the document scroll after a redraw and returns it.
pub(super) fn place_scroll(
    view: &mut PostView,
    lines: &[Line<'static>],
    spans: &[(usize, usize)],
    pictures: &[DocPicture],
    inputs: &DocumentInputs,
    height: usize,
) -> usize {
    let requested_scroll = view.scroll;
    // Replies loaded above the text being read: put it back where it was.
    let reading_anchor = view.take_reading_anchor();
    if let Some((row, offset)) = reading_anchor
        && let Some(&(start, _)) = spans.get(row)
    {
        view.scroll = start.saturating_add_signed(offset);
    }
    // Pictures above the text being read took their rows as they loaded: move with it. On
    // a content change, text matching below, where it applies, places the view instead.
    if reading_anchor.is_none() {
        let grown: isize = pictures
            .iter()
            .zip(&view.rendered_pictures)
            .take_while(|(new, (url, ..))| new.url == *url)
            .filter(|(_, (_, line, _))| *line < requested_scroll)
            // Only its rows above the top count: one wholly above moves the text by all it
            // gained, one across the top by those it now has above the top.
            .map(|(new, (_, line, rows))| {
                let above = requested_scroll - line;
                if *rows <= above {
                    new.rows() as isize - *rows as isize
                } else {
                    new.rows().min(above) as isize - above as isize
                }
            })
            .sum();
        view.scroll = view.scroll.saturating_add_signed(grown);
        // A comment held in place (`l`) is held at the moved text too.
        if grown != 0
            && let Some(hold) = &mut view.hold_view
            && !hold.top
            && let Some(&(start, _)) = spans.get(hold.row)
        {
            hold.offset = view.scroll as isize - start as isize;
        }
    }
    if view.content_changed {
        let handoff = std::mem::take(&mut view.more_handoff);
        if handoff && reading_anchor.is_some() {
            // The semantic row already accounts for spoiler reflow above it.
            view.rewrapped = None;
        }
        let held = view
            .hold_view
            .filter(|hold| !view.free_scroll && hold.row == view.cursor && !hold.top);
        // Text matching must not override a More → first-new-reply handoff by matching a
        // later sibling.
        let anchor_row = if !view.free_scroll {
            Some(view.cursor)
        } else if handoff {
            reading_anchor.map(|(row, _)| row)
        } else {
            None
        };
        let bounds = anchor_row
            .and_then(|row| spans.get(row))
            .map_or(0..lines.len(), |&(start, end)| start..end);
        if requested_scroll != 0
            && (view.free_scroll || held.is_some())
            && let Some((was, target)) = edit_anchor(
                &view.rendered_document,
                requested_scroll,
                lines,
                view.page_height,
                bounds,
            )
        {
            view.scroll = target.saturating_sub(was - requested_scroll);
            if let Some(hold) = &mut view.hold_view
                && let Some((start, _)) = spans.get(hold.row)
            {
                hold.offset = view.scroll as isize - *start as isize;
            }
            // The retained document already represents the pre-toggle display.
            view.rewrapped = None;
        }
        view.content_changed = false;
    }
    // After `v` only spoiler lines changed, so the old top line exists in the new wrap.
    // Keys in the same batch move on from it; `g` stays at the top.
    if let Some((was, pinned)) = view.rewrapped.take()
        && view.free_scroll
        && view.reveal_spoilers != was
        && (view.scroll != 0 || pinned == 0)
    {
        let shown = view.reveal_spoilers;
        view.reveal_spoilers = was;
        let old_lines = post_document(
            view,
            inputs.width,
            inputs.now,
            None,
            inputs.sizes,
            inputs.main_image,
            inputs.overflow_title,
        )
        .lines;
        view.reveal_spoilers = shown;
        if let Some(at) = same_line(&old_lines, pinned, lines) {
            let moved = view.scroll as isize - pinned as isize;
            view.scroll = at.saturating_add_signed(moved);
        }
    }
    // `l` on the comment being read holds its text in place while it stays selected and
    // visible; moving the selection ends the hold. Loads above move both.
    if view.free_scroll || view.hold_view.is_none_or(|h| h.row != view.cursor) {
        view.hold_view = None;
    }
    let (scroll, held) = settle_scroll(view, spans, lines.len(), height);
    let main_visible = pictures
        .iter()
        .any(|p| p.main && p.rows() > 0 && p.line < scroll + height && p.line + p.rows() > scroll);
    view.image_shown = main_visible;
    view.scroll = scroll;
    if !held {
        view.hold_view = None;
    }
    // A hold kept at the top for the image: with no image this frame, hold the text where it is.
    if !view.image_shown
        && let Some(hold) = &mut view.hold_view
        && hold.top
        && let Some(&(start, _)) = spans.get(hold.row)
    {
        hold.top = false;
        hold.offset = scroll as isize - start as isize;
    }
    scroll
}
