//! Account collections, composition and contextual actions share one overlay.

use super::*;
use crate::reddit::experience::{
    Action, Details, Entry, Inbox, Page, Query, Request, Response, SearchKind, Target, UserFeed,
    name, reddit_url, thread_location,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

mod collections;
mod comment_tools;
mod details;
mod input;
mod links;
mod menus;
mod results;
mod types;

use collections::*;
use menus::*;
pub use types::*;

#[cfg(test)]
mod tests;
