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
//! - Refresh, sort change, hidden toggle: the cursor stays on the same name
//!   when it is still visible. If not, the cursor index stays the same,
//!   clamped to the list.
//! - Filter change: the cursor stays on the same name when it still matches.
//!   If not, the cursor goes to the first visible entry.
//! - A directory change clears the filter.
//!
//! # Remote folders
//!
//! A folder can be on a remote host (`Location::host`). The pane does not
//! care how the UI reads it. A load request has the host, and the
//! navigation keeps it: `open_current`, `parent`, and `refresh` stay on the
//! host of the current folder, and back and forward use the host of the
//! history entry. `navigate_to` always goes to a local folder. The remote
//! host can resolve `~` in a request path, so `finish_load_at` takes the
//! real path of the folder.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::filter::FilterMode;
use crate::fs::Entry;
use crate::sort::SortOptions;

/// A folder: on this Mac, or on a remote host over SSH.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Location {
    /// The SSH destination (`[user@]host`) of a remote folder. `None` for a
    /// local folder.
    pub host: Option<String>,
    pub path: PathBuf,
}

impl Location {
    pub fn local(path: impl Into<PathBuf>) -> Location {
        Location {
            host: None,
            path: path.into(),
        }
    }

    pub fn remote(host: impl Into<String>, path: impl Into<PathBuf>) -> Location {
        Location {
            host: Some(host.into()),
            path: path.into(),
        }
    }

    pub fn is_remote(&self) -> bool {
        self.host.is_some()
    }

    /// The text for titles and messages: `~/x` for a local folder, and
    /// `host:/x` for a remote folder.
    pub fn display(&self) -> String {
        match &self.host {
            Some(host) => crate::remote::display(host, &self.path),
            None => crate::fs::display_path(&self.path),
        }
    }
}

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
    /// The folder to read. A remote path can be `~` or `~/x`.
    pub path: PathBuf,
    /// The SSH destination for a remote folder, else `None`.
    pub host: Option<String>,
    pub kind: LoadKind,
    pub generation: u64,
}

