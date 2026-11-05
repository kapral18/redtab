//! Settings editing and application; persistence belongs to config::store.
use super::{App, CardScroll, Feed, Group, Overlay, TabKind, experience::Field};
use crate::config::{
    managed::{Chezmoi, Plan, Verification},
    schema::{Kind, SETTINGS, Scope},
    store::Store,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

mod apply;
mod input;
mod preferences;
mod types;

pub use types::*;

#[cfg(test)]
mod tests;
