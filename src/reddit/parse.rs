//! Reddit JSON to the reader's persisted models. Bodies come from Reddit's rendered HTML
//! when present, else the raw text; see `text`.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::text;
use crate::model::{
    Comment, Listing, Media, MoreReplies, Post, PostPage, ReplyPage, Subscription, THREAD_ROOT,
};

mod comments;
mod listings;
mod post_media;
mod posts;
mod replies;

#[cfg(test)]
pub use comments::parse_post_page;
pub(in crate::reddit) use comments::*;
#[cfg(test)]
pub use listings::parse_listing;
pub(in crate::reddit) use listings::*;
use post_media::*;
pub(crate) use posts::*;
pub(crate) use replies::*;

fn id_valid(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|c| c.is_ascii_alphanumeric())
}

/// The id in `/r/<sub>/comments/<id>/…`, `/user/<name>/comments/<id>/…` or `/comments/<id>`.
pub(crate) fn post_id(permalink: &str) -> Option<&str> {
    match *permalink.split('/').collect::<Vec<_>>() {
        ["", "r" | "user" | "u", _, "comments", id, ..] | ["", "comments", id, ..] => Some(id),
        _ => None,
    }
    .filter(|id| id_valid(id))
}

#[cfg(test)]
mod tests;
