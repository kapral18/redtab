//! Inline video: ffmpeg decodes raw frames for the scrolling tile renderer. One
//! player owns playback; ffplay plays optional sound in its own process.

mod audio;
mod youtube;

use audio::Audio;

use std::io::{self, BufRead, BufReader};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use base64::Engine;
use parking_lot::Mutex;
use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::layout::{Position, Rect, Size};
use ratatui::style::Color;
use ratatui_image::FontSize;

mod clock;
mod controls;
mod decoder;
mod ffmpeg;
mod kitty;
mod playback_loop;
mod player;
mod sources;

pub use clock::*;
use decoder::*;
pub use ffmpeg::*;
pub use kitty::*;
pub use player::*;
use sources::*;

/// Frames per second for a source slower than this or naming no rate (a GIF follows the
/// rate ffmpeg reports for it), and wherever ffmpeg can't follow the source's rate.
pub(crate) const MIN_FPS: f64 = 24.0;
/// Faster sources are halved or resampled down to this.
const MAX_FPS: f64 = 30.0;
/// Decode at most this width; the pixel budget bounds tall frames.
const MAX_FRAME_WIDTH: u32 = 1280;
/// Pixel budget shared by decoder and tiled/native output; portrait frames receive
/// the same source detail as landscape frames.
pub(crate) const MAX_VIDEO_PIXELS: f64 = 921_600.0;
/// A kitty frame comes from a rendition at least a third larger than it, so the stream's
/// own compression shows less in the picture the terminal enlarges.
const RENDITION_HEADROOM: f64 = 0.75;
/// Frames decoded ahead of the one on screen.
const AHEAD: usize = 3;
/// A frame this late means a stall: resume from it instead of racing through late frames.
const STALL: f64 = 0.25;
const FRAME_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
/// How far `,` and `.` seek, in seconds.
pub const SEEK_STEP: f64 = 5.0;

#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Pixels,
    /// The same picture as its run's first frame (true for that frame itself).
    pub same_as_first: bool,
}

#[derive(Clone)]
pub enum Pixels {
    /// Lossless PNG, ready for the kitty protocol.
    Png(Vec<u8>),
    /// Unencoded frames use scrolling tiles on every terminal protocol.
    Raw(Arc<image::DynamicImage>),
}

#[cfg(test)]
mod tests;
