use super::*;

#[test]
fn settings_find_should_follow_its_backing_post_for_nsfw_and_filter_changes() {
    for external in [false, true] {
        for (adult, filter, keep) in [
            (true, false, false),
            (false, false, true),
            (false, true, false),
        ] {
            let (mut app, directory, _fixture) = app();
            crate::app::tests::confirm_preferences(&mut app, true);
            let mut page = post_page();
            page.post.nsfw = adult;
            page.post.title = "needle".into();
            let generation = app
                .show_post(page.post.clone(), &page.post.permalink)
                .unwrap();
            app.on_msg(Msg::Post {
                generation,
                result: Ok(Box::new(page)),
            });
            let path = directory.path().join("config.toml");
            std::fs::write(&path, "[media]\nimage_preview=false\n").unwrap();
            app.install_settings_store(crate::config::store::Store::open(Some(&path)).unwrap());
            app.experience.screens = vec![Screen::Input(form(
                "Find in loaded comments",
                FormKind::Find,
                vec![field("Query", "needle", false)],
                "/comments/post",
            ))];
            app.overlay = Some(Overlay::Experience);
            if filter {
                if external {
                    let username = app.reddit.account().unwrap().name;
                    std::fs::write(&path, format!("[media]\nimage_preview=false\n[accounts.{username}.filters]\nkeywords=[\"needle\"]\n")).unwrap();
                    app.settings.store.select_account(Some(&username));
                    app.poll_settings(Instant::now() + Duration::from_secs(2));
                } else {
                    app.change_setting(
                        "filters.keywords",
                        toml::Value::Array(vec!["needle".into()]),
                    );
                }
            } else {
                crate::app::tests::confirm_preferences(&mut app, false);
            }
            assert_eq!(app.post_view.is_some(), keep);
            assert_eq!(
                matches!(app.overlay, Some(Overlay::Experience)),
                keep,
                "Find must close with its post: external={external} adult={adult} filter={filter}"
            );
            if keep {
                key(&mut app, KeyCode::Enter);
                assert!(matches!(
                    app.experience.screens.last(),
                    Some(Screen::Browse { .. })
                ));
            } else {
                assert!(app.experience.screens.is_empty());
            }
        }
    }
}
