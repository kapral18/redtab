//! Card graphics preparation; the UI only queues work and paints completed tiles.

use ratatui::layout::{Position, Rect};
use ratatui::widgets::Widget;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, LazyLock, Weak};

use image::DynamicImage;
use parking_lot::{Condvar, Mutex};
use ratatui::layout::Size;
use ratatui_image::Resize;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;

mod encode;
mod overlay;
mod renderer;
mod tiles;

#[cfg(test)]
pub(crate) use encode::encode_tile;
use encode::*;
pub use overlay::*;
pub use renderer::*;
pub use tiles::*;

/// Kitty placeholders address at most 297 columns per image.
pub const TILE_COLS: u16 = 256;
/// Small strips let scrolling reuse the unchanged middle of the viewport.
pub const TILE_ROWS: u16 = 16;
const CACHE_BYTES: usize = 256 * 1024 * 1024;
const CACHE_TILES: usize = 512;
const NATIVE_BYTES: usize = 128 * 1024 * 1024;
const BADGE_MIN_PIXELS: f64 = 80.0;
const VIDEO_BYTES_PER_SECOND: f64 = 12.0 * 1024.0 * 1024.0;
static NEXT_IMAGE_ID: LazyLock<AtomicU32> = LazyLock::new(|| {
    let seed = *uuid::Uuid::new_v4().as_bytes();
    AtomicU32::new(u32::from_be_bytes([seed[0], seed[1], seed[2], seed[3]]))
});

#[cfg(test)]
mod tests;