impl LoadRequest {
    pub fn location(&self) -> Location {
        Location {
            host: self.host.clone(),
            path: self.path.clone(),
        }
    }
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
    /// The SSH destination when `cwd` is on a remote host.
    host: Option<String>,
    /// All entries of `cwd`, sorted, hidden entries included.
    entries: Vec<Entry>,
    /// Indices into `entries` after the hidden rule and the filter.
    visible: Vec<usize>,
    /// Index into `visible`.
    cursor: usize,
    options: PaneOptions,
    filter: String,
    back: Vec<Location>,
    forward: Vec<Location>,
    /// Directory to the name under the cursor when the pane left it.
    cursor_memory: HashMap<Location, String>,
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
        let request = LoadRequest {
            path: start.clone(),
            host: None,
            kind: LoadKind::Navigate,
            generation: 1,
        };
        let pane = Pane {
            cwd: start,
            host: None,
            entries: Vec::new(),
            visible: Vec::new(),
            cursor: 0,
            options,
            filter: String::new(),
            back: Vec::new(),
            forward: Vec::new(),
            cursor_memory: HashMap::new(),
            pending: Some(request.clone()),
            pending_child: None,
            generation: 1,
            error: None,
            loaded: false,
        };
        (pane, request)
    }

    /// The path of the directory on screen. For a remote folder, it is a
    /// path on the remote host: see `host`.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The SSH destination when the directory is on a remote host.
    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    pub fn is_remote(&self) -> bool {
        self.host.is_some()
    }

    /// The directory on screen with its host.
    pub fn location(&self) -> Location {
        Location {
            host: self.host.clone(),
            path: self.cwd.clone(),
        }
    }

    /// The directory on screen when it is a local folder, else `None`.
    pub fn local_cwd(&self) -> Option<&Path> {
        self.host.is_none().then_some(self.cwd.as_path())
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
        let hidden = self.entries.iter().filter(|e| e.hidden).count();
        (self.visible.len(), self.entries.len(), hidden)
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
        self.cursor = index.min(self.visible.len().saturating_sub(1));
    }

    /// Moves by `delta` rows. Clamps to the list (no wrap).
    pub fn move_cursor(&mut self, delta: isize) {
        self.set_cursor(self.cursor.saturating_add_signed(delta));
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
        match self
            .visible
            .iter()
            .position(|&i| self.entries[i].name == name)
        {
            Some(index) => {
                self.cursor = index;
                true
            }
            None => false,
        }
    }

    // ---- Navigation ----

    /// A load of the local folder `path` (already absolute and normalized).
    pub fn navigate_to(&mut self, path: PathBuf) -> LoadRequest {
        self.navigate(Location::local(path))
    }

    /// A load of the folder `path` on the remote host `host`. The path is
    /// absolute and normalized, or `~` or `~/x`.
    pub fn navigate_remote(&mut self, host: String, path: PathBuf) -> LoadRequest {
        self.navigate(Location::remote(host, path))
    }

    /// A load of `location`.
    pub fn navigate(&mut self, location: Location) -> LoadRequest {
        self.request(location, LoadKind::Navigate)
    }

    /// See `OpenOutcome`. A folder on a remote host loads from the same
    /// host. For a remote file, `OpenFile` has the remote path.
    pub fn open_current(&mut self) -> OpenOutcome {
        let Some(entry) = self.current() else {
            return OpenOutcome::Nothing;
        };
        let (is_dir, path) = (entry.is_dir, entry.path.clone());
        if is_dir {
            let location = Location {
                host: self.host.clone(),
                path,
            };
            OpenOutcome::Navigate(self.navigate(location))
        } else {
            OpenOutcome::OpenFile(path)
        }
    }

    /// `None` at `/`.
    pub fn parent(&mut self) -> Option<LoadRequest> {
        let parent = self.cwd.parent()?.to_path_buf();
        let child = self
            .cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        let location = Location {
            host: self.host.clone(),
            path: parent,
        };
        let request = self.request(location, LoadKind::Parent);
        self.pending_child = child;
        Some(request)
    }

    /// True when `back` has a directory to go to.
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// True when `forward` has a directory to go to.
    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// Removes the back and forward entries in the local folder `prefix`
    /// (the folder itself and all folders in it), for example the folders
    /// of an ejected volume. Remote folders stay. Then it removes the
    /// entries that are the same as the next entry or as the current
    /// directory. So back and forward always change the directory.
    pub fn forget_history_under(&mut self, prefix: &Path) {
        let current = self.location();
        for stack in [&mut self.back, &mut self.forward] {
            stack.retain(|location| location.is_remote() || !location.path.starts_with(prefix));
            stack.dedup();
            while stack.last() == Some(&current) {
                stack.pop();
            }
        }
    }

    /// `None` when the back stack is empty.
    pub fn back(&mut self) -> Option<LoadRequest> {
        let location = self.back.last()?.clone();
        Some(self.request(location, LoadKind::Back))
    }

    /// `None` when the forward stack is empty.
    pub fn forward(&mut self) -> Option<LoadRequest> {
        let location = self.forward.last()?.clone();
        Some(self.request(location, LoadKind::Forward))
    }

    pub fn refresh(&mut self) -> LoadRequest {
        let location = self.location();
        self.request(location, LoadKind::Refresh)
    }

    /// Applies a load result of the folder `request.path`. See `finish_load_at`.
    pub fn finish_load(&mut self, request: &LoadRequest, result: io::Result<Vec<Entry>>) -> bool {
        self.finish_load_at(request, request.path.clone(), result)
    }

    /// Applies a load result. `dir` is the real path of the folder that the
    /// load read: a remote host resolves `~` (and gives `/Users/admin` for
    /// `~`). Returns false and changes nothing when `request` is not the
    /// pending request (an old generation). On `Err`, sets `error()` and
    /// keeps the current directory and entries.
    pub fn finish_load_at(
        &mut self,
        request: &LoadRequest,
        dir: PathBuf,
        result: io::Result<Vec<Entry>>,
    ) -> bool {
        if self.pending.as_ref() != Some(request) {
            return false;
        }
        self.pending = None;
        let child = self.pending_child.take();
        let mut entries = match result {
            Ok(entries) => entries,
            Err(err) => {
                let path = request.location().display();
                self.error = Some(format!("Cannot open {path}: {err}"));
                return true;
            }
        };
        self.error = None;
        crate::sort::sort_entries(&mut entries, self.options.sort);

        let new = Location {
            host: request.host.clone(),
            path: dir,
        };
        let dir_changed = !self.loaded || new != self.location();
        if dir_changed && self.loaded {
            if let Some(name) = self.current().map(|e| e.name.clone()) {
                self.cursor_memory.insert(self.location(), name);
            }
        }
        self.update_history(request.kind, &new);

        let (old_name, old_index) = (self.current().map(|e| e.name.clone()), self.cursor);
        self.cwd = new.path.clone();
        self.host = new.host.clone();
        self.entries = entries;
        self.loaded = true;
        if dir_changed {
            self.filter.clear();
            self.rebuild_visible();
            let remembered = self.cursor_memory.get(&new).cloned();
            let target = child
                .filter(|_| request.kind == LoadKind::Parent)
                .or(remembered);
            self.cursor = 0;
            if let Some(name) = target {
                self.select_name(&name);
            }
        } else {
            self.rebuild_visible();
            self.restore_cursor(old_name.as_deref(), old_index);
        }
        true
    }

    // ---- View options ----

    pub fn options(&self) -> PaneOptions {
        self.options
    }

    pub fn show_hidden(&self) -> bool {
        self.options.show_hidden
    }

    pub fn set_show_hidden(&mut self, show: bool) {
        self.options.show_hidden = show;
        self.refilter(true);
    }

    pub fn toggle_hidden(&mut self) {
        self.set_show_hidden(!self.options.show_hidden);
    }

    pub fn sort(&self) -> SortOptions {
        self.options.sort
    }

    /// Sorts `entries` again.
    pub fn set_sort(&mut self, sort: SortOptions) {
        let (name, index) = (self.current().map(|e| e.name.clone()), self.cursor);
        self.options.sort = sort;
        crate::sort::sort_entries(&mut self.entries, sort);
        self.rebuild_visible();
        self.restore_cursor(name.as_deref(), index);
    }

    pub fn set_filter_mode(&mut self, mode: FilterMode) {
        self.options.filter_mode = mode;
        self.refilter(false);
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn set_filter(&mut self, query: &str) {
        self.filter = query.to_string();
        self.refilter(false);
    }

    pub fn clear_filter(&mut self) {
        self.set_filter("");
    }

    // ---- Private helpers ----

    /// Starts a new request. It replaces the pending request.
    fn request(&mut self, location: Location, kind: LoadKind) -> LoadRequest {
        self.generation += 1;
        let request = LoadRequest {
            path: location.path,
            host: location.host,
            kind,
            generation: self.generation,
        };
        self.pending = Some(request.clone());
        self.pending_child = None;
        request
    }

    /// Changes the history stacks after a successful load of `new`.
    fn update_history(&mut self, kind: LoadKind, new: &Location) {
        let old = self.location();
        match kind {
            LoadKind::Navigate | LoadKind::Parent => {
                if self.loaded && *new != old {
                    self.back.push(old);
                }
                self.forward.clear();
            }
            LoadKind::Back => {
                self.back.pop();
                if self.loaded {
                    self.forward.push(old);
                }
            }
            LoadKind::Forward => {
                self.forward.pop();
                if self.loaded {
                    self.back.push(old);
                }
            }
            LoadKind::Refresh => {}
        }
    }

    /// Makes `visible` from the hidden rule and the filter. Clamps the cursor.
    fn rebuild_visible(&mut self) {
        let Self {
            entries,
            options,
            filter,
            ..
        } = self;
        let all_match = filter.is_empty();
        self.visible = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                (options.show_hidden || !e.hidden)
                    && (all_match || crate::filter::matches(options.filter_mode, filter, &e.name))
            })
            .map(|(i, _)| i)
            .collect();
        self.set_cursor(self.cursor);
    }

    /// Rebuilds `visible` and keeps the cursor on the same name. When the name
    /// is gone, the cursor keeps its index (`keep_index`) or goes to the top.
    fn refilter(&mut self, keep_index: bool) {
        let (name, index) = (self.current().map(|e| e.name.clone()), self.cursor);
        self.rebuild_visible();
        self.restore_cursor(name.as_deref(), if keep_index { index } else { 0 });
    }

    fn restore_cursor(&mut self, name: Option<&str>, fallback_index: usize) {
        if !name.is_some_and(|n| self.select_name(n)) {
            self.set_cursor(fallback_index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::EntryKind;

    fn entry(name: &str, is_dir: bool) -> Entry {
        Entry {
            name: name.to_string(),
            path: PathBuf::from("/t").join(name),
            kind: if is_dir {
                EntryKind::Directory
            } else {
                EntryKind::File
            },
            is_dir,
            is_package: false,
            size: 1,
            modified: None,
            created: None,
            hidden: name.starts_with('.'),
            executable: false,
        }
    }

    fn options() -> PaneOptions {
        PaneOptions {
            show_hidden: false,
            sort: SortOptions::default(),
            filter_mode: FilterMode::Substring,
        }
    }

    fn listing(names: &[&str]) -> Vec<Entry> {
        names
            .iter()
            .map(|n| entry(n, n.ends_with('/')))
            .map(|mut e| {
                e.name = e.name.trim_end_matches('/').to_string();
                e.path = PathBuf::from("/t").join(&e.name);
                e
            })
            .collect()
    }

    fn names(pane: &Pane) -> Vec<String> {
        (0..pane.visible_len())
            .map(|i| pane.visible_entry(i).unwrap().name.clone())
            .collect()
    }

    fn load(pane: &mut Pane, request: &LoadRequest, items: &[&str]) {
        assert!(pane.finish_load(request, Ok(listing(items))));
    }

    fn pane_at(start: &str, items: &[&str]) -> Pane {
        let (mut pane, req) = Pane::new(PathBuf::from(start), options());
        load(&mut pane, &req, items);
        pane
    }

    fn current_name(pane: &Pane) -> Option<&str> {
        pane.current().map(|e| e.name.as_str())
    }

    #[test]
    fn initial_load() {
        let (mut pane, req) = Pane::new(PathBuf::from("/a"), options());
        assert_eq!(
            req,
            LoadRequest {
                path: "/a".into(),
                host: None,
                kind: LoadKind::Navigate,
                generation: 1
            }
        );
        assert!(pane.is_loading() && !pane.is_loaded());
        assert_eq!(pane.cwd(), Path::new("/a"));
        load(&mut pane, &req, &["b", "a/", ".h", "c"]);
        assert!(pane.is_loaded() && !pane.is_loading());
        assert_eq!(names(&pane), ["a", "b", "c"]);
        assert_eq!(pane.counts(), (3, 4, 1));
        assert_eq!(pane.cursor(), 0);
        assert_eq!(pane.error(), None);
    }

    #[test]
    fn navigate_back_forward() {
        let mut pane = pane_at("/a", &["x/"]);
        let req = pane.navigate_to("/a/x".into());
        assert_eq!(req.generation, 2);
        assert!(pane.is_loading());
        // History does not change at request time.
        assert!(pane.back().is_none());
        let req = pane.navigate_to("/a/x".into());
        load(&mut pane, &req, &["y"]);
        assert_eq!(pane.cwd(), Path::new("/a/x"));
        assert!(pane.can_go_back() && !pane.can_go_forward());

        let req = pane.back().unwrap();
        assert_eq!(
            (req.path.as_path(), req.kind),
            (Path::new("/a"), LoadKind::Back)
        );
        // The stacks stay until the load succeeds.
        assert_eq!(pane.back.len(), 1);
        load(&mut pane, &req, &["x/"]);
        assert_eq!(pane.cwd(), Path::new("/a"));
        assert!(!pane.can_go_back() && pane.can_go_forward());
        assert!(pane.back().is_none());

        let req = pane.forward().unwrap();
        assert_eq!(req.path, Path::new("/a/x"));
        load(&mut pane, &req, &["y"]);
        assert_eq!(pane.cwd(), Path::new("/a/x"));
        assert!(pane.forward().is_none());
        assert!(pane.back().is_some());
    }

    /// Goes to `path` and completes the load.
    fn visit(pane: &mut Pane, path: &str) {
        let req = pane.navigate_to(path.into());
        load(pane, &req, &["x"]);
    }

    /// Back (or forward) with a completed load. The new directory, or
    /// `None` when that history is empty.
    fn step(pane: &mut Pane, forward: bool) -> Option<PathBuf> {
        let req = if forward { pane.forward() } else { pane.back() }?;
        load(pane, &req, &["x"]);
        Some(pane.cwd().to_path_buf())
    }

    #[test]
    fn forget_history_under_a_volume() {
        let mut pane = pane_at("/a", &["x"]);
        for path in ["/Volumes/X", "/Volumes/X2", "/Volumes/X/sub", "/b"] {
            visit(&mut pane, path);
        }
        // Back to `/a`: all the other folders are in the forward history.
        while step(&mut pane, false).is_some() {}
        assert_eq!(pane.cwd(), Path::new("/a"));
        pane.forget_history_under(Path::new("/Volumes/X"));
        // `/Volumes/X2` is another volume: it stays.
        assert_eq!(step(&mut pane, true), Some(PathBuf::from("/Volumes/X2")));
        assert_eq!(step(&mut pane, true), Some(PathBuf::from("/b")));
        assert_eq!(step(&mut pane, true), None);
        assert_eq!(step(&mut pane, false), Some(PathBuf::from("/Volumes/X2")));
        assert_eq!(step(&mut pane, false), Some(PathBuf::from("/a")));
    }

    #[test]
    fn forget_history_removes_repeats_and_the_current_directory() {
        let mut pane = pane_at("/a", &["x"]);
        for path in ["/Volumes/X", "/a", "/home", "/Volumes/X/sub", "/home"] {
            visit(&mut pane, path);
        }
        pane.forget_history_under(Path::new("/Volumes/X"));
        // Without the volume, the back history is `/a`, `/a`, `/home`, and
        // `/home` is the current directory. Each back step must change the
        // directory, so only `/a` stays.
        assert_eq!(step(&mut pane, false), Some(PathBuf::from("/a")));
        assert!(!pane.can_go_back());
    }

    #[test]
    fn navigate_clears_forward() {
        let mut pane = pane_at("/a", &[]);
        let r = pane.navigate_to("/b".into());
        load(&mut pane, &r, &[]);
        let r = pane.back().unwrap();
        load(&mut pane, &r, &[]);
        assert!(pane.forward.len() == 1);
        let r = pane.navigate_to("/c".into());
        load(&mut pane, &r, &[]);
        assert!(pane.forward().is_none());
        assert_eq!(pane.back, [Location::local("/a")]);
    }

    #[test]
    fn navigate_to_same_dir_does_not_push_history() {
        let mut pane = pane_at("/a", &["f"]);
        let r = pane.navigate_to("/a".into());
        load(&mut pane, &r, &["f"]);
        assert!(pane.back().is_none());
    }

    #[test]
    fn parent_puts_cursor_on_child() {
        let mut pane = pane_at("/a/b", &["f"]);
        let req = pane.parent().unwrap();
        assert_eq!(
            (req.path.as_path(), req.kind),
            (Path::new("/a"), LoadKind::Parent)
        );
        load(&mut pane, &req, &["a1", "b", "c"]);
        assert_eq!(current_name(&pane), Some("b"));
        assert_eq!(pane.back, [Location::local("/a/b")]);
        // Back goes to the child.
        let req = pane.back().unwrap();
        load(&mut pane, &req, &["f"]);
        assert_eq!(pane.cwd(), Path::new("/a/b"));
    }

    #[test]
    fn parent_at_root_is_none() {
        let mut pane = pane_at("/", &["a"]);
        assert!(pane.parent().is_none());
    }

    #[test]
    fn cursor_memory_on_reenter() {
        let mut pane = pane_at("/t", &["d1/", "d2/", "d3/"]);
        pane.set_cursor(2);
        let r = pane.navigate_to("/t/d3".into());
        load(&mut pane, &r, &["x", "y"]);
        pane.set_cursor(1);
        // Go up: the cursor lands on d3. The memory of /a/d3 is "y".
        let r = pane.parent().unwrap();
        load(&mut pane, &r, &["d1/", "d2/", "d3/"]);
        assert_eq!(current_name(&pane), Some("d3"));
        // Move away and enter d3 again with open_current.
        pane.set_cursor(0);
        pane.set_cursor(2);
        let OpenOutcome::Navigate(r) = pane.open_current() else {
            panic!("expected navigate")
        };
        load(&mut pane, &r, &["x", "y"]);
        assert_eq!(current_name(&pane), Some("y"));
    }

    #[test]
    fn stale_generation_ignored() {
        let mut pane = pane_at("/a", &["x"]);
        let old = pane.navigate_to("/b".into());
        let new = pane.navigate_to("/c".into());
        assert!(!pane.finish_load(&old, Ok(listing(&["from_b"]))));
        assert_eq!(pane.cwd(), Path::new("/a"));
        assert_eq!(names(&pane), ["x"]);
        assert!(pane.is_loading());
        // A request that equals the pending one in path but not in generation is stale.
        let fake = LoadRequest {
            generation: new.generation + 1,
            ..new.clone()
        };
        assert!(!pane.finish_load(&fake, Ok(vec![])));
        assert!(pane.finish_load(&new, Ok(listing(&["from_c"]))));
        assert_eq!(pane.cwd(), Path::new("/c"));
        // The same request twice: the second call is stale.
        assert!(!pane.finish_load(&new, Ok(vec![])));
        assert_eq!(names(&pane), ["from_c"]);
    }

    #[test]
    fn failed_load_keeps_state_and_sets_error() {
        let mut pane = pane_at("/a", &["x", "y"]);
        pane.set_cursor(1);
        let r = pane.navigate_to("/nope".into());
        let err = io::Error::new(io::ErrorKind::NotFound, "gone");
        assert!(pane.finish_load(&r, Err(err)));
        assert_eq!(pane.error(), Some("Cannot open /nope: gone"));
        assert_eq!(pane.cwd(), Path::new("/a"));
        assert_eq!(names(&pane), ["x", "y"]);
        assert_eq!(pane.cursor(), 1);
        assert!(!pane.is_loading());
        assert!(pane.back().is_none());
        // A good load clears the error.
        let r = pane.refresh();
        load(&mut pane, &r, &["x", "y"]);
        assert_eq!(pane.error(), None);
    }

    #[test]
    fn failed_back_keeps_stacks() {
        let mut pane = pane_at("/a", &[]);
        let r = pane.navigate_to("/b".into());
        load(&mut pane, &r, &[]);
        let r = pane.back().unwrap();
        let err = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
        assert!(pane.finish_load(&r, Err(err)));
        assert_eq!(pane.cwd(), Path::new("/b"));
        assert_eq!(pane.back, [Location::local("/a")]);
        assert!(pane.forward.is_empty());
        assert!(pane.error().is_some());
        assert!(pane.back().is_some());
    }

    #[test]
    fn failed_first_load() {
        let (mut pane, req) = Pane::new(PathBuf::from("/nope"), options());
        let err = io::Error::new(io::ErrorKind::NotFound, "gone");
        assert!(pane.finish_load(&req, Err(err)));
        assert!(!pane.is_loaded());
        assert!(pane.error().is_some());
    }

    #[test]
    fn hidden_toggle_keeps_cursor_name() {
        let mut pane = pane_at("/a", &[".a", "b", "c"]);
        assert_eq!(names(&pane), ["b", "c"]);
        pane.set_cursor(1);
        pane.toggle_hidden();
        assert_eq!(names(&pane), [".a", "b", "c"]);
        assert_eq!(current_name(&pane), Some("c"));
        pane.set_cursor(0);
        pane.toggle_hidden();
        // The name is gone: the index stays, clamped.
        assert_eq!(names(&pane), ["b", "c"]);
        assert_eq!(pane.cursor(), 0);
        pane.toggle_hidden();
        pane.set_cursor(2);
        pane.toggle_hidden();
        assert_eq!(current_name(&pane), Some("c"));
    }

    #[test]
    fn hidden_toggle_index_clamps() {
        let mut pane = pane_at("/a", &[".a", ".b", "c"]);
        pane.set_show_hidden(true);
        pane.set_cursor(1);
        assert_eq!(current_name(&pane), Some(".b"));
        pane.set_show_hidden(false);
        assert_eq!(names(&pane), ["c"]);
        assert_eq!(pane.cursor(), 0);
    }

    #[test]
    fn filter_moves_to_first_match_when_needed() {
        let mut pane = pane_at("/a", &["apple", "banana", "blueberry", "cherry"]);
        pane.set_cursor(2);
        pane.set_filter("b");
        assert_eq!(names(&pane), ["banana", "blueberry"]);
        // Same name still matches: the cursor stays on it.
        assert_eq!(current_name(&pane), Some("blueberry"));
        pane.set_filter("an");
        assert_eq!(names(&pane), ["banana"]);
        assert_eq!(pane.cursor(), 0);
        pane.set_filter("zzz");
        assert_eq!(pane.visible_len(), 0);
        assert_eq!(pane.cursor(), 0);
        assert!(pane.current().is_none());
        pane.clear_filter();
        assert_eq!(pane.visible_len(), 4);
        assert_eq!(pane.cursor(), 0);
        pane.set_cursor(3);
        pane.set_filter("a");
        assert_eq!(current_name(&pane), Some("apple"));
    }

    #[test]
    fn filter_mode_change() {
        let mut pane = pane_at("/a", &["readme", "other"]);
        pane.set_filter("rdm");
        assert_eq!(pane.visible_len(), 0);
        pane.set_filter_mode(FilterMode::Fuzzy);
        assert_eq!(names(&pane), ["readme"]);
        assert_eq!(pane.options().filter_mode, FilterMode::Fuzzy);
    }

    #[test]
    fn refresh_keeps_filter_and_cursor() {
        let mut pane = pane_at("/a", &["apple", "banana", "blueberry"]);
        pane.set_filter("b");
        pane.set_cursor(1);
        let r = pane.refresh();
        assert_eq!(r.kind, LoadKind::Refresh);
        load(
            &mut pane,
            &r,
            &["apple", "banana", "blueberry", "boysenberry"],
        );
        assert_eq!(pane.filter(), "b");
        assert_eq!(names(&pane), ["banana", "blueberry", "boysenberry"]);
        assert_eq!(current_name(&pane), Some("blueberry"));
        // The entry under the cursor disappears: the index stays.
        let r = pane.refresh();
        load(&mut pane, &r, &["apple", "banana", "boysenberry"]);
        assert_eq!(names(&pane), ["banana", "boysenberry"]);
        assert_eq!(pane.cursor(), 1);
        // Refresh does not touch the history.
        assert!(pane.back().is_none());
    }

    #[test]
    fn directory_change_clears_filter() {
        let mut pane = pane_at("/a", &["d/", "e"]);
        pane.set_filter("d");
        let r = pane.navigate_to("/a/d".into());
        load(&mut pane, &r, &["x"]);
        assert_eq!(pane.filter(), "");
    }

    #[test]
    fn open_current_outcomes() {
        let mut pane = pane_at("/t", &[]);
        assert_eq!(pane.open_current(), OpenOutcome::Nothing);

        let (mut pane, req) = Pane::new(PathBuf::from("/t"), options());
        let mut app = entry("Foo.app", false);
        app.is_package = true;
        assert!(pane.finish_load(
            &req,
            Ok(vec![entry("dir", true), app, entry("f.txt", false)])
        ));
        let OpenOutcome::Navigate(r) = pane.open_current() else {
            panic!("dir")
        };
        assert_eq!(r.path, Path::new("/t/dir"));
        assert_eq!(r.kind, LoadKind::Navigate);
        // Natural order: `f.txt` sorts before `Foo.app`.
        pane.set_cursor(1);
        assert_eq!(
            pane.open_current(),
            OpenOutcome::OpenFile("/t/f.txt".into())
        );
        pane.set_cursor(2);
        assert_eq!(
            pane.open_current(),
            OpenOutcome::OpenFile("/t/Foo.app".into())
        );
    }

    #[test]
    fn cursor_movement_clamps() {
        let mut pane = pane_at("/a", &["a", "b", "c"]);
        pane.move_cursor(-1);
        assert_eq!(pane.cursor(), 0);
        pane.move_cursor(2);
        assert_eq!(pane.cursor(), 2);
        pane.move_cursor(10);
        assert_eq!(pane.cursor(), 2);
        pane.cursor_top();
        assert_eq!(pane.cursor(), 0);
        pane.cursor_bottom();
        assert_eq!(pane.cursor(), 2);
        pane.move_cursor(isize::MIN);
        assert_eq!(pane.cursor(), 0);
        pane.move_cursor(isize::MAX);
        assert_eq!(pane.cursor(), 2);
        assert!(pane.select_name("b"));
        assert_eq!(pane.cursor(), 1);
        assert!(!pane.select_name("zzz"));
        assert_eq!(pane.cursor(), 1);
    }

    #[test]
    fn empty_list_cursor() {
        let mut pane = pane_at("/a", &[]);
        pane.cursor_bottom();
        pane.move_cursor(5);
        assert_eq!(pane.cursor(), 0);
        assert!(pane.current().is_none());
    }

    #[test]
    fn set_sort_keeps_cursor_name() {
        let mut pane = pane_at("/a", &["a", "b", "c"]);
        pane.set_cursor(0);
        pane.set_sort(SortOptions {
            reverse: true,
            ..SortOptions::default()
        });
        assert_eq!(names(&pane), ["c", "b", "a"]);
        assert_eq!(current_name(&pane), Some("a"));
        assert!(pane.sort().reverse);
    }

    #[test]
    fn load_sorts_with_options() {
        let (mut pane, req) = Pane::new(PathBuf::from("/a"), options());
        load(&mut pane, &req, &["file10", "file2", "dir/"]);
        assert_eq!(names(&pane), ["dir", "file2", "file10"]);
    }

    #[test]
    fn error_helpers() {
        let mut pane = pane_at("/a", &[]);
        pane.set_error("boom");
        assert_eq!(pane.error(), Some("boom"));
        pane.clear_error();
        assert_eq!(pane.error(), None);
    }

    /// A remote load: the host resolves `~` to `dir`.
    fn load_remote(pane: &mut Pane, request: &LoadRequest, dir: &str, items: &[&str]) {
        let entries = listing(items)
            .into_iter()
            .map(|mut e| {
                e.path = Path::new(dir).join(&e.name);
                e
            })
            .collect();
        assert!(pane.finish_load_at(request, PathBuf::from(dir), Ok(entries)));
    }

    #[test]
    fn remote_home_resolves_and_navigation_keeps_the_host() {
        let mut pane = pane_at("/Users/admin", &["Documents/"]);
        let req = pane.navigate_remote("admin@mini".into(), "~".into());
        assert_eq!(req.host.as_deref(), Some("admin@mini"));
        assert_eq!(req.path, PathBuf::from("~"));
        load_remote(&mut pane, &req, "/Users/admin", &["Projects/", "notes.txt"]);
        assert_eq!(pane.host(), Some("admin@mini"));
        assert_eq!(pane.cwd(), Path::new("/Users/admin"));
        assert!(pane.is_remote() && pane.local_cwd().is_none());
        assert_eq!(pane.location().display(), "admin@mini:/Users/admin");
        // The same path on another host is another folder: history grows.
        assert_eq!(pane.back, [Location::local("/Users/admin")]);

        // Enter a folder, parent, and refresh stay on the host.
        let OpenOutcome::Navigate(req) = pane.open_current() else {
            panic!("expected a folder");
        };
        assert_eq!(
            req.location(),
            Location::remote("admin@mini", "/Users/admin/Projects")
        );
        load_remote(&mut pane, &req, "/Users/admin/Projects", &["src/"]);
        let req = pane.parent().unwrap();
        assert_eq!(req.host.as_deref(), Some("admin@mini"));
        load_remote(&mut pane, &req, "/Users/admin", &["Projects/", "notes.txt"]);
        assert_eq!(current_name(&pane), Some("Projects"));
        assert_eq!(pane.refresh().host.as_deref(), Some("admin@mini"));

        // A file opens with its remote path.
        pane.set_cursor(1);
        assert_eq!(
            pane.open_current(),
            OpenOutcome::OpenFile(PathBuf::from("/Users/admin/notes.txt"))
        );
    }

    #[test]
    fn back_and_forward_cross_hosts() {
        let mut pane = pane_at("/a", &["x"]);
        let req = pane.navigate_remote("h".into(), "/a".into());
        load_remote(&mut pane, &req, "/a", &["remote-x"]);
        let req = pane.navigate_to("/b".into());
        load(&mut pane, &req, &[]);
        assert!(!pane.is_remote());
        let req = pane.back().unwrap();
        assert_eq!(req.location(), Location::remote("h", "/a"));
        load_remote(&mut pane, &req, "/a", &["remote-x"]);
        let req = pane.back().unwrap();
        assert_eq!(req.location(), Location::local("/a"));
        load(&mut pane, &req, &["x"]);
        let req = pane.forward().unwrap();
        assert_eq!(req.host.as_deref(), Some("h"));
    }

    #[test]
    fn cursor_memory_is_per_host() {
        let mut pane = pane_at("/d", &["a", "b"]);
        pane.set_cursor(1);
        let req = pane.navigate_remote("h".into(), "/d".into());
        load_remote(&mut pane, &req, "/d", &["a", "b"]);
        assert_eq!(
            current_name(&pane),
            Some("a"),
            "a new folder starts at the top"
        );
        let req = pane.back().unwrap();
        load(&mut pane, &req, &["a", "b"]);
        assert_eq!(current_name(&pane), Some("b"));
    }

    #[test]
    fn forget_history_keeps_remote_folders() {
        let mut pane = pane_at("/a", &["x"]);
        let req = pane.navigate_remote("h".into(), "/Volumes/X".into());
        load_remote(&mut pane, &req, "/Volumes/X", &[]);
        visit(&mut pane, "/Volumes/X");
        visit(&mut pane, "/b");
        pane.forget_history_under(Path::new("/Volumes/X"));
        assert_eq!(
            pane.back,
            [Location::local("/a"), Location::remote("h", "/Volumes/X")]
        );
    }

    #[test]
    fn remote_error_names_the_host() {
        let mut pane = pane_at("/a", &["x"]);
        let req = pane.navigate_remote("admin@mini".into(), "~/nope".into());
        let err = io::Error::other("no such folder");
        assert!(pane.finish_load_at(&req, PathBuf::from("~/nope"), Err(err)));
        assert_eq!(
            pane.error(),
            Some("Cannot open admin@mini:~/nope: no such folder")
        );
        assert!(!pane.is_remote());
        assert_eq!(pane.cwd(), Path::new("/a"));
    }
}
