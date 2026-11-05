//! Key bindings: overlays first, then the focused pane.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

use crate::app::{App, Focus, Overlay, PickerState, TabKind};
use crate::model::Sort;

pub mod map;
pub use map::State;

mod actions;
mod dispatch;
mod hints;
mod input;
mod overlay;

use actions::*;
use dispatch::*;
pub use hints::*;
pub use input::*;
use overlay::*;

pub fn handle(app: &mut App, key: KeyEvent) {
    dispatch(app, key);
    app.settle_jump();
}
