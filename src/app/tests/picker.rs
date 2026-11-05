use super::*;

#[test]
fn picker_prefers_prefix_matches_exact_first() {
    let app = app_with(&["neovim", "sidehustle", "vim"]);

    assert_eq!(
        entry_title(&app, &app.picker_entries("neov")[0]).as_deref(),
        Some("r/neovim")
    );
    let vim = app.picker_entries("vim");
    assert_eq!(entry_title(&app, &vim[0]).as_deref(), Some("r/vim"));
    assert!(!vim.contains(&PickerEntry::Open("vim".into())));
}

#[test]
fn picker_offers_a_new_subreddit_before_containing_and_fuzzy_matches() {
    let app = app_with(&["learnrust", "sidehustle"]);

    let rust = app.picker_entries("r/rust");

    assert_eq!(rust[0], PickerEntry::Open("rust".into()));
    assert_eq!(entry_title(&app, &rust[1]).as_deref(), Some("r/learnrust"));
    assert!(
        rust.iter()
            .any(|e| entry_title(&app, e).as_deref() == Some("r/sidehustle"))
    );
}

#[test]
fn picker_never_offers_an_invalid_name() {
    let app = app_with(&[]);

    assert!(
        !app.picker_entries("no spaces")
            .iter()
            .any(|e| matches!(e, PickerEntry::Open(_)))
    );
}

#[test]
fn opening_a_subreddit_accepts_common_reddit_spellings() {
    let mut app = app_with(&[]);
    for name in ["r/tmux/", "R/rust", "/r/vim/"] {
        app.open_subreddit(name);
    }
    let titles: Vec<String> = app.tabs.iter().map(Tab::title).collect();

    assert!(app.status.as_ref().is_none_or(|s| !s.error));
    for name in ["tmux", "rust", "vim"] {
        assert!(titles.iter().any(|t| t.contains(name)), "{titles:?}");
    }
    assert!(
        app.picker_entries("r/tmux/")
            .iter()
            .any(|e| matches!(e, PickerEntry::Tab { title, .. } if title.contains("tmux")))
    );
}

#[test]
fn picker_offers_subreddits_named_like_builtin_tabs() {
    let app = app_with(&[]);

    assert!(
        app.picker_entries("r/home")
            .contains(&PickerEntry::Open("home".into()))
    );
}
