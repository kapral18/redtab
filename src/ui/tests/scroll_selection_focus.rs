use super::*;

#[test]
fn scroll_keys_before_the_draw_scroll_from_the_new_top() {
    // Between an answer at the top, or `r` bringing new posts from further down, and
    // the next draw, scroll keys scroll from the new first post.
    for (page, manual) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut app = card_feed_app(kitty());
        screen(&mut app, 100, 20);
        let mut posts = app.tabs[4].feed.posts.clone();
        if manual {
            app.move_selection(3);
            screen(&mut app, 100, 20);
            let mut new = posts[0].clone();
            new.id = "new".into();
            posts.insert(0, new);
            app.reload();
        } else {
            posts.swap(0, 1);
        }
        let first = posts[0].id.clone();
        let (tab, generation) = (app.tabs[4].id, app.tabs[4].feed.generation);
        app.on_msg(Msg::Feed {
            tab,
            generation,
            append: false,
            result: Ok(crate::model::Listing { posts, after: None }),
        });

        if page {
            app.page_list(1, 2);
        } else {
            app.scroll_card(3);
        }
        screen(&mut app, 100, 20);

        let feed = &app.active_tab().feed;
        let case = format!("page {page} manual {manual}");
        if page {
            assert!(feed.selected <= 1, "{case}: {}", feed.selected);
            assert!(feed.selected > 0 || feed.cards.line > 0, "{case}");
        } else {
            assert_eq!(app.current_post().unwrap().id, first, "{case}");
            assert_eq!((feed.offset, feed.cards.line), (0, 3), "{case}");
        }
    }
}

#[test]
fn when_line_scrolling_crosses_a_separator_it_should_move_exactly_one_row_each_way() {
    let mut app = two_tall_media_cards();
    app.tabs[4].feed.posts.push(post_with("tail", Media::None));
    app.scroll_card(11);
    screen(&mut app, 100, 20);

    app.scroll_card(1);
    let down = screen(&mut app, 100, 20);
    app.scroll_card(-1);
    let up = screen(&mut app, 100, 20);

    assert_eq!(
        down.lines()
            .position(|line| line.contains("Following video")),
        Some(5)
    );
    assert_eq!(
        up.lines().position(|line| line.contains("Following video")),
        Some(6)
    );
    assert_eq!(app.active_tab().feed.cards.line, 11);
}

#[test]
fn when_item_selection_follows_continuous_scrolling_it_should_reveal_the_selected_post() {
    let mut app = two_tall_media_cards();
    app.scroll_card(8);
    screen(&mut app, 100, 20);

    press(&mut app, 'j');
    let actual = screen(&mut app, 100, 20);

    assert_eq!(app.current_post().unwrap().id, "p2");
    assert_eq!(app.active_tab().feed.offset, 0);
    assert_eq!(app.active_tab().feed.cards.line, 9);
    assert_eq!(
        actual
            .lines()
            .position(|line| line.contains("Following video")),
        Some(8)
    );
}

#[test]
fn when_bookmarks_are_empty_row_keys_should_not_queue_a_scroll_for_future_entries() {
    let mut app = two_tall_media_cards();
    let first = app.tabs[4].feed.posts[0].clone();
    app.state.toggle_bookmark(&first, 1);
    app.switch_tab(1);
    screen(&mut app, 100, 20);
    app.state.toggle_bookmark(&first, 2);
    screen(&mut app, 100, 20);

    app.scroll_card(8);

    assert_eq!(app.active_tab().feed.cards.pending, 0);
}

#[test]
fn when_detail_is_split_h_should_focus_the_list_without_leaving_the_comment() {
    let (mut app, _) = open_long_post(|_| {});
    let mut other = app.all_subs.groups[0].posts[0].clone();
    other.id = "other".into();
    other.permalink = "/r/s/comments/other/x/".into();
    app.all_subs.groups[0].posts.push(other);
    screen(&mut app, 180, 30);
    press(&mut app, 'j');
    screen(&mut app, 180, 30);
    let view = app.post_view.as_ref().unwrap();
    let expected = (view.generation, view.cursor, view.scroll, view.free_scroll);
    assert!(view.scroll > 0, "reading the comment, not the post's top");

    press(&mut app, 'h');
    screen(&mut app, 180, 30);
    let view = app.post_view.as_ref().unwrap();
    let actual = (view.generation, view.cursor, view.scroll, view.free_scroll);
    press(&mut app, 'j');

    assert_eq!(actual, expected, "h must not leave or reset the comment");
    assert_eq!(app.all_subs.post, 1, "j now moves the list selection");
    assert_eq!(app.current_post().unwrap().id, "other");
}

#[test]
fn h_shows_a_hidden_sidebar_only_while_it_has_focus() {
    let mut app = app(&["a"]);
    app.switch_tab(0);
    app.toggle_sidebar();

    press(&mut app, 'h');
    let shown = app.show_sidebar;
    press(&mut app, 'j');
    press(&mut app, 'l');

    assert!(shown);
    assert!(!app.show_sidebar);
    assert_eq!(app.active, 1, "the tab picked in the sidebar stays active");
}
