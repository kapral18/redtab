use super::*;

impl App {
    pub fn comment_tools(&mut self) {
        if self.post_view.is_none() {
            return;
        }
        let mut choices = vec![choice(
            "/",
            "Find in loaded comments",
            Command::Input(form(
                "Find in loaded comments",
                FormKind::Find,
                vec![field("Text or author", "", false)],
                &self.current_post().map(|p| p.permalink).unwrap_or_default(),
            )),
        )];
        for (shortcut, label, sort) in [
            ("b", "Best", "confidence"),
            ("t", "Top", "top"),
            ("n", "New", "new"),
            ("o", "Old", "old"),
            ("c", "Controversial", "controversial"),
            ("Q", "Q&A", "qa"),
        ] {
            choices.push(choice(
                shortcut,
                &format!("Sort comments: {label}"),
                Command::Sort(sort.into()),
            ));
        }
        self.push_menu("Comments", choices, MenuScope::Comments);
    }

    pub(crate) fn find_in_comments(&mut self) {
        if self.post_view.as_ref().is_none_or(PostView::spoiler_hidden) {
            return;
        }
        self.clear_experience();
        self.experience.screens.push(Screen::Input(form(
            "Find in loaded comments",
            FormKind::Find,
            vec![field("Text or author", "", false)],
            &self
                .current_post()
                .map(|post| post.permalink)
                .unwrap_or_default(),
        )));
        self.overlay = Some(Overlay::Experience);
    }

    pub(super) fn sort_comments(&mut self, sort: &str) {
        let Some(post) = self
            .post_view
            .as_ref()
            .map(|view| view.shown_post().clone())
        else {
            return;
        };
        let permalink = post.permalink.clone();
        if let Some(generation) = self.show_post(post, &permalink) {
            self.post_view.as_mut().unwrap().comment_sort = sort.into();
            self.focus = Focus::Post;
            self.pool.want_post(Some(generation));
            self.pool.submit(Job::SortedPost {
                generation,
                permalink,
                sort: sort.into(),
            });
        }
    }

    pub(super) fn find_comments(&mut self, text: &str) {
        let Some(view) = &self.post_view else {
            return;
        };
        if view.spoiler_hidden() {
            self.experience.error =
                Some("Reveal this post's spoilers with zs before searching".into());
            return;
        }
        let Some(page) = view.page.as_ref() else {
            self.experience.error = Some("Wait for the comments to load".into());
            return;
        };
        let mut entries = Vec::new();
        collect_matches(
            &page.comments,
            &page.post,
            &text.to_lowercase(),
            &mut entries,
        );
        self.experience.screens.push(Screen::Browse {
            pagination: Default::default(),
            query: Query::Thread(page.post.permalink.clone()),
            page: Page {
                title: format!("Loaded comments matching {text}"),
                text: "Only comments already loaded in this post are searched.".into(),
                entries,
                after: None,
                url: page.post.reddit_url(),
            },
            selected: 0,
            scroll: 0,
        });
    }

    pub(super) fn select_loaded_comment(&mut self, id: &str) -> bool {
        let Some(view) = &mut self.post_view else {
            return false;
        };
        if !view
            .page
            .as_ref()
            .is_some_and(|page| has_comment(&page.comments, id))
        {
            return false;
        }
        view.collapsed.clear();
        let index = view.rows().iter().position(
            |row| matches!(&row.kind, RowKind::Comment { comment, .. } if comment.id == id),
        );
        if let Some(index) = index {
            view.cursor = index;
            view.free_scroll = false;
            view.hold_view = None;
            view.drawn_cursor = None;
            view.row_starts.clear();
            view.content_changed = true;
            self.focus = Focus::Post;
            return true;
        }
        false
    }

    pub fn gallery_step(&mut self, forward: bool) {
        let Some(view) = &mut self.post_view else {
            return;
        };
        if view.spoiler_hidden() {
            self.info(format!(
                "Reveal spoilers with {} first",
                crate::keys::map::action_label(crate::keys::map::Action::Spoilers)
            ));
            return;
        }
        let Media::Gallery { images, .. } = &view.shown_post().media else {
            return;
        };
        let len = images.len();
        if len == 0 {
            return;
        }
        view.gallery_index = if forward {
            (view.gallery_index + 1) % len
        } else {
            (view.gallery_index + len - 1) % len
        };
        view.scroll = 0;
        view.free_scroll = true;
        let url = view.preview_image().map(std::borrow::Cow::into_owned);
        if let Some(url) = url.filter(|_| self.inline_images_on()) {
            self.request_image(&url);
        }
    }
}

pub(super) fn collect_matches(
    comments: &[Comment],
    post: &Post,
    query: &str,
    entries: &mut Vec<Entry>,
) {
    for comment in comments {
        if !comment.id.starts_with(THREAD_ROOT)
            && (crate::model::unformatted(&comment.body)
                .to_lowercase()
                .contains(query)
                || comment.author.to_lowercase().contains(query))
        {
            entries.push(Entry {
                icon: Default::default(),
                title: format!("u/{} · {}", comment.author, comment.id),
                body: comment.body.clone(),
                target: Some(Target {
                    id: format!("t1_{}", comment.id),
                    author: comment.author.clone(),
                    subreddit: post.subreddit.clone(),
                    permalink: format!("/comments/{}/_/{}", post.id, comment.id),
                }),
                query: None,
                post: None,
            });
        }
        collect_matches(&comment.replies, post, query, entries);
    }
}
