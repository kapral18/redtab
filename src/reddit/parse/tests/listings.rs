use super::*;

#[test]
fn when_listing_fields_arrive_in_either_order_streaming_should_keep_the_reader_contract() {
    let value: Value =
        serde_json::from_str(include_str!("../../../../tests/fixtures/sub.json")).unwrap();
    for input in [
        include_str!("../../../../tests/fixtures/sub.json").to_string(),
        serde_json::to_string(&value).unwrap(),
    ] {
        let mut published = Vec::new();

        let actual =
            stream_listing(input.as_bytes(), &mut |post| published.push(post.clone())).unwrap();

        assert_eq!(actual, parse_listing(&input).unwrap());
        assert_eq!(published, actual.posts);
    }
}

#[test]
fn when_listing_structure_is_invalid_the_stream_should_not_complete() {
    for input in [
        "",
        "[]",
        "{}",
        r#"{"kind":"Listing"}"#,
        r#"{"kind":"Other","data":{"children":[]}}"#,
        r#"{"data":{"children":[]},"kind":"Other"}"#,
        r#"{"kind":"Listing","data":{}}"#,
        r#"{"kind":"Listing","data":{"children":null}}"#,
        r#"{"kind":"Listing","kind":"Listing","data":{"children":[]}}"#,
        r#"{"kind":"Listing","data":{"children":[]},"data":{"children":[]}}"#,
        r#"{"kind":"Listing","data":{"children":[],"children":[]}}"#,
        r#"{"kind":"Listing","data":{"children":[],"after":null,"after":null}}"#,
        r#"{"kind":"Listing","data":{"children":[]}} trailing"#,
        r#"{"kind":"Listing","data":{"children":[{"kind":"t3","data":{"id":"a"}}]}}"#,
    ] {
        assert!(
            stream_listing(input.as_bytes(), &mut |_| {}).is_err(),
            "{input}"
        );
    }
}

#[test]
fn when_listing_media_has_dimensions_its_selected_preview_should_preserve_them() {
    for (mut fields, expected) in [
        (
            json!({"url":"https://i.redd.it/p.jpg","preview":{"images":[{"source":{"url":"https://preview.redd.it/p.jpg","width":640,"height":320}}]}}),
            Some((640, 320)),
        ),
        (
            json!({"url":"https://i.redd.it/p.gif","preview":{"images":[{"source":{"url":"https://preview.redd.it/p.gif","width":400,"height":600}}]}}),
            Some((400, 600)),
        ),
        (
            json!({"url":"https://example.com/photo","post_hint":"image","preview":{"images":[{"source":{"url":"https://preview.redd.it/p.jpg","width":640,"height":480}}]}}),
            Some((640, 480)),
        ),
        (
            json!({"gallery_data":{"items":[{"media_id":"bad"},{"media_id":"good"}]},"media_metadata":{"bad":{"s":{"u":"https://example.com/not-allowed.jpg","x":1,"y":1}},"good":{"s":{"u":"https://i.redd.it/gallery.jpg","x":320,"y":640}}}}),
            Some((320, 640)),
        ),
        (
            json!({"gallery_data":{"items":[]},"thumbnail":"https://preview.redd.it/thumb.jpg","thumbnail_width":160,"thumbnail_height":90}),
            Some((160, 90)),
        ),
        (
            json!({"secure_media":{"reddit_video":{"hls_url":"https://v.redd.it/p/HLSPlaylist.m3u8","width":720,"height":1280}},"preview":{"images":[{"source":{"url":"https://preview.redd.it/poster.jpg","width":640,"height":360}}]}}),
            Some((640, 360)),
        ),
        (
            json!({"url":"https://example.com/article","preview":{"images":[{"source":{"url":"https://preview.redd.it/link.jpg","width":800,"height":400}}]}}),
            Some((800, 400)),
        ),
        (
            json!({"url":"https://example.com/article","thumbnail":"https://preview.redd.it/thumb.jpg","thumbnail_width":160,"thumbnail_height":120}),
            Some((160, 120)),
        ),
        (
            json!({"preview":{"images":[{"source":{"url":"https://preview.redd.it/wrong.jpg","width":1,"height":1}}]},"crosspost_parent_list":[{"url":"https://i.redd.it/parent.jpg","preview":{"images":[{"source":{"url":"https://preview.redd.it/parent.jpg","width":800,"height":1200}}]}}]}),
            Some((800, 1200)),
        ),
        (
            json!({"url":"https://i.redd.it/p.jpg","preview":{"images":[{"source":{"url":"https://preview.redd.it/p.jpg","width":0,"height":320}}]}}),
            None,
        ),
        (
            json!({"url":"https://i.redd.it/p.jpg","preview":{"images":[{"source":{"url":"https://preview.redd.it/p.jpg","width":4294967296u64,"height":320}}]}}),
            None,
        ),
    ] {
        fields.as_object_mut().unwrap().extend(
            json!({"id":"p","title":"preview","subreddit":"s","permalink":"/r/s/comments/p/x/"})
                .as_object()
                .unwrap()
                .clone(),
        );
        let actual = parse_listing_value(
            json!({"kind":"Listing","data":{"children":[{"kind":"t3","data":fields}]}}),
        )
        .unwrap();

        assert_eq!(actual.posts[0].preview_size, expected, "{fields}");
        assert_eq!(
            serde_json::from_value::<Listing>(serde_json::to_value(&actual).unwrap()).unwrap(),
            actual
        );
    }
}
