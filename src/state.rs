//! Reading history and bookmarks in `$XDG_STATE_HOME/redtab/state.json`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::cache::{Freshness, Source};
use crate::config::Config;
use crate::model::Post;

mod bookmarks;
mod load;
mod locking;
mod reads;
mod save;

pub use bookmarks::*;
use load::*;
use locking::*;
use reads::*;

/// Observations and read marks not refreshed for this long are dropped on save.
const RETENTION_SECS: i64 = 90 * 24 * 60 * 60;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListView {
    #[default]
    Card,
    Compact,
}

impl ListView {
    pub fn label(self) -> &'static str {
        match self {
            Self::Card => "Card",
            Self::Compact => "Compact",
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    list_view: ListView,
    #[serde(default)]
    list_view_changed_at: i64,
    /// Compact records from fetched listings; old visit-based `seen` marks are ignored.
    #[serde(default)]
    observed: HashMap<String, ObservedCommunity>,
    /// Post id → last refresh of its opened/read mark.
    #[serde(default)]
    read: HashMap<String, i64>,
    #[serde(default)]
    deleted_originals: HashMap<String, i64>,
    #[serde(default)]
    bookmarks: Vec<Bookmark>,
    /// Removed bookmarks: id → removal time, so merging an older copy can't resurrect them.
    #[serde(default)]
    removed: HashMap<String, i64>,
    /// Set when the file exists but cannot be read: never overwrite it.
    #[serde(skip)]
    read_only: bool,
    #[serde(skip)]
    unread_counts: RefCell<Option<UnreadCounts>>,
    #[serde(skip)]
    read_revision: u64,
}

impl State {
    /// The file was unreadable at startup, so this session never writes it.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// The state moves to another file: saving there merges it, and refuses an invalid one.
    pub fn move_to_new_file(&mut self) {
        self.read_only = false;
    }

    pub fn list_view(&self) -> ListView {
        self.list_view
    }

    pub fn toggle_list_view(&mut self) {
        self.list_view = match self.list_view {
            ListView::Card => ListView::Compact,
            ListView::Compact => ListView::Card,
        };
        self.list_view_changed_at = chrono::Utc::now()
            .timestamp_millis()
            .max(self.list_view_changed_at + 1);
    }
}

#[cfg(test)]
mod tests;
