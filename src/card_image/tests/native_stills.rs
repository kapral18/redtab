use super::*;

#[test]
fn when_terminal_cells_change_a_still_strip_should_keep_its_fitted_grid_size() {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(8, 16));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let image = Arc::new(DynamicImage::new_rgb8(160, 512));
    for tmux in [false, true] {
        let mut key = Key::new(
            "portrait",
            &picker,
            Size::new(20, 32),
            (160, 512),
            16,
            16,
            0,
        );
        key.tmux = tmux;
        let tile = Encoder::default()
            .encode(&Request {
                key,
                image: Arc::clone(&image),
                picker: picker.clone(),
                buffer: false,
            })
            .unwrap();
        let Tile::KittyImage { upload, .. } = tile else {
            panic!("native still");
        };
        let header = upload
            .split("_G")
            .nth(1)
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        let values: HashMap<_, u32> = header
            .split(',')
            .filter_map(|field| {
                let (name, value) = field.split_once('=')?;
                Some((name, value.parse().ok()?))
            })
            .collect();

        // Ghostty Placement.gridSize uses c/r directly; omitted values derive
        // cells from the uploaded pixel dimensions and the current font size.
        for (cell_width, cell_height) in [(8, 16), (16, 32), (4, 8)] {
            let cols = values
                .get("c")
                .copied()
                .unwrap_or_else(|| values["s"].div_ceil(cell_width));
            let rows = values
                .get("r")
                .copied()
                .unwrap_or_else(|| values["v"].div_ceil(cell_height));
            assert_eq!(
                (cols, rows),
                (20, 16),
                "cell size {cell_width}x{cell_height}; {header}"
            );
        }
    }
}

#[test]
fn when_native_stills_are_prepared_the_upload_should_compress_the_pixels() {
    let mut picker = picker();
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let tile = Encoder::default()
        .encode(&Request {
            key: Key::new("still", &picker, Size::new(128, 16), (1, 1), 0, 16, 0),
            image: Arc::new(DynamicImage::new_rgba8(1, 1)),
            picker,
            buffer: false,
        })
        .unwrap();
    let Tile::KittyImage { upload, .. } = tile else {
        panic!("native still");
    };
    assert!(
        upload.len() < 128 * 16 * 4,
        "compress still uploads off the UI thread"
    );
}

#[test]
fn when_native_uploads_exceed_terminal_residency_a_warm_still_should_upload_again() {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(ratatui_image::FontSize::new(17, 37));
    picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
    let image = Arc::new(DynamicImage::new_rgba8(1, 1));
    let mut encoder = Encoder::default();
    let mut video_key = Key::new("paused", &picker, Size::new(128, 16), (1, 1), 0, 16, 0);
    video_key.video = true;
    let video = encoder
        .encode(&Request {
            key: video_key,
            image: Arc::clone(&image),
            picker: picker.clone(),
            buffer: false,
        })
        .unwrap();
    assert!(video.take_upload().is_some());
    assert!(video.take_upload().is_none());
    let mut first = None;
    for n in 0..28 {
        let tile = encoder
            .encode(&Request {
                key: Key::new(
                    &format!("still{n}"),
                    &picker,
                    Size::new(128, 16),
                    (1, 1),
                    0,
                    16,
                    0,
                ),
                image: Arc::clone(&image),
                picker: picker.clone(),
                buffer: false,
            })
            .unwrap();
        assert!(tile.take_upload().is_some());
        assert!(tile.take_upload().is_none());
        if first.is_none() {
            first = Some(tile);
        }
    }
    assert!(
        first.unwrap().take_upload().is_some(),
        "keep the prepared PNG, but resend IDs beyond the terminal residency budget"
    );
    assert!(
        video.take_upload().is_some(),
        "a retained paused frame must also survive terminal eviction"
    );
}

#[test]
fn clip_native_frames_should_use_lossless_png_without_outer_compression() {
    let Tile::Kitty { frame, upload, .. } = kitty_video_tile() else {
        panic!("native tile");
    };
    let crate::video::Pixels::Png(data) = &frame.pixels else {
        panic!("encoded payload");
    };
    let decoded = image::load_from_memory(data).expect("native video payload must be PNG");
    assert_eq!(
        (decoded.width(), decoded.height()),
        (frame.width, frame.height)
    );
    assert!(upload.contains("f=100,"));
    assert!(!upload.contains("o=z,"));
}

#[test]
fn native_still_ids_should_be_independent_across_processes() {
    if std::env::var_os("REDTAB_NATIVE_ID_TEST_CHILD").is_some() {
        let mut picker = picker();
        picker.set_protocol_type(ratatui_image::picker::ProtocolType::Kitty);
        let key = Key::new("same-image", &picker, Size::new(2, 2), (2, 2), 0, 2, 0);
        let encoded = Encoder::default()
            .encode(&Request {
                key,
                image: Arc::new(DynamicImage::new_rgb8(2, 2)),
                picker,
                buffer: false,
            })
            .unwrap();
        let Tile::KittyImage { id, .. } = encoded else {
            panic!("native image");
        };
        println!("NATIVE_ID={id}");
        return;
    }
    let allocate = || {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!(
                    "{}::native_still_ids_should_be_independent_across_processes",
                    module_path!().split_once("::").unwrap().1
                ),
                "--nocapture",
            ])
            .env("REDTAB_NATIVE_ID_TEST_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("NATIVE_ID=").map(str::to_owned))
            .unwrap()
    };
    assert_ne!(
        allocate(),
        allocate(),
        "two tmux panes must not start with the same native image ID"
    );
}
