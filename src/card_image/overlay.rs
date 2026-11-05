use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Indicator {
    Video { gif: bool, play: bool },
    Link,
    Gallery(usize),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Overlay {
    pub indicator: Indicator,
    /// Full-image rows visible in this viewport, independent of the encoded tile.
    pub top: u16,
    pub rows: u16,
}

pub(super) struct OverlayGeometry {
    pub(super) width: u32,
    pub(super) top: u32,
    pub(super) height: u32,
    pub(super) font_height: u32,
}

impl Overlay {
    pub(super) fn paint(
        &self,
        image: &mut image::RgbaImage,
        origin: (u32, u32),
        geometry: OverlayGeometry,
    ) {
        let OverlayGeometry {
            width,
            top,
            height,
            font_height,
        } = geometry;
        let scale = (font_height / 9).max(1);
        let pad = 3 * scale;
        let center = (width / 2, top + height / 2);
        let radius = (width.min(height) / 5).clamp(1, 28 * scale);
        let (label, symbol) = match &self.indicator {
            Indicator::Video { gif, play } => (
                if *gif { "GIF".into() } else { "VIDEO".into() },
                play.then_some(false),
            ),
            Indicator::Link => {
                let radius = (5 * scale).min(width.min(height) / 4).max(1);
                let corner = (width.saturating_sub(pad + radius), top + pad + radius);
                paint_symbol(image, origin, corner, radius, true);
                return;
            }
            Indicator::Gallery(count) => (format!("1/{count}"), None),
        };
        if let Some(link) = symbol {
            paint_symbol(image, origin, center, radius, link);
        }
        if width >= 40 * scale && height >= 18 * scale {
            paint_label(
                image,
                origin,
                (pad, top + pad),
                &label,
                width.saturating_sub(pad * 2),
                scale,
            );
        }
    }
}

pub(super) fn paint_symbol(
    image: &mut image::RgbaImage,
    origin: (u32, u32),
    center: (u32, u32),
    radius: u32,
    link: bool,
) {
    let radius = i64::from(radius);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let dx = i64::from(origin.0 + x) - i64::from(center.0);
        let dy = i64::from(origin.1 + y) - i64::from(center.1);
        if dx * dx + dy * dy > radius * radius {
            continue;
        }
        let stroke = (radius / 9).max(1);
        let mark = if link {
            let a = radius / 2;
            ((dx + dy).abs() <= stroke && dx.abs() <= a && dy.abs() <= a)
                || ((dy + a).abs() <= stroke && dx >= 0 && dx <= a)
                || ((dx - a).abs() <= stroke && dy <= 0 && dy >= -a)
                || ((dx + a).abs() <= stroke && dy >= -a / 2 && dy <= a)
                || ((dy - a).abs() <= stroke && dx >= -a && dx <= a / 2)
        } else {
            dx >= -radius / 4 && dx <= radius / 2 && dy.abs() <= (radius / 2 - dx) * 2 / 3
        };
        *pixel = image::Rgba(if mark {
            [235, 237, 250, 255]
        } else {
            [17, 17, 27, 255]
        });
    }
}

pub(super) fn paint_label(
    image: &mut image::RgbaImage,
    origin: (u32, u32),
    at: (u32, u32),
    text: &str,
    width: u32,
    scale: u32,
) {
    let limit = width.saturating_sub(4 * scale) / (6 * scale);
    let mut chars: Vec<_> = text.chars().take(limit as usize).collect();
    if text.chars().count() > limit as usize && chars.len() >= 3 {
        let end = chars.len();
        chars[end - 3..].fill('.');
    }
    let cols = chars.len() as u32 * 6 * scale + 4 * scale;
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let Some(x) = (origin.0 + x).checked_sub(at.0) else {
            continue;
        };
        let Some(y) = (origin.1 + y).checked_sub(at.1) else {
            continue;
        };
        if x >= cols || y >= 11 * scale {
            continue;
        }
        *pixel = image::Rgba([17, 17, 27, 255]);
        let Some(x) = x.checked_sub(2 * scale) else {
            continue;
        };
        let Some(y) = y.checked_sub(2 * scale) else {
            continue;
        };
        let (col, row) = (x / scale, y / scale);
        if row < 7
            && col % 6 < 5
            && let Some(ch) = chars.get((col / 6) as usize)
            && glyph(*ch)[row as usize] & (1 << (4 - col % 6)) != 0
        {
            *pixel = image::Rgba([235, 237, 250, 255]);
        }
    }
}

/// Small bitmap labels remain part of the picture on kitty, sixel, iTerm2 and halfblocks.
pub(super) fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 18, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        _ => [0; 7],
    }
}
