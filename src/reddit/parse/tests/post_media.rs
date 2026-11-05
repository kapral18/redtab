use super::*;

#[test]
fn when_a_crosspost_parent_is_deleted_its_body_should_label_the_original_and_keep_media() {
    let fields = json!({
        "id":"cross", "title":"Shared video", "subreddit":"mechanical_gifs",
        "permalink":"/r/mechanical_gifs/comments/cross/shared/",
        "crosspost_parent_list":[{
            "removed_by_category":"deleted", "author":"[deleted]", "title":"Original video",
            "subreddit":"oddlysatisfying", "permalink":"/r/oddlysatisfying/comments/parent/original/",
            "secure_media":{"reddit_video":{"hls_url":"https://v.redd.it/a/master.m3u8"}}
        }]
    });

    let (actual, body) = post(fields).unwrap();

    assert!(body.contains("Original post deleted"), "{body}");
    assert_eq!(
        actual.media.video_url(),
        Some("https://v.redd.it/a/master.m3u8")
    );
}

#[test]
fn media_uses_hls_gif_gallery_crosspost_and_link_previews() {
    let preview = json!({"images":[{"source":{"url":"https://preview.redd.it/post.jpg?s=x"}}]});
    let video = media(
        &json!({"secure_media":{"reddit_video":{"hls_url":"https://v.redd.it/id/HLSPlaylist.m3u8"}},"preview":preview}),
    );
    assert_eq!(
        video.video_url(),
        Some("https://v.redd.it/id/HLSPlaylist.m3u8")
    );
    assert_eq!(
        video.preview_image().as_deref(),
        Some("https://preview.redd.it/post.jpg?s=x")
    );
    assert_eq!(
        media(&json!({"url":"https://i.redd.it/anim.GIF"})).video_url(),
        Some("https://i.redd.it/anim.GIF")
    );
    assert_eq!(
        media(
            &json!({"crosspost_parent_list":[{"url":"https://i.redd.it/p.png","post_hint":"image"}]})
        ),
        Media::Image {
            url: "https://i.redd.it/p.png".into(),
            preview: None
        }
    );
    let link = media(&json!({"url":"https://example.com/article","preview":preview}));
    assert_eq!(
        link.list_thumb().as_deref(),
        Some("https://preview.redd.it/post.jpg?s=x")
    );
    assert_eq!(link.preview_image(), None);
    let external = json!({
        "url": "https://example.com/photo.jpg",
        "post_hint": "image",
        "preview": preview,
    });
    assert_eq!(
        media(&external).preview_image().as_deref(),
        Some("https://preview.redd.it/post.jpg?s=x")
    );
}

#[test]
fn when_an_image_is_huge_its_inline_preview_should_use_a_detailed_safe_resolution() {
    let fields = json!({
        "id":"large", "title":"large guide", "subreddit":"coolguides",
        "permalink":"/r/coolguides/comments/large/guide/",
        "url":"https://i.redd.it/guide.png",
        "preview":{"images":[{
            "source":{"url":"https://preview.redd.it/guide.png", "width":12000, "height":18000},
            "resolutions":[
                {"url":"https://preview.redd.it/guide.png?width=320", "width":320, "height":480},
                {"url":"https://preview.redd.it/guide.png?width=1080", "width":1080, "height":1620},
                {"url":"https://preview.redd.it/guide.png?width=2160", "width":2160, "height":3240}
            ]
        }]}
    });

    let actual: Post =
        serde_json::from_value(serde_json::to_value(post(fields).unwrap().0).unwrap()).unwrap();

    assert_eq!(
        actual.media.preview_image().as_deref(),
        Some("https://preview.redd.it/guide.png?width=1080")
    );
    assert_eq!(actual.preview_size, Some((1080, 1620)));
    assert!(
        matches!(actual.media, Media::Image { url, .. } if url == "https://i.redd.it/guide.png")
    );
}

#[test]
fn when_a_large_image_is_externally_hosted_its_original_handoff_should_keep_the_full_reddit_source()
{
    let fields = json!({
        "id":"external", "title":"external guide", "subreddit":"coolguides",
        "permalink":"/r/coolguides/comments/external/guide/", "post_hint":"image",
        "url":"https://example.com/original.png",
        "preview":{"images":[{
            "source":{"url":"https://external-preview.redd.it/full.png","width":12000,"height":18000},
            "resolutions":[{"url":"https://external-preview.redd.it/sized.png","width":1080,"height":1620}]
        }]}
    });

    let actual = post(fields).unwrap().0;

    assert_eq!(
        actual.media.preview_image().as_deref(),
        Some("https://external-preview.redd.it/sized.png")
    );
    assert_eq!(actual.preview_size, Some((1080, 1620)));
    assert!(
        matches!(actual.media, Media::Image { url, .. } if url == "https://external-preview.redd.it/full.png")
    );
}

