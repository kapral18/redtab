use super::*;

mod header;
mod scroll;

use header::*;
use scroll::*;

pub(super) fn draw_post(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    media: &mut ListPictures,
) -> Option<Rect> {
    let controls = crate::keys::map::context(app) == crate::keys::map::Context::Post
        && app.help_scroll.is_none();
    let gallery_controls =
        controls && crate::keys::map::available(app, crate::keys::map::Action::NextImage);
    let view = app.post_view.as_ref()?;
    let post = view.shown_post();
    let now = now();

    let (rest, overflow_title) = draw_post_header(f, app, area, now, controls, gallery_controls)?;

    let video = post.media.video_url().is_some();
    let image_url = post.media.shows_inline().then(|| {
        app.post_view
            .as_ref()
            .and_then(PostView::preview_image)
            .map(std::borrow::Cow::into_owned)
    });
    // Until the post page arrives, a stale list copy may lack the spoiler tag.
    let hide_image = app
        .post_view
        .as_ref()
        .is_some_and(|v| v.spoiler_hidden() || v.page.is_none());
    let margin = Margin {
        horizontal: 1,
        vertical: 0,
    };
    let full = rest.inner(margin);
    let width = full.width.saturating_sub(2) as usize;
    let still_url = image_url.as_ref().and_then(|u| u.as_deref());
    let main_pixels = post
        .preview_size
        .filter(|_| video)
        .or_else(|| {
            still_url.and_then(|url| {
                match app.images.get(url) {
                    Some(ImageSlot::Ready { pixels, .. }) => Some(*pixels),
                    _ => app.card_image_metadata.get(url).map(|m| m.size),
                }
                .or_else(|| {
                    (!matches!(post.media, Media::Gallery { .. }) || view.gallery_index == 0)
                        .then_some(post.preview_size)
                        .flatten()
                })
            })
        })
        .filter(|&(w, h)| w > 0 && h > 0)
        .or_else(|| {
            video
                .then(|| {
                    app.video_preview(false)
                        .map(|(frame, _)| (frame.width, frame.height))
                })
                .flatten()
        })
        .or_else(|| video.then_some((16, 9)));
    let failed_without_picture = video && failed_video_without_picture(app, post, still_url);
    let main_size = main_pixels
        .filter(|_| !failed_without_picture)
        .map(|pixels| {
            fit_picture(
                pixels,
                app.image_picker.font_size(),
                Size::new(width as u16, full.height.saturating_sub(2)),
                full.height,
            )
        });
    let video_id = video.then(|| post.id.clone());
    let video_url = post.media.video_url().map(str::to_string);
    let main_url = still_url.map(str::to_string).or_else(|| video_id.clone());
    let main_image = main_url.as_deref().filter(|_| !hide_image).map(|url| {
        let size = if app.inline_images_on() && !failed_without_picture {
            main_size
        } else {
            Some(Size::new(width as u16, 0))
        };
        (url, size)
    });
    if video && app.inline_images_on() && !hide_image {
        app.video_area = main_size
            .filter(|size| size.width > 0 && size.height > 0)
            .map(|size| Rect::new(0, 0, size.width, size.height));
    }
    let video_status = video.then(|| video_status(app));
    let sizes = DocumentContext {
        focused: controls,
        phase: app.loading_frame,
        images: app.inline_images_on().then_some(&app.images),
        font: app.image_picker.font_size(),
        rows: TEXT_PICTURE_ROWS.min(full.height).max(1),
    };
    let view = app.post_view.as_mut()?;
    if view.title_in_document != overflow_title.is_some() {
        view.title_in_document = overflow_title.is_some();
        view.content_changed = true;
    }
    let mut more_lines = std::mem::take(&mut view.rendered_more);
    let key = document_key(view, width, now, &sizes, main_image, overflow_title);
    // Relative ages can wrap a header to another line count: that moves the text below
    // like an edit does. A spinner frame advancing in the same draw changes no layout.
    let clock_only = view.page.document_cache.as_ref().is_some_and(|cache| {
        cache.key.second != key.second
            && DocumentKey {
                second: key.second,
                phase: key.phase,
                ..cache.key.clone()
            } == key
    });
    let cached = view
        .page
        .document_cache
        .take()
        .filter(|cache| cache.key == key && !view.content_changed && view.rewrapped.is_none());
    let PostDocument {
        mut lines,
        spans,
        pictures,
    } = if let Some(cache) = cached {
        PostDocument {
            lines: std::mem::take(&mut view.rendered_document),
            spans: cache.spans,
            pictures: cache.pictures,
        }
    } else {
        post_document(
            view,
            width,
            now,
            Some(&mut more_lines),
            &sizes,
            main_image,
            overflow_title,
        )
    };
    // A header rewrapping as its age changes moves the rows after it, even when another
    // rewrap keeps the document's length.
    if clock_only
        && !view.rendered_document.is_empty()
        && (lines.len() != view.rendered_document.len()
            || spans
                .iter()
                .map(|(start, _)| *start)
                .ne(view.row_starts.iter().copied()))
    {
        view.content_changed = true;
    }
    view.page.document_cache = Some(CachedDocument {
        key,
        spans: spans.clone(),
        pictures: pictures.clone(),
    });
    if let Some(status) = video_status
        && let Some(picture) = pictures.iter().find(|p| p.main)
        && picture.rows() > 0
    {
        lines[picture.line + picture.rows()] = status;
    }
    view.row_starts = spans.iter().map(|(s, _)| *s).collect();
    let inputs = DocumentInputs {
        width,
        now,
        sizes: &sizes,
        main_image,
        overflow_title,
    };
    let scroll = place_scroll(
        view,
        &lines,
        &spans,
        &pictures,
        &inputs,
        full.height as usize,
    );
    let doc_area = full;
    view.drawn_cursor = (!view.free_scroll).then_some(view.cursor);
    view.record_drawn_anchor();
    let height = doc_area.height as usize;
    view.page_height = height;
    let total = lines.len();
    for (row, line) in lines.iter().skip(scroll).take(height).enumerate() {
        f.buffer_mut()
            .set_line(doc_area.x, doc_area.y + row as u16, line, doc_area.width);
    }
    let primary_loader = if view.page.is_none() && view.error.is_none() {
        let occupied = total.saturating_sub(1).saturating_sub(scroll).min(height) as u16;
        loading::draw(
            f,
            Rect::new(
                doc_area.x,
                doc_area.y + occupied,
                doc_area.width,
                doc_area.height - occupied,
            ),
            loading::Kind::Initial,
            "Loading post",
            sizes.phase,
        )
    } else {
        more_lines
            .iter()
            .find(|(id, line)| {
                view.loading_more.contains(id) && *line >= scroll && *line < scroll + height
            })
            .map(|(_, line)| {
                Rect::new(
                    doc_area.x,
                    doc_area.y + (*line - scroll) as u16,
                    doc_area.width,
                    1,
                )
            })
    };
    view.rendered_document = lines;
    view.rendered_more = more_lines;
    view.rendered_pictures = pictures
        .iter()
        .map(|p| (p.url.clone(), p.line, p.rows()))
        .collect();
    draw_scrollbar(f, doc_area, scroll, height, total);
    // Pictures in the text: drawn when wholly in view, loaded from a screen away.
    let near = scroll.saturating_sub(height)..scroll + height * 2;
    let images_on = sizes.images.is_some();
    let mut wanted = Vec::new();
    let mut shown = Vec::new();
    for picture in &pictures {
        if picture.main {
            let start = picture.line.max(scroll);
            let end = (picture.line + picture.rows()).min(scroll + height);
            if start < end {
                let size = picture.size.unwrap_or(Size::new(width as u16, 1));
                media.cards.push(CardPicture {
                    url: still_url.map(str::to_string),
                    area: Rect::new(
                        doc_area.x + picture.x,
                        doc_area.y + (start - scroll) as u16,
                        size.width,
                        (end - start) as u16,
                    ),
                    size,
                    skipped: (start - picture.line) as u16,
                    pixels: main_pixels.unwrap_or((1, 1)),
                    indicator: None,
                    selected: video,
                    detail: true,
                    post_id: video_id.clone(),
                    video_url: video_url.clone(),
                    video_frame: false,
                });
            }
            continue;
        }
        if images_on && near.contains(&picture.line) {
            wanted.push(picture.url.clone());
        }
        match picture.size {
            Some(size)
                if picture.line >= scroll
                    && picture.line + usize::from(size.height) <= scroll + height =>
            {
                let at = Rect::new(
                    doc_area.x + picture.x,
                    doc_area.y + (picture.line - scroll) as u16,
                    size.width,
                    size.height,
                );
                shown.push((picture.url.clone(), at));
            }
            _ => {}
        }
    }
    media
        .inline
        .extend(shown.into_iter().map(|(url, area)| ImageSpot { url, area }));
    for url in wanted {
        app.request_text_picture(&url);
    }
    release_far_pictures(app, &pictures, scroll, height);
    app.post_drawn(!doc_area.is_empty());
    primary_loader
}

