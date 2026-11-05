use super::*;

impl App {
    pub(super) fn on_post(
        &mut self,
        generation: u64,
        result: Result<Box<PostPage>, String>,
        source: Source,
        refreshing: bool,
    ) {
        if !refreshing {
            self.pending_posts
                .retain(|_, pending| *pending != generation);
        }
        if result
            .as_ref()
            .is_err_and(|error| error == crate::reddit::REQUEST_CANCELLED)
            && let Some(view) = self
                .post_view
                .as_ref()
                .filter(|view| view.generation == generation)
        {
            let permalink = view.post.permalink.clone();
            self.load_post(permalink);
            return;
        }
        let Some(view) = &mut self.post_view else {
            return;
        };
        if view.generation != generation {
            // An earlier load of this same post, still on its way when you came
            // back to it: take its network answer rather than wait for the newer
            // load, while this view has nothing, or only a cached copy refreshing
            // that the answer is provably no older than (`r` keeps an up-to-date
            // copy loading). Never a cached answer: one still refreshing may rely
            // on a network load the worker has since dropped.
            let waiting = view.page.is_none()
                || (view.loading
                    && view
                        .freshness
                        .is_some_and(|f| f.source.cached && source.may_replace(f.source)));
            let same_post = !source.cached
                && view.comment_sort.is_empty()
                && self
                    .pending_posts
                    .get(&view.post.permalink)
                    .is_none_or(|pending| *pending == generation)
                && generation > self.content_generation_floor
                && waiting
                && result
                    .as_ref()
                    .is_ok_and(|page| !page.post.id.is_empty() && page.post.id == view.post.id);
            if !same_post {
                return;
            }
            view.generation = generation;
            // Reply loads sent under the newer generation are dropped on arrival.
            view.loading_more.clear();
            self.pool.want_post(Some(generation));
        }
        view.loading = false;
        match result {
            Ok(page) => {
                let mut page = *page;
                filter_removed_comments(&mut page.comments, source, &view.removed_comments);
                let known: Option<HashSet<String>> = view.page.as_ref().map(|old| {
                    thread_roots(&old.comments)
                        .iter()
                        .map(|comment| comment.id.clone())
                        .collect()
                });
                let selected = view.cursor_key();
                view.content_changed = true;
                view.pin_view();
                if let Some(old) = view.page.as_mut() {
                    merge_comments(
                        &mut page.comments,
                        &mut old.comments,
                        source,
                        false,
                        false,
                        &mut view.removed_comments,
                    );
                } else {
                    merge_comments(
                        &mut page.comments,
                        &mut Vec::new(),
                        source,
                        false,
                        false,
                        &mut view.removed_comments,
                    );
                }
                // A link post's page has no thumbnail; keep the listing's.
                if let (
                    Media::Link {
                        url: page_url,
                        thumb: thumb @ None,
                    },
                    Media::Link {
                        url,
                        thumb: Some(listing_thumb),
                    },
                ) = (&mut page.post.media, &view.post.media)
                    && page_url == url
                {
                    *thumb = Some(listing_thumb.clone());
                }
                let image = page
                    .post
                    .media
                    .preview_image_at(view.gallery_index)
                    .map(std::borrow::Cow::into_owned);
                // The bookmark learns a spoiler tag its saved listing may lack.
                if page.post.spoiler {
                    self.state.mark_bookmark_spoiler(&page.post.id);
                }
                if page.post.original_deleted {
                    view.post.original_deleted = true;
                    for post in self
                        .tabs
                        .iter_mut()
                        .flat_map(|tab| &mut tab.feed.posts)
                        .chain(
                            self.all_subs
                                .groups
                                .iter_mut()
                                .flat_map(|group| &mut group.posts),
                        )
                        .filter(|post| post.id == page.post.id)
                    {
                        post.original_deleted = true;
                    }
                    self.state.mark_original_deleted(&page.post.id, now());
                }
                let arrived = known.as_ref().is_some_and(|known| {
                    unseen_leads(
                        thread_roots(&page.comments)
                            .iter()
                            .map(|comment| (comment.id.as_str(), comment.stickied)),
                        |id| known.contains(id),
                    )
                });
                view.page = Some(page).into();
                view.error = None;
                view.reselect(selected);
                view.row_starts.clear();
                if view.reveal && !source.cached {
                    view.reveal = false;
                    if arrived {
                        view.show_top();
                    }
                }
                if self.inline_images_on()
                    && let Some(url) = image
                {
                    self.request_image(&url);
                }
            }
            Err(e) => {
                if !source.cached {
                    view.reveal = false;
                }
                view.error = Some(e);
            }
        }
    }