#[test]
fn when_preview_sizes_cross_the_limit_selection_should_preserve_quality_and_reject_bad_candidates()
{
    for (width, height, candidates, expected) in [
        (
            4000,
            4000,
            json!([{"url":"https://preview.redd.it/small.png","width":800,"height":800}]),
            "https://i.redd.it/original.png",
        ),
        (
            4000,
            4001,
            json!([
                {"url":"https://preview.redd.it/small.png","width":320,"height":320},
                {"url":"https://preview.redd.it/best.png","width":800,"height":800}
            ]),
            "https://preview.redd.it/best.png",
        ),
        (
            12000,
            18000,
            json!([
                {"url":"https://example.com/untrusted.png","width":1080,"height":1620},
                {"url":"https://preview.redd.it/zero.png","width":0,"height":1620},
                {"url":"https://preview.redd.it/huge.png","width":12000,"height":18000},
                {"url":"https://preview.redd.it/overflow.png","width":4294967296u64,"height":1}
            ]),
            "https://i.redd.it/original.png",
        ),
    ] {
        let fields = json!({
            "id":"p", "title":"image", "subreddit":"s", "permalink":"/r/s/comments/p/",
            "url":"https://i.redd.it/original.png",
            "preview":{"images":[{"source":{"url":"https://preview.redd.it/original.png","width":width,"height":height},"resolutions":candidates}]}
        });

        let actual = post(fields).unwrap().0;

        assert_eq!(actual.media.preview_image().as_deref(), Some(expected));
    }
}

#[test]
fn when_a_gallery_has_invalid_entries_its_safe_previews_should_stay_with_their_originals() {
    let fields = json!({
        "id":"gallery", "title":"guides", "subreddit":"coolguides",
        "permalink":"/r/coolguides/comments/gallery/guides/",
        "gallery_data":{"items":[{"media_id":"bad"},{"media_id":"large"},{"media_id":"small"}]},
        "media_metadata":{
            "bad":{"s":{"u":"https://example.com/not-allowed.png","x":1,"y":1}},
            "large":{
                "s":{"u":"https://i.redd.it/large.png","x":12000,"y":18000},
                "p":[{"u":"https://preview.redd.it/large.png?width=1080","x":1080,"y":1620}]
            },
            "small":{"s":{"u":"https://i.redd.it/small.png","x":600,"y":400}}
        }
    });

    let actual = post(fields).unwrap().0;

    assert_eq!(
        actual.media.preview_image().as_deref(),
        Some("https://preview.redd.it/large.png?width=1080")
    );
    assert_eq!(actual.preview_size, Some((1080, 1620)));
    assert!(
        matches!(actual.media, Media::Gallery { images, .. } if images == ["https://i.redd.it/large.png", "https://i.redd.it/small.png"])
    );
}

#[test]
fn card_media_converted_reddit_gif_should_keep_its_gif_label() {
    let actual = parse_listing_value(json!({"kind":"Listing","data":{"children":[{"kind":"t3","data":{
        "id":"animated","title":"animation","subreddit":"s","permalink":"/comments/animated/",
        "secure_media":{"reddit_video":{"hls_url":"https://v.redd.it/clip/HLSPlaylist.m3u8","is_gif":true}}
    }}]}})).unwrap();
    assert_eq!(actual.posts[0].media.tag().as_deref(), Some("gif"));
}

#[test]
fn card_media_converted_preview_animation_should_keep_its_gif_label() {
    let actual = parse_listing_value(json!({"kind":"Listing","data":{"children":[{"kind":"t3","data":{
        "id":"animated","title":"animation","subreddit":"s","permalink":"/comments/animated/",
        "preview":{"images":[{"variants":{"mp4":{"source":{"url":"https://i.redd.it/animation.mp4"}}}}]}
    }}]}})).unwrap();
    assert_eq!(actual.posts[0].media.tag().as_deref(), Some("gif"));
}
