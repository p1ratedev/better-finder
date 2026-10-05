//! The state of one file list: directory, entries, cursor, history, filter.
//!
//! The pane does no disk IO. A navigation method returns a `LoadRequest`.
//! The UI runs `fs::read_dir` on a background thread and then calls
//! `Pane::finish_load` on the main thread. The pane changes directory only
//! when a load succeeds. When a load fails, the pane stays where it is and
//! `error()` returns the message.
//!
//! Each `LoadRequest` has a generation number. `finish_load` ignores a result
//! with an old generation (the user navigated again before it arrived).
//!
//! # Cursor rules
//!
//! - Before the pane leaves a directory, it stores the name under the cursor
//!   for that directory.
//! - Enter a directory, back, forward: the cursor goes to the stored name for
//!   that directory, or to the first entry.
//! - Parent: the cursor goes to the directory that you came from.
//! - Refresh, sort change, hidden toggle, filter change: the cursor stays on
//!   the same name when it is still visible. If not, the cursor index stays
//!   the same, clamped to the list.
//! - A directory change clears the filter.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::filter::FilterMode;
use crate::fs::Entry;
use crate::sort::SortOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadKind {
    /// A new directory. On success, push the old directory on the back stack
    /// and clear the forward stack.
    Navigate,
    /// To the parent. Like `Navigate`, and the cursor goes to `child`.
    Parent,
    Back,
    Forward,
    /// The same directory again.
    Refresh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadRequest {
    pub path: PathBuf,
    pub kind: LoadKind,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenOutcome {
    /// The entry is a directory: load it.
    Navigate(LoadRequest),
    /// The entry is a file or a package: the UI opens it with its default app.
    OpenFile(PathBuf),
    /// The list is empty.
    Nothing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneOptions {
    pub show_hidden: bool,
    pub sort: SortOptions,
    pub filter_mode: FilterMode,
}

#[derive(Debug, Clone)]
pub struct Pane {
    /// The directory on screen. Before the first load succeeds, the start directory.
    cwd: PathBuf,
    /// All entries of `cwd`, sorted, hidden entries included.
    entries: Vec<Entry>,
    /// Indices into `entries` after the hidden rule and the filter.
    visible: Vec<usize>,
    /// Index into `visible`.
    cursor: usize,
    options: PaneOptions,
    filter: String,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    /// Directory to the name under the cursor when the pane left it.
    cursor_memory: HashMap<PathBuf, String>,
    /// The request that is in progress, if any.
    pending: Option<LoadRequest>,
    /// Name to put the cursor on when the pending parent load completes.
    pending_child: Option<String>,
    generation: u64,
    error: Option<String>,
    loaded: bool,
}

impl Pane {
    /// A pane for `start`. Returns the first load request for `start`.
    pub fn new(start: PathBuf, options: PaneOptions) -> (Pane, LoadRequest) {
        let _ = (start, options);
        todo!()
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// True after the first load succeeded.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn is_loading(&self) -> bool {
        self.pending.is_some()
    }

    /// The message of the most recent failed load or operation. A successful
    /// load clears it.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn set_error(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    // ---- Entries and cursor ----

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub fn visible_entry(&self, index: usize) -> Option<&Entry> {
        self.visible.get(index).map(|&i| &self.entries[i])
    }

    /// (visible entries, all entries, hidden entries) for the status bar.
    pub fn counts(&self) -> (usize, usize, usize) {
        todo!()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The entry under the cursor.
    pub fn current(&self) -> Option<&Entry> {
        self.visible_entry(self.cursor)
    }

    /// Clamps to the list.
    pub fn set_cursor(&mut self, index: usize) {
        let _ = index;
        todo!()
    }

    /// Moves by `delta` rows. Clamps to the list (no wrap).
    pub fn move_cursor(&mut self, delta: isize) {
        let _ = delta;
        todo!()
    }

    pub fn cursor_top(&mut self) {
        self.set_cursor(0);
    }

    pub fn cursor_bottom(&mut self) {
        self.set_cursor(usize::MAX);
    }

    /// Moves the cursor to the visible entry with this exact name. Returns
    /// false when no visible entry has the name.
    pub fn select_name(&mut self, name: &str) -> bool {
        let _ = name;
        todo!()
    }

    // ---- Navigation ----

    /// A load of `path` (already absolute and normalized).
    pub fn navigate_to(&mut self, path: PathBuf) -> LoadRequest {
        let _ = path;
        todo!()
    }

    /// See `OpenOutcome`.
    pub fn open_current(&mut self) -> OpenOutcome {
        todo!()
    }

    /// `None` at `/`.
    pub fn parent(&mut self) -> Option<LoadRequest> {
        todo!()
    }

    /// `None` when the back stack is empty.
    pub fn back(&mut self) -> Option<LoadRequest> {
        todo!()
    }

    /// `None` when the forward stack is empty.
    pub fn forward(&mut self) -> Option<LoadRequest> {
        todo!()
    }

    pub fn refresh(&mut self) -> LoadRequest {
        todo!()
    }

    /// Applies a load result. Returns false and changes nothing when
    /// `request` is not the pending request (an old generation). On `Err`,
    /// sets `error()` and keeps the current directory and entries.
    pub fn finish_load(&mut self, request: &LoadRequest, result: io::Result<Vec<Entry>>) -> bool {
        let _ = (request, result);
        todo!()
    }

    // ---- View options ----

    pub fn options(&self) -> PaneOptions {
        self.options
    }

    pub fn show_hidden(&self) -> bool {
        self.options.show_hidden
    }

    pub fn set_show_hidden(&mut self, show: bool) {
        let _ = show;
        todo!()
    }

    pub fn toggle_hidden(&mut self) {
        self.set_show_hidden(!self.options.show_hidden);
    }

    pub fn sort(&self) -> SortOptions {
        self.options.sort
    }

    /// Sorts `entries` again.
    pub fn set_sort(&mut self, sort: SortOptions) {
        let _ = sort;
        todo!()
    }

    pub fn set_filter_mode(&mut self, mode: FilterMode) {
        let _ = mode;
        todo!()
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn set_filter(&mut self, query: &str) {
        let _ = query;
        todo!()
    }

    pub fn clear_filter(&mut self) {
        self.set_filter("");
    }
}
