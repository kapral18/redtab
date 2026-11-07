//! Reddit collections and explicit user actions, through the existing session transport.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use url::Url;
use wreq::header::HeaderValue;

use super::{Reddit, parse, text, transport};
use crate::model::{Post, PostPage};

mod actions;
mod pages;
mod requests;
mod search;
pub(in crate::reddit) use search::PATH as SEARCH_PATH;
mod types;

pub use actions::*;
pub use pages::*;
use requests::*;
pub use types::*;

#[cfg(test)]
mod tests;
