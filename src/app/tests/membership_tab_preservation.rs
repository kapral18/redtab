use super::*;

#[test]
fn when_membership_changes_it_should_preserve_tabs_and_reject_old_island_results() {
    let mut app = app_with(&["rust"]);
    let index = tab_index(&app, "rust");
    app.switch_tab(index);
    app.tabs[index].feed.selected = 3;
    let tab_id = app.active_tab().id;
    let old_generation = app.all_subs.groups[0].generation;
    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok((vec![])
            .into_iter()
            .map(|name| crate::model::Subscription::Community {
                name,
                icon: crate::model::SubscriptionIcon::Unknown,
            })
            .collect()),
    });
    assert_eq!(app.active_tab().id, tab_id);
    assert_eq!(app.active_tab().feed.selected, 3);
    assert!(matches!(
        app.active_tab().kind,
        TabKind::Subreddit { pinned: false, .. }
    ));
    assert!(app.all_subs.groups.is_empty());

    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok((vec!["rust".into()])
            .into_iter()
            .map(|name| crate::model::Subscription::Community {
                name,
                icon: crate::model::SubscriptionIcon::Unknown,
            })
            .collect()),
    });
    app.on_msg(Msg::Group {
        generation: old_generation,
        subreddit: "rust".into(),
        result: Ok(listing("rust", &["obsolete"], None)),
    });
    assert_eq!(
        app.tabs
            .iter()
            .filter(|t| t.subreddit() == Some("rust"))
            .count(),
        1
    );
    assert_eq!(app.active_tab().id, tab_id);
    assert!(matches!(
        app.active_tab().kind,
        TabKind::Subreddit { pinned: true, .. }
    ));
    assert!(app.all_subs.groups[0].posts.is_empty());
}

#[test]
fn when_join_leave_and_rejoin_are_confirmed_it_should_move_the_same_reading_tab() {
    use crate::reddit::tests::{Fixture, Reply};
    use crossterm::event::KeyCode;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let joined = Arc::new(AtomicBool::new(false));
    let writes = Arc::new(AtomicUsize::new(0));
    let actual = joined.clone();
    let count = writes.clone();
    let fixture = Fixture::new(move |request| {
        let url = url::Url::parse(&format!("http://fixture{}", request.url)).unwrap();
        match url.path() {
            "/api/me.json" => {
                Reply::json(json!({"data":{"name":"reader","modhash":"fixture-csrf"}}))
            }
            "/api/subscribe" => {
                let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                    .into_owned()
                    .collect();
                assert_eq!(form["sr_name"], "armenia");
                count.fetch_add(1, Ordering::SeqCst);
                actual.store(form["action"] == "sub", Ordering::SeqCst);
                Reply::json(json!({}))
            }
            "/subreddits/mine/subscriber.json" => {
                let mut children = vec![json!({"kind":"t5","data":{"display_name":"rust"}})];
                if actual.load(Ordering::SeqCst) {
                    children.push(json!({"kind":"t5","data":{"display_name":"armenia"}}));
                }
                Reply::json(json!({"kind":"Listing","data":{"children":children,"after":null}}))
            }
            path if path.ends_with("/hot") => {
                let sub = path.split('/').nth(2).unwrap();
                let children: Vec<_> = ["a","b","c","d"].into_iter().map(|id|
                        json!({"kind":"t3","data":{"id":id,"title":id,"subreddit":sub,"permalink":format!("/r/{sub}/comments/{id}/title/"),"author":"reader"}})).collect();
                Reply::json(json!({"kind":"Listing","data":{"children":children,"after":null}}))
            }
            _ => Reply::status(404),
        }
    });
    let (mut app, messages) = app_at(&fixture.base, &["rust"]);
    app.open_subreddit("armenia");
    while app.active_tab().feed.loading() {
        app.on_msg(
            messages
                .recv_timeout(Duration::from_secs(5))
                .expect("feed response"),
        );
    }
    app.tabs[app.active].feed.selected = 3;
    app.tabs[app.active].feed.offset = 2;
    let tab = app.active_tab().id;
    for (expected_count, join) in [(1, true), (2, false), (3, true)] {
        sequence(&mut app, " m");
        assert!(
            matches!(&app.overlay, Some(Overlay::Membership { target: MembershipTarget::Community(subreddit), join: action }) if subreddit == "armenia" && *action == join)
        );
        press(&mut app, KeyCode::Enter);
        assert!(app.overlay.is_none());
        while app.membership.is_some() {
            app.on_msg(
                messages
                    .recv_timeout(Duration::from_secs(5))
                    .expect("membership response"),
            );
        }
        assert_eq!(joined.load(Ordering::SeqCst), join);
        assert_eq!(writes.load(Ordering::SeqCst), expected_count);
        assert_eq!(app.active_tab().id, tab);
        assert_eq!(app.active_tab().feed.selected, 3);
        assert_eq!(app.active_tab().feed.offset, 2);
        assert_eq!(app.joined("armenia"), Some(join));
        assert!(
            matches!(app.active_tab().kind, TabKind::Subreddit { pinned, .. } if pinned == join)
        );
        assert_eq!(
            app.tabs
                .iter()
                .filter(|tab| tab.subreddit() == Some("armenia"))
                .count(),
            1
        );
    }
}

