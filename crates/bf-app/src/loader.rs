//! Runs `bf_core::fs::read_dir` on a background thread and sends the result
//! to the main queue. A request with a host lists a remote folder over SSH
//! (`remote::list`) on the same thread.
//!
//! The first load of the app has a second path: `spawn_first` returns a
//! handle, and the main thread can take the result before the run loop
//! starts (`FirstLoad::try_take`). That never blocks. When the result is not
//! there yet, it goes to the main queue like any other load.

use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bf_core::sort::{SortOptions, sort_entries};
use bf_core::{Entry, LoadRequest};
use dispatch2::DispatchQueue;

use crate::app;
use crate::file_ops;
use crate::remote;

/// A finished load.
pub struct Delivery {
    pub request: LoadRequest,
    /// The real path of the folder: the request path, or the path that a
    /// remote host gives for `~`.
    pub dir: PathBuf,
    pub result: io::Result<Vec<Entry>>,
    /// The time to read and sort.
    pub read_time: Duration,
    /// The directory is on a local volume. Only then the app asks for
    /// path-specific icons and the title bar proxy icon.
    pub is_local: bool,
    /// The volume of the directory, for the path bar.
    pub volume: Option<file_ops::VolumeInfo>,
}

/// Reads `request.path` on a new thread. The thread sorts the entries with
/// `sort`, so that `Pane::finish_load` on the main thread sorts a list that
/// is already in order. The main queue then calls `app::finish_load`.
pub fn spawn(request: LoadRequest, sort: SortOptions) {
    run_thread(request, sort, deliver_on_main);
}

fn deliver_on_main(delivery: Delivery) {
    DispatchQueue::main().exec_async(move || app::finish_load(delivery));
}

fn run_thread(
    request: LoadRequest,
    sort: SortOptions,
    deliver: impl FnOnce(Delivery) + Send + 'static,
) {
    // The thread gets its own copy, so that a failed spawn can still report.
    let thread_request = request.clone();
    let deliver = Arc::new(Mutex::new(Some(deliver)));
    let thread_deliver = deliver.clone();
    let spawned = std::thread::Builder::new()
        .name("bf-loader".into())
        .spawn(move || {
            let started = Instant::now();
            let path = thread_request.path.clone();
            let (dir, mut result) = match &thread_request.host {
                Some(host) => match remote::list(host, &path) {
                    Ok(listing) => (listing.dir, Ok(listing.entries)),
                    Err(err) => (path, Err(err)),
                },
                None => {
                    let result = bf_core::fs::read_dir(&path);
                    (path, result)
                }
            };
            let local = thread_request.host.is_none();
            if let Ok(entries) = &mut result {
                sort_entries(entries, sort);
                // The kinds come from local files: not for remote entries.
                if local {
                    crate::columns::prefetch_kinds(entries);
                }
            }
            let read_time = started.elapsed();
            let found = local && result.is_ok();
            let is_local = found && file_ops::is_local_volume(&dir);
            let volume = found.then(|| file_ops::volume_info(&dir)).flatten();
            let delivery = Delivery {
                request: thread_request,
                dir,
                result,
                read_time,
                is_local,
                volume,
            };
            let deliver = thread_deliver
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(deliver) = deliver {
                deliver(delivery);
            }
        });
    if let Err(err) = spawned {
        let result = Err(io::Error::other(format!("cannot start a thread: {err}")));
        let deliver = deliver
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(deliver) = deliver {
            deliver(Delivery {
                dir: request.path.clone(),
                request,
                result,
                read_time: Duration::ZERO,
                is_local: false,
                volume: None,
            });
        }
    }
}

enum Slot {
    /// The thread has not finished.
    Waiting,
    /// The thread finished before the main thread asked.
    Ready(Delivery),
    /// The main thread asked too early. The thread uses the main queue.
    Abandoned,
}

/// The handle of the first load.
pub struct FirstLoad(Arc<Mutex<Slot>>);

/// Like `spawn`, but the main thread can take the result with `try_take`.
pub fn spawn_first(request: LoadRequest, sort: SortOptions) -> FirstLoad {
    let slot = Arc::new(Mutex::new(Slot::Waiting));
    let thread_slot = slot.clone();
    run_thread(request, sort, move |delivery| {
        let mut guard = thread_slot.lock().unwrap_or_else(PoisonError::into_inner);
        match *guard {
            Slot::Waiting => *guard = Slot::Ready(delivery),
            Slot::Ready(_) | Slot::Abandoned => {
                drop(guard);
                deliver_on_main(delivery);
            }
        }
    });
    FirstLoad(slot)
}

impl FirstLoad {
    /// The result when it is ready. Never waits. When it is not ready, the
    /// result goes to the main queue later.
    pub fn try_take(self) -> Option<Delivery> {
        let mut guard = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match std::mem::replace(&mut *guard, Slot::Abandoned) {
            Slot::Ready(delivery) => Some(delivery),
            Slot::Waiting | Slot::Abandoned => None,
        }
    }
}
