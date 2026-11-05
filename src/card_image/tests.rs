use super::*;

mod encode;
mod kitty_video_uploads;
mod media_canvas_fitting;
mod native_stills;
mod overlay;
mod tiles;
mod video_batch_publishing;
mod video_pixel_sampling;

fn picker() -> Picker {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(1, 1));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Iterm2);
    picker
}

fn complete(renderer: &mut Renderer) {
    while renderer.pending() {
        renderer.wait_for_completion();
        renderer.poll();
    }
}

fn decoded(protocol: &Tile) -> image::RgbaImage {
    use base64::Engine;
    let Tile::Image(Protocol::ITerm2(protocol)) = protocol else {
        panic!("iTerm2 tile");
    };
    let encoded = protocol.data.split("doNotMoveCursor=1:").nth(1).unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.split('\u{7}').next().unwrap())
        .unwrap();
    image::load_from_memory(&bytes).unwrap().to_rgba8()
}

fn pixel(protocol: &Tile) -> image::Rgba<u8> {
    *decoded(protocol).get_pixel(0, 0)
}

fn indicator_pixels(indicator: Option<Indicator>, top: u16, rows: u16) -> image::RgbaImage {
    let picker = picker();
    let source = Arc::new(DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        200,
        100,
        image::Rgba([75, 130, 190, 255]),
    )));
    let mut key = Key::new(
        "preview",
        &picker,
        Size::new(200, 100),
        (200, 100),
        top,
        rows,
        0,
    );
    key.overlay = indicator.map(|indicator| Overlay {
        indicator,
        top,
        rows,
    });
    let mut renderer = Renderer::default();
    renderer.begin();
    renderer.tile(key.clone(), &source, &picker, Preparation::Refresh);
    renderer.finish();
    complete(&mut renderer);
    decoded(
        &renderer
            .tile(key, &source, &picker, Preparation::Refresh)
            .unwrap()
            .unwrap(),
    )
}

fn kitty_video_tile() -> Tile {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let image = Arc::new(DynamicImage::new_rgb8(160, 90));
    let mut key = Key::new("video", &picker, Size::new(68, 20), (320, 180), 0, 16, 0);
    key.video = true;
    Encoder::default()
        .encode(&Request {
            key,
            image,
            picker,
            buffer: false,
        })
        .unwrap()
}