#[test]
fn following_unfollowing_and_refollowing_should_preserve_the_same_reading_tab() {
    use crate::reddit::tests::{Fixture, Reply};
    use crossterm::event::KeyCode;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let followed = std::sync::Arc::new(AtomicBool::new(false));
    let writes = std::sync::Arc::new(AtomicUsize::new(0));
    let actual = followed.clone();
    let count = writes.clone();
    let fixture = Fixture::new(move |request| {
        let path = request.url.split('?').next().unwrap();
        match path {
            "/api/me.json" => {
                Reply::json(json!({"data":{"name":"reader","modhash":"fixture-csrf"}}))
            }
            "/user/blackvault/about.json" => {
                Reply::json(json!({"data":{"name":"blackvault","subreddit":{
                    "name":"t5_profile", "subreddit_type":"user", "url":"/user/blackvault/"
                }}}))
            }
            "/api/subscribe" => {
                let form: HashMap<_, _> = url::form_urlencoded::parse(request.body.as_bytes())
                    .into_owned()
                    .collect();
                assert_eq!(form["sr"], "t5_profile");
                actual.store(form["action"] == "sub", Ordering::SeqCst);
                count.fetch_add(1, Ordering::SeqCst);
                Reply::json(json!({}))
            }
            "/subreddits/mine/subscriber.json" => {
                let mut children = vec![json!({"kind":"t5","data":{"display_name":"blackvault"}})];
                if actual.load(Ordering::SeqCst) {
                    children.push(json!({"kind":"t5","data":{"display_name":"u_blackvault", "subreddit_type":"user", "url":"/user/blackvault/"}}));
                }
                Reply::json(json!({"kind":"Listing","data":{"children":children,"after":null}}))
            }
            "/user/blackvault/submitted.json" => {
                Reply::json(json!({"kind":"Listing","data":{"children":[
                    {"kind":"t3","data":{"id":"a","title":"a","subreddit":"rust","author":"blackvault","permalink":"/r/rust/comments/a/title/"}},
                    {"kind":"t3","data":{"id":"b","title":"b","subreddit":"rust","author":"blackvault","permalink":"/r/rust/comments/b/title/"}},
                    {"kind":"t3","data":{"id":"c","title":"c","subreddit":"rust","author":"blackvault","permalink":"/r/rust/comments/c/title/"}}
                ],"after":null}}))
            }
            "/r/rust/comments/c/title/.json" | "/comments/c.json" => Reply::json(json!([
                {"kind":"Listing","data":{"children":[{"kind":"t3","data":{"id":"c","title":"c","subreddit":"rust","author":"blackvault","permalink":"/r/rust/comments/c/title/", "url":"https://i.redd.it/c.png","post_hint":"image"}}]}},
                {"kind":"Listing","data":{"children":[]}}
            ])),
            _ => Reply::status(404),
        }
    });
    let (mut app, messages) = app_at(&fixture.base, &["blackvault"]);
    app.open_user("blackvault");
    while app.active_tab().feed.loading() {
        app.on_msg(
            messages
                .recv_timeout(Duration::from_secs(5))
                .expect("user feed response"),
        );
    }
    app.tabs[app.active].feed.selected = 2;
    app.tabs[app.active].feed.offset = 1;
    app.open_current_post();
    while app
        .post_view
        .as_ref()
        .is_some_and(|view| view.page.is_none() && view.error.is_none())
    {
        app.on_msg(
            messages
                .recv_timeout(Duration::from_secs(5))
                .expect("post response"),
        );
    }
    assert!(app.post_view.as_ref().unwrap().page.is_some());
    app.post_view.as_mut().unwrap().scroll = 7;
    app.focus = Focus::Sidebar;
    let tab = app.active_tab().id;

    for (expected_writes, join) in [(1, true), (2, false), (3, true)] {
        sequence(&mut app, " m");
        assert!(
            matches!(&app.overlay, Some(Overlay::Membership { target: MembershipTarget::User(name), join: action }) if name == "blackvault" && *action == join)
        );
        press(&mut app, KeyCode::Enter);
        while app.membership.is_some() {
            app.on_msg(
                messages
                    .recv_timeout(Duration::from_secs(5))
                    .expect("follow response"),
            );
        }
        assert_eq!(writes.load(Ordering::SeqCst), expected_writes);
        assert_eq!(app.active_tab().id, tab);
        assert_eq!(app.active_tab().feed.selected, 2);
        assert_eq!(app.active_tab().feed.offset, 1);
        assert_eq!(app.active_tab().feed.posts.len(), 3);
        assert_eq!(app.post_view.as_ref().unwrap().scroll, 7);
        assert_eq!(app.post_view.as_ref().unwrap().shown_post().id, "c");
        assert_eq!(
            app.post_view
                .as_ref()
                .unwrap()
                .shown_post()
                .media
                .preview_image()
                .as_deref(),
            Some("https://i.redd.it/c.png")
        );
        assert_eq!(
            app.subscribed(&MembershipTarget::User("blackvault".into())),
            Some(join)
        );
        assert_eq!(app.joined("blackvault"), Some(true));
        let (_, following, opened) = app.sidebar_tabs();
        assert_eq!(following.contains(&app.active), join);
        assert_eq!(opened.contains(&app.active), !join);
    }
}

#[test]
fn a_removed_follow_should_keep_its_open_tab_under_opened_and_preserve_selection() {
    let mut app = app_with(&[]);
    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok(vec![Subscription::User {
            name: "blackvault".into(),
            icon: None,
        }]),
    });
    app.open_user("blackvault");
    let id = app.active_tab().id;
    app.on_msg(Msg::Subscriptions {
        generation: 0,
        result: Ok(vec![]),
    });
    let (_, following, opened) = app.sidebar_tabs();
    assert!(following.is_empty());
    assert_eq!(opened, [app.active]);
    assert_eq!(app.active_tab().id, id);
    assert!(matches!(
        app.active_tab().kind,
        TabKind::User {
            followed: false,
            ..
        }
    ));
}
