use super::*;

#[test]
fn ui_audit_help_should_remain_discoverable_in_a_narrow_main_footer() {
    let mut app = feed_app(Picker::halfblocks());
    app.show_sidebar = false;
    app.health = Health::Up;
    let actual = screen(&mut app, 40, 16);
    assert!(actual.lines().last().unwrap().contains("F1"), "{actual}");
}

#[test]
fn narrow_help_should_wrap_descriptions_and_clamp_at_the_last_key() {
    let mut app = app(&[]);
    press(&mut app, '?');
    app.help_scroll = Some(usize::MAX);
    press(&mut app, 'j');

    let shown = screen(&mut app, 32, 12);

    assert!(shown.contains("Quit"));
    assert!(shown.contains("Quit redtab"));
    assert!(app.help_scroll.is_some_and(|s| s < usize::MAX));
}

#[test]
fn every_help_key_is_set_apart_from_its_description() {
    let mut app = app(&[]);
    press(&mut app, '?');

    let mut s = String::new();
    let mut offset = 0;
    loop {
        app.help_scroll = Some(offset);
        s.push_str(&screen(&mut app, 160, 160));
        if app.help_scroll.unwrap() < offset {
            break;
        }
        offset += 20;
    }

    for (key, desc) in help::entries(&app).iter().filter(|(_, d)| !d.is_empty()) {
        assert!(
            s.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .contains(&key.split_whitespace().collect::<Vec<_>>().join(" "))
                && s.split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .contains(desc),
            "{key:?} / {desc:?} is not shown whole:\n{s}"
        );
    }
}
