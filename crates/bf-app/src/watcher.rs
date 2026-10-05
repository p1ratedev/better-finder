//! Watches the current directory and the config files with FSEvents.
//!
//! One background thread owns the watchers, so the main thread never waits
//! for a watcher to stop or start. The main thread sends commands on a
//! channel. Events go back to the main queue after a debounce of 150 ms.

use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::app;

const DEBOUNCE: Duration = Duration::from_millis(150);

enum Command {
    /// Watch this directory (and stop the watch of the previous one).
    Directory(PathBuf),
    /// Watch these config files. A missing file is fine: the watch is on the
    /// parent directory.
    Config(Vec<PathBuf>),
}

thread_local! {
    static SENDER: RefCell<Option<Sender<Command>>> = const { RefCell::new(None) };
}

static DIRECTORY_SCHEDULED: AtomicBool = AtomicBool::new(false);
static CONFIG_SCHEDULED: AtomicBool = AtomicBool::new(false);

/// Starts the watcher thread. Call it once on the main thread.
pub fn start() {
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new()
        .name("bf-watcher".into())
        .spawn(move || run(rx));
    if spawned.is_ok() {
        SENDER.with(|s| *s.borrow_mut() = Some(tx));
    }
}

fn send(command: Command) {
    SENDER.with(|s| {
        if let Some(tx) = s.borrow().as_ref() {
            let _ = tx.send(command);
        }
    });
}

/// Watches `path` instead of the previous directory.
pub fn watch_directory(path: &Path) {
    send(Command::Directory(path.to_path_buf()));
}

/// Watches the config files `files`.
pub fn watch_config(files: Vec<PathBuf>) {
    send(Command::Config(files));
}

fn run(rx: Receiver<Command>) {
    let mut directory: Option<(RecommendedWatcher, PathBuf)> = None;
    let mut config: Option<RecommendedWatcher> = None;
    while let Ok(command) = rx.recv() {
        match command {
            Command::Directory(path) => {
                if directory.as_ref().is_some_and(|(_, p)| *p == path) {
                    continue;
                }
                // Drop the old watcher first. It stops its FSEvents stream.
                drop(directory.take());
                directory = watch_dir(&path).map(|w| (w, path));
            }
            Command::Config(files) => {
                drop(config.take());
                config = watch_files(&files);
            }
        }
    }
}

fn watch_dir(path: &Path) -> Option<RecommendedWatcher> {
    // notify panics on a path that is not valid UTF-8.
    path.to_str()?;
    let mut watcher = notify::recommended_watcher(|result: notify::Result<Event>| {
        if let Ok(event) = result {
            if !matches!(event.kind, EventKind::Access(_)) {
                schedule(&DIRECTORY_SCHEDULED, app::auto_refresh);
            }
        }
    })
    .ok()?;
    watcher.watch(path, RecursiveMode::NonRecursive).ok()?;
    Some(watcher)
}

fn watch_files(files: &[PathBuf]) -> Option<RecommendedWatcher> {
    // Each watched file as (directory, name). FSEvents reports real paths
    // (`/private/var/...`), so keep the canonical directory too.
    let mut targets: Vec<(PathBuf, OsString)> = Vec::new();
    let mut dirs: HashSet<PathBuf> = HashSet::new();
    for file in files {
        let (Some(dir), Some(name)) = (file.parent(), file.file_name()) else {
            continue;
        };
        if !dir.is_dir() || dir.to_str().is_none() {
            continue;
        }
        targets.push((dir.to_path_buf(), name.to_os_string()));
        if let Ok(real) = dir.canonicalize() {
            targets.push((real, name.to_os_string()));
        }
        dirs.insert(dir.to_path_buf());
    }
    if dirs.is_empty() {
        return None;
    }
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
        let Ok(event) = result else { return };
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        let hit = event.paths.iter().any(|p| {
            targets
                .iter()
                .any(|(dir, name)| p.parent() == Some(dir) && p.file_name() == Some(name))
        });
        if hit {
            schedule(&CONFIG_SCHEDULED, app::auto_reload_config);
        }
    })
    .ok()?;
    for dir in &dirs {
        let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
    }
    Some(watcher)
}

/// Runs `action` on the main queue after the debounce time. More events in
/// that time do not schedule it again.
fn schedule(flag: &'static AtomicBool, action: fn()) {
    if flag.swap(true, Ordering::SeqCst) {
        return;
    }
    app::after(DEBOUNCE, move || {
        flag.store(false, Ordering::SeqCst);
        action();
    });
}
