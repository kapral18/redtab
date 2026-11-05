use super::*;

#[test]
fn when_a_cached_youtube_link_has_no_reddit_thumbnail_it_should_have_a_public_poster() {
    let media: Media = serde_json::from_value(serde_json::json!({"Link": {
        "url":"https://www.youtube.com/watch?v=n5l_E770Ksw", "thumb":null
    }}))
    .unwrap();

    assert_eq!(
        media.preview_image().as_deref(),
        Some("https://i.ytimg.com/vi/n5l_E770Ksw/hqdefault.jpg")
    );
    assert_eq!(
        media.list_thumb().as_deref(),
        Some("https://i.ytimg.com/vi/n5l_E770Ksw/hqdefault.jpg")
    );
}

#[test]
fn when_a_youtube_link_is_loaded_it_should_have_inline_video_and_poster_semantics() {
    let media = Media::Link {
        url: "https://youtu.be/dQw4w9WgXcQ".into(),
        thumb: Some("https://external-preview.redd.it/poster.jpg".into()),
    };

    assert_eq!(media.video_url(), Some("https://youtu.be/dQw4w9WgXcQ"));
    assert_eq!(media.tag().as_deref(), Some("YouTube"));
    assert_eq!(
        media.preview_image().as_deref(),
        Some("https://external-preview.redd.it/poster.jpg")
    );
    assert!(media.shows_inline());
}

#[test]
fn when_youtube_urls_are_classified_only_supported_video_shapes_should_match() {
    for url in [
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ&feature=share",
        "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
        "https://youtu.be/dQw4w9WgXcQ",
        "https://www.youtube.com/shorts/dQw4w9WgXcQ",
        "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
    ] {
        assert_eq!(youtube_id(url).as_deref(), Some("dQw4w9WgXcQ"), "{url}");
    }
    for url in [
        "",
        "https://youtube.com/playlist?list=foo",
        "https://youtube.com/@channel",
        "https://youtube.com/watch?v=short",
        "https://youtube.com/watch?v=dQw4w9WgXcQ&v=abcdefghijk",
        "https://youtube.com.attacker.test/watch?v=dQw4w9WgXcQ",
        "https://user:secret@youtube.com/watch?v=dQw4w9WgXcQ",
        "https://youtube.com:444/watch?v=dQw4w9WgXcQ",
        "file:///youtube.com/watch?v=dQw4w9WgXcQ",
    ] {
        assert_eq!(youtube_id(url), None, "{url}");
    }
}

#[test]
fn when_a_saved_youtube_link_is_deserialized_it_should_gain_playback_without_rewriting_storage() {
    let stored = serde_json::json!({"Link":{"url":"https://www.youtube.com/shorts/dQw4w9WgXcQ","thumb":null}});
    let media: Media = serde_json::from_value(stored.clone()).unwrap();

    assert!(media.shows_inline());
    assert_eq!(serde_json::to_value(media).unwrap(), stored);
}

#[test]
fn card_media_saved_video_without_a_gif_flag_should_still_be_a_video() {
    let actual: Media =
        serde_json::from_str(r#"{"Video":{"url":"https://v.redd.it/a.mp4","poster":null}}"#)
            .unwrap();
    assert_eq!(actual.tag().as_deref(), Some("video"));
}

#[test]
fn when_image_urls_have_gif_extensions_they_should_play_as_video() {
    for (url, expected) in [
        ("https://i.redd.it/a.GIF", Some("https://i.redd.it/a.GIF")),
        (
            "https://i.redd.it/a.gif?width=640&s=x",
            Some("https://i.redd.it/a.gif?width=640&s=x"),
        ),
        (
            "https://i.redd.it/a.gif#t",
            Some("https://i.redd.it/a.gif#t"),
        ),
        ("https://i.redd.it/a.gifv", None),
        ("https://i.redd.it/a.jpeg", None),
        ("https://i.redd.it/gif", None),
        ("", None),
    ] {
        let media = Media::Image {
            url: url.into(),
            preview: None,
        };

        assert_eq!(media.video_url(), expected, "{url:?}");
    }
}

#[test]
fn when_a_gif_post_lists_it_should_use_its_first_frame_as_a_still_preview() {
    let media = Media::Image {
        url: "https://i.redd.it/a.gif".into(),
        preview: None,
    };

    assert_eq!(
        media.list_thumb().as_deref(),
        Some("https://i.redd.it/a.gif")
    );
    assert_eq!(
        media.preview_image().as_deref(),
        Some("https://i.redd.it/a.gif")
    );
}

#[test]
fn a_link_post_should_list_its_preview_but_not_show_it_as_the_posts_picture() {
    let media = Media::Link {
        url: "https://example.com/article".into(),
        thumb: Some("https://external-preview.redd.it/t.png".into()),
    };

    assert_eq!(
        media.list_thumb().as_deref(),
        Some("https://external-preview.redd.it/t.png")
    );
    assert_eq!(media.preview_image(), None);
    assert!(!media.shows_inline());
}

#[test]
fn when_a_saved_image_has_no_preview_metadata_it_should_keep_its_original() {
    let actual: Media =
        serde_json::from_str(r#"{"Image":{"url":"https://i.redd.it/old.png"}}"#).unwrap();

    assert_eq!(
        actual.preview_image().as_deref(),
        Some("https://i.redd.it/old.png")
    );
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::json!({"Image":{"url":"https://i.redd.it/old.png"}})
    );
}

#[test]
fn when_a_gif_has_a_still_preview_playback_should_keep_the_original() {
    let actual = Media::Image {
        url: "https://i.redd.it/movie.gif".into(),
        preview: Some("https://preview.redd.it/movie.png".into()),
    };

    assert_eq!(actual.video_url(), Some("https://i.redd.it/movie.gif"));
    assert_eq!(
        actual.preview_image().as_deref(),
        Some("https://preview.redd.it/movie.png")
    );
    assert_eq!(
        actual.list_thumb().as_deref(),
        Some("https://preview.redd.it/movie.png")
    );
}

#[test]
fn gallery_media_saved_before_thumbnails_still_loads() {
    // Bookmarks in state.json store a post's media in this shape.
    let old: Media = serde_json::from_str(r#"{"Gallery":{"images":[]}}"#).unwrap();
    assert_eq!(
        old,
        Media::Gallery {
            images: Vec::new(),
            previews: Vec::new(),
            thumb: None,
        }
    );

    let new = Media::Gallery {
        images: Vec::new(),
        previews: Vec::new(),
        thumb: Some("https://preview.redd.it/a.jpg".into()),
    };
    let json = serde_json::to_string(&new).unwrap();
    assert_eq!(serde_json::from_str::<Media>(&json).unwrap(), new);
    assert_eq!(
        new.list_thumb().as_deref(),
        Some("https://preview.redd.it/a.jpg")
    );
}
