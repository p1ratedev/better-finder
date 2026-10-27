//! Core logic of Better Finder.
//!
//! This crate has no UI code and no macOS framework code. The macOS app
//! (`bf-app`) uses it. All logic that tests can cover lives here.
//!
//! The public API of this crate is a contract with `bf-app`. Do not change
//! a public signature without an update to both crates.

pub mod action;
pub mod color;
pub mod config;
pub mod drop;
pub mod filter;
pub mod format;
pub mod fs;
pub mod keymap;
pub mod names;
pub mod pane;
pub mod sort;
pub mod transfer;
pub mod viewer;
pub mod viewport;

pub use action::Action;
pub use color::Rgba;
pub use config::{Config, LoadResult};
pub use fs::{Entry, EntryKind};
pub use keymap::{KeyEvent, KeyOutcome, Keymap, KeymapState};
pub use pane::{LoadRequest, OpenOutcome, Pane};
