//! imux as a library: everything lives here and `main.rs` only reads the command line. The split
//! exists so that the fuzz targets under `fuzz/` (Phase 7) can call the parsers that read bytes
//! from panes, from the socket and from files.

// The modules are public for the fuzz targets and `main.rs`, not as a stable API: these two lints
// only ask for API polish (an `is_empty` beside `len`, `next` not named like the iterator's).
#![allow(clippy::len_without_is_empty, clippy::should_implement_trait)]

pub mod buffers;
pub mod cli;
pub mod client;
pub mod completions;
pub mod config;
pub mod copymode;
pub mod errcode;
pub mod fuzzy;
pub mod image;
pub mod init;
pub mod input;
pub mod jsonedit;
pub mod log;
pub mod paths;
pub mod persist;
pub mod plugins;
pub mod proto;
pub mod render;
pub mod sanitize;
pub mod schema;
pub mod server;
pub mod session;
pub mod shell;
pub mod setup;
pub mod skill;
pub mod target;
pub mod term;
pub mod tty;
pub mod worktree;