    pub(super) fn on_more_replies(
        &mut self,
        generation: u64,
        comment_id: String,
        result: Result<ReplyPage, String>,
        source: Source,
        refreshing: bool,
    ) {
        let Some(view) = &mut self.post_view else {
            return;
        };
        if view.generation != generation {
            return;
        }
        view.loading_more.remove(&comment_id);
        match result {
            Ok(ReplyPage {
                comments: mut replies,
                more,
                refreshed,
                order,
            }) => {
                filter_removed_comments(&mut replies, source, &view.removed_comments);
                view.content_changed = true;
                // Rows above may grow: keep the selection and the text being read in place.
                let mut selected = view.cursor_key();
                if !view.more_handoff
                    && view.free_scroll
                    && let Some((_, start)) = view.rendered_more.iter().find(|(id, start)| {
                        id == &comment_id
                            && (*start == view.scroll || *start == view.scroll.saturating_add(1))
                    })
                {
                    let offset = view.scroll as isize - *start as isize;
                    view.keep_reading =
                        Some((RowKey::More(comment_id.clone()), offset, view.scroll));
                    view.more_handoff = true;
                }
                view.pin_view();
                let more_at = view.row_index(&RowKey::More(comment_id.clone()));
                let selected_more =
                    matches!(&selected, Some(RowKey::More(id)) if *id == comment_id);
                let reading_more = matches!(&view.keep_reading, Some((RowKey::More(id), _, _)) if *id == comment_id);
                let mut first_added = None;
                if let Some(page) = view.page.as_mut()
                    && let Some(c) = find_comment_mut(&mut page.comments, &comment_id)
                {
                    let allow_new = !replies_superseded(c, source);
                    if selected_more || reading_more {
                        first_added = first_new_reply(
                            &replies,
                            &c.replies,
                            source,
                            allow_new,
                            &view.collapsed,
                        )
                        .map(|id| RowKey::Comment(id.to_owned()));
                    }
                    merge_replies(
                        &mut c.replies,
                        replies,
                        source,
                        allow_new,
                        ReplyMerge::Expansion {
                            reorder: c.sources.is_none_or(|old| source.may_replace(old.more)),
                            refreshed: refreshed.as_deref(),
                            order: &order,
                        },
                        &mut view.removed_comments,
                    );
                    let mut sources = c.sources.unwrap_or(CommentSources {
                        content: source,
                        more: source,
                        parent: None,
                    });
                    if source.may_replace(sources.more) {
                        // An empty preview has no replacement row: keep the More
                        // target until terminal delivery.
                        if !refreshing {
                            c.more = more;
                        }
                        sources.more = source;
                        c.sources = Some(sources);
                    }
                }
                if selected_more {
                    selected = if reading_more {
                        first_added.clone().or(selected)
                    } else {
                        first_added.take().or(selected)
                    };
                }
                view.reselect(selected);
                view.row_starts.clear();
                // A selected or read More row follows its first inserted reply,
                // even with a newer continuation pending.
                let reading = view.keep_reading.take();
                view.keep_reading = reading.map(|(key, offset, scroll)| match (&key, more_at) {
                    (RowKey::More(id), index) if *id == comment_id => {
                        let next = first_added.take().unwrap_or_else(|| {
                            if view.row_index(&key).is_some() {
                                key
                            } else {
                                index
                                    .and_then(|i| view.rows().get(i).map(RowKey::of))
                                    .unwrap_or(key)
                            }
                        });
                        (next, offset, scroll)
                    }
                    _ => (key, offset, scroll),
                });
            }
            Err(e) => self.error(format!("more replies: {e}")),
        }
    }
}
