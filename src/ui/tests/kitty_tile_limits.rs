use super::*;

#[test]
fn when_a_card_is_wider_than_kittys_column_limit_it_should_render_its_right_edge() {
    let mut app = card_with_image(640, 320);

    let cells = placeholder_cells(&mut app, 340, 150);

    assert_eq!(cells.iter().map(|cell| cell.0).min(), Some(34));
    assert_eq!(cells.iter().map(|cell| cell.0).max(), Some(333));
}

#[test]
fn when_scrolling_below_kittys_row_limit_image_tiles_should_use_local_coordinates() {
    let mut app = card_with_image(100, 1000);
    app.tabs[4]
        .feed
        .posts
        .extend((0..200).map(|i| post_with(&format!("tail{i}"), Media::None)));
    screen(&mut app, 100, 600);
    app.scroll_card(300);
    screen(&mut app, 100, 600);

    let mut terminal = Terminal::new(TestBackend::new(100, 600)).unwrap();
    terminal.draw(|frame| draw(frame, &mut app)).unwrap();

    let CardImageSlot::Ready {
        protocol: Some(protocol),
        ..
    } = app.card_images.get("https://i.redd.it/p1.jpeg").unwrap()
    else {
        panic!("card protocol");
    };
    assert_eq!(protocol.size, Size::new(66, 330));
    assert!(protocol.tiles.contains_key(&(298, 0, 6, 66)));
    assert_eq!(protocol.tiles.len(), 3);
    assert_eq!(protocol.tiles[&(298, 0, 6, 66)].size(), Size::new(66, 16));
    assert_eq!(protocol.tiles[&(320, 0, 10, 66)].size(), Size::new(66, 10));
    let row = terminal.backend().buffer()[(28, 4)]
        .symbol()
        .split_once('\u{10EEEE}')
        .unwrap()
        .1
        .chars()
        .next();
    assert_eq!(row, Some('\u{34B}'));
}
