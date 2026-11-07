use super::*;

#[test]
fn when_compressed_native_tiles_fit_the_budget_it_should_keep_them_offscreen() {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(1, 2));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let image = Arc::new(DynamicImage::new_rgba8(1, 1));
    let mut renderer = Renderer::default();
    // Half of what 16 decoded 128x32 strips would take; their PNGs are far smaller.
    renderer.cache_bytes = CacheBytes(8 * 128 * 32 * 4);
    renderer.begin();
    for n in 0..16 {
        renderer.tile(
            Key::new(
                &format!("still{n}"),
                &picker,
                Size::new(128, 16),
                (1, 1),
                0,
                16,
                0,
            ),
            &image,
            &picker,
            Preparation::Refresh,
        );
    }
    renderer.finish();
    complete(&mut renderer);
    renderer.begin();
    renderer.finish();
    assert_eq!(
        renderer.cache.len(),
        16,
        "budget the encoded payload, not an uncompressed pixel multiple"
    );
}
