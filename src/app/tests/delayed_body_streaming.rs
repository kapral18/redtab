use super::*;

#[test]
fn when_home_has_a_three_post_primer_it_should_stream_native_cards_without_lookups() {
    assert_feed_arrives_before_tail(true, false);
}

#[test]
fn when_a_feed_body_is_delayed_it_should_show_complete_posts_before_the_tail() {
    assert_feed_arrives_before_tail(false, false);
}

#[test]
fn when_an_append_body_is_delayed_it_should_show_posts_without_finishing_pagination() {
    assert_feed_arrives_before_tail(false, true);
}
