use super::*;

#[test]
fn tab_on_the_search_tab_should_switch_the_kind_only_from_its_list() {
    let mut app = app_with(&["a"]);
    app.switch_tab(SEARCH_TAB);
    press(&mut app, crossterm::event::KeyCode::Tab);
    assert_eq!(app.active, SEARCH_TAB);
    assert_eq!(
        app.search.kind,
        crate::reddit::experience::SearchKind::Communities
    );

    app.ancestor_follow = Some(AncestorFollow {
        tab: app.tabs[SEARCH_TAB].id,
        group: None,
        waiting: true,
    });
    press(&mut app, crossterm::event::KeyCode::Tab);
    assert_ne!(
        app.active, SEARCH_TAB,
        "following posts across tabs passes Search"
    );

    app.switch_tab(SEARCH_TAB);
    app.focus_sidebar();
    press(&mut app, crossterm::event::KeyCode::Tab);
    assert_ne!(app.active, SEARCH_TAB, "the sidebar keeps tab cycling");
}
