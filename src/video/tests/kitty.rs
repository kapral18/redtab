use super::*;

#[test]
fn kitty_frames_go_out_in_chunks_and_through_tmux() {
    let frame = Frame {
        width: 2,
        height: 1,
        pixels: Pixels::Png(vec![7; 5000]),
        same_as_first: false,
    };
    let plain = kitty_transmit(42, &frame, 3, 2, false);
    let escapes: Vec<&str> = plain.split("\x1b\\").filter(|s| !s.is_empty()).collect();
    assert_eq!(escapes.len(), 2, "5000 bytes are 6668 base64 characters");
    assert!(escapes[0].starts_with("\x1b_Ga=T,U=1,i=42,p=1,f=100,t=d,s=2,v=1,c=3,r=2,q=2,m=1;"));
    assert!(escapes[1].starts_with("\x1b_Gm=0;"));
    assert!(
        escapes
            .iter()
            .all(|e| e.split_once(';').unwrap().1.len() <= 4096)
    );
    let payload: String = escapes
        .iter()
        .map(|escape| escape.split_once(';').unwrap().1)
        .collect();
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .unwrap(),
        vec![7; 5000]
    );

    // Inside tmux: the escapes, ESCs doubled, in one passthrough.
    let tmux = kitty_transmit(42, &frame, 3, 2, true);
    let wrapped = format!("\x1bPtmux;{}\x1b\\", plain.replace('\x1b', "\x1b\x1b"));
    assert_eq!(tmux, wrapped);

    // A frame too big for one passthrough splits between whole escapes.
    let big = Frame {
        pixels: Pixels::Png(vec![7; 600_000]),
        ..frame
    };
    let plain = kitty_transmit(42, &big, 3, 2, false);
    let tmux = kitty_transmit(42, &big, 3, 2, true);
    let passthroughs: Vec<&str> = tmux.split_inclusive("\x1b\\\x1bPtmux;").collect();
    assert_eq!(
        passthroughs.len(),
        4,
        "800,000 base64 bytes in 256 KiB pieces"
    );
    assert!(passthroughs.iter().all(|p| p.len() <= PASSTHROUGH_MAX + 9));
    let unwrapped = tmux
        .strip_prefix("\x1bPtmux;")
        .and_then(|t| t.strip_suffix("\x1b\\"))
        .unwrap()
        .replace("\x1b\\\x1bPtmux;", "")
        .replace("\x1b\x1b", "\x1b");
    assert_eq!(unwrapped, plain);
}

#[test]
fn kitty_cells_should_encode_the_full_id_like_the_upstream_protocol() {
    use ratatui::widgets::Widget;
    let id = 0xab12_3456;
    let protocol = ratatui_image::protocol::Protocol::Kitty(
        ratatui_image::protocol::kitty::Kitty::new(
            image::DynamicImage::new_rgba8(1, 1),
            Size::new(1, 1),
            id,
            false,
            false,
        )
        .unwrap(),
    );
    let mut reference = Buffer::empty(Rect::new(0, 0, 1, 1));
    ratatui_image::Image::new(&protocol).render(reference.area, &mut reference);
    let mut actual = Buffer::empty(reference.area);
    kitty_cells(&mut actual, reference.area, id, Position::ORIGIN, None);
    let (_, suffix) = reference[(0, 0)].symbol().split_once('\u{10EEEE}').unwrap();
    assert_eq!(actual[(0, 0)].symbol(), format!("\u{10EEEE}{suffix}"));
    assert_eq!(actual[(0, 0)].fg, reference[(0, 0)].fg);
}

#[test]
fn kitty_cells_carry_the_id_the_rows_and_the_frame_once() {
    let mut buf = Buffer::empty(Rect::new(0, 0, 4, 3));
    let id = 0x0001_0203;
    kitty_cells(
        &mut buf,
        Rect::new(1, 0, 2, 2),
        id,
        Position::ORIGIN,
        Some("DATA".into()),
    );
    let cell = |x, y| buf.cell((x, y)).unwrap();

    assert_eq!(cell(1, 0).symbol(), "DATA\u{10EEEE}\u{305}\u{305}\u{305}");
    assert_eq!(cell(2, 0).symbol(), "\u{10EEEE}");
    assert_eq!(cell(1, 1).symbol(), "\u{10EEEE}\u{30D}\u{305}\u{305}");
    assert_eq!(cell(2, 1).symbol(), "\u{10EEEE}");
    for (x, y) in [(1, 0), (2, 0), (1, 1), (2, 1)] {
        assert_eq!(
            cell(x, y).fg,
            Color::Rgb(1, 2, 3),
            "every cell names the image"
        );
        assert_eq!(
            cell(x, y).diff_option,
            CellDiffOption::ForcedWidth(std::num::NonZeroU16::MIN),
            "one column each, whatever the data in it"
        );
    }
    assert_eq!(cell(0, 0).symbol(), " ", "outside the area");
    assert_eq!(cell(1, 2).symbol(), " ", "below the placement");
}