fn release_far_pictures(app: &mut App, pictures: &[DocPicture], scroll: usize, height: usize) {
    // Far from view: a picture still queued is let go so the ones in view come first; a
    // loaded one only once many are loaded. Either loads again when near. Pictures with no
    // place now (folded, removed, hidden again) are far too.
    let kept = scroll.saturating_sub(height * 3)..scroll + height * 4;
    let post_image = app
        .post_view
        .as_ref()
        .and_then(PostView::preview_image)
        .map(std::borrow::Cow::into_owned);
    let text_pictures = || {
        app.images
            .iter()
            .filter(|(url, _)| post_image.as_deref() != Some(url.as_str()))
    };
    let loaded = text_pictures()
        .filter(|(_, slot)| matches!(slot, ImageSlot::Ready { .. }))
        .count();
    let far: Vec<String> = text_pictures()
        .filter(|(_, slot)| match slot {
            ImageSlot::Loading => true,
            ImageSlot::Ready { .. } => loaded > TEXT_PICTURES_KEPT,
            ImageSlot::Failed(_) => false,
        })
        .filter(|(url, _)| {
            !pictures
                .iter()
                .any(|p| &p.url == *url && kept.contains(&p.line))
        })
        .map(|(url, _)| url.clone())
        .collect();
    for url in &far {
        app.drop_text_picture(url);
    }
}
