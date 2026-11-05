mod app;
mod auth;
mod auth_browser;
mod cache;
mod card_image;
mod config;
mod keys;
mod media;
mod model;
mod reddit;
mod state;
mod terminal_output;
mod ui;
mod video;
mod worker;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event};
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use crossterm::{ExecutableCommand, QueueableCommand};
use ratatui_image::picker::Picker;

use crate::app::App;
use crate::config::Config;
use crate::reddit::{Health, Reddit};
use crate::worker::Pool;

mod cli;
mod event_loop;
mod graphics_probe;
mod terminal;

use cli::*;
use event_loop::*;
use graphics_probe::*;
use terminal::*;

const WORKER_THREADS: usize = 6;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            terminal_output::diagnostic(format_args!("Error: {error:?}"));
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod test_support {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    thread_local! {
        static DIR: tempfile::TempDir = tempfile::tempdir().unwrap();
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// A state file path no other test uses, in a directory removed when the test's thread ends.
    pub fn state_path() -> PathBuf {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        DIR.with(|dir| dir.path().join(format!("state-{n}.json")))
    }
}
