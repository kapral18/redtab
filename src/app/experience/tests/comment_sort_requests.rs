use super::*;

#[test]
fn when_an_old_default_thread_arrives_after_sorting_it_should_not_replace_the_sorted_request() {
    let (mut app, _dir, _fixture) = app();
    let page = post_page();
    let post = page.post.clone();
    let link = post.permalink.clone();
    let old = app.show_post(post, &link).unwrap();
    app.sort_comments("new");
    let current = app.post_view.as_ref().unwrap().generation;
    app.on_msg(Msg::Post {
        generation: old,
        result: Ok(Box::new(page)),
    });
    let view = app.post_view.as_ref().unwrap();
    assert_eq!(view.generation, current);
    assert!(view.page.is_none());
    assert_eq!(view.comment_sort, "new");
}

#[test]
fn when_a_sorted_thread_arrives_after_reopening_the_default_view_it_should_be_ignored() {
    let (mut app, _dir, _fixture) = app();
    let page = post_page();
    let post = page.post.clone();
    let link = post.permalink.clone();
    app.show_post(post.clone(), &link);
    app.sort_comments("new");
    let old = app.post_view.as_ref().unwrap().generation;
    let current = app.show_post(post, &link).unwrap();
    app.on_msg(Msg::SortedPost {
        generation: old,
        sort: "new".into(),
        result: Ok(Box::new(page)),
    });
    let view = app.post_view.as_ref().unwrap();
    assert_eq!(view.generation, current);
    assert!(view.page.is_none());
}

#[test]
fn when_comment_sort_changes_existing_comments_should_be_replaced_by_an_uncached_sorted_request() {
    let (mut app, _dir, _fixture) = app();
    let page = post_page();
    let post = page.post.clone();
    let link = post.permalink.clone();
    let generation = app.show_post(post, &link).unwrap();
    app.on_msg(Msg::Post {
        generation,
        result: Ok(Box::new(page)),
    });
    app.sort_comments("new");
    let view = app.post_view.as_ref().unwrap();
    assert_eq!(view.comment_sort, "new");
    assert!(view.page.is_none());
    assert!(view.loading);
    assert_ne!(view.generation, generation);
}
