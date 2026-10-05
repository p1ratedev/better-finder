//! Watches the current directory and the config files with FSEvents.
//!
//! - One FSEvents stream for the current directory, one for the directories
//!   of the config files.
//! - Directory-level events (no file events) and a latency of 0.25 s.
//!   FSEvents joins the events, so a busy tree gives few wakeups.
//! - FSEvents watches a whole tree. The callbacks keep only the events for
//!   the watched directories themselves.
//! - All stream work runs on one serial dispatch queue: create, start, stop,
//!   and the callbacks. The main thread only sends work to that queue. It
//!   never waits for a stream, and it does no disk access for the watch.
//! - Debounce on the main queue. The first event after a quiet period acts
//!   at once (FSEvents waited 0.25 s already). More events in a burst wait
//!   for a quiet time after the last event, but not more than a maximum
//!   time after the first one. A slow directory read makes both times longer.

use std::ffi::{CStr, c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2_foundation::{NSArray, NSString};

use crate::app;

// ---- FSEvents (CoreServices) ----

/// `FSEventStreamContext`.
#[repr(C)]
struct StreamContext {
    version: isize,
    info: *mut c_void,
    retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<unsafe extern "C" fn(*const c_void)>,
    copy_description: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
}

/// `FSEventStreamCallback`. Without `kFSEventStreamCreateFlagUseCFTypes`,
/// the paths are an array of C strings.
type StreamCallback = unsafe extern "C" fn(
    stream: *const c_void,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const u32,
    ids: *const u64,
);

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: *const c_void,
        callback: StreamCallback,
        context: *const StreamContext,
        paths: *const c_void,
        since_when: u64,
        latency: f64,
        flags: u32,
    ) -> *mut c_void;
    fn FSEventStreamSetDispatchQueue(stream: *mut c_void, queue: *const c_void);
    fn FSEventStreamStart(stream: *mut c_void) -> u8;
    fn FSEventStreamStop(stream: *mut c_void);
    fn FSEventStreamInvalidate(stream: *mut c_void);
    fn FSEventStreamRelease(stream: *mut c_void);
}

/// `kFSEventStreamEventIdSinceNow`.
const SINCE_NOW: u64 = u64::MAX;
/// `kFSEventStreamCreateFlagWatchRoot`: an event when the watched directory
/// moves or goes away.
const FLAG_WATCH_ROOT: u32 = 0x4;
/// Event flags that ask for a full scan of the tree.
const EVENT_RESCAN: u32 = 0x1 | 0x2 | 0x4 | 0x20;
const LATENCY: f64 = 0.25;

// ---- Debounce ----

/// The duration of the most recent directory read, in milliseconds.
static LAST_READ_MS: AtomicU64 = AtomicU64::new(0);

/// Records the duration of a directory read. A slow read makes the
/// debounce wait longer.
pub fn set_last_read(time: Duration) {
    let ms = u64::try_from(time.as_millis()).unwrap_or(u64::MAX);
    LAST_READ_MS.store(ms, Ordering::Relaxed);
}

/// The quiet time: 300 ms (more than the FSEvents latency, so that the
/// batches of one burst join), or two times the last read, at most 2 s.
fn quiet_time() -> Duration {
    let read = LAST_READ_MS.load(Ordering::Relaxed);
    Duration::from_millis(read.saturating_mul(2).clamp(300, 2000))
}

/// The maximum wait after the first event: four times the quiet time, at
/// least 1 s.
fn max_wait(quiet: Duration) -> Duration {
    (quiet * 4).max(Duration::from_secs(1))
}

struct Debounce {
    /// The first and the last event of the burst that waits.
    first: Option<Instant>,
    last: Option<Instant>,
    /// A check waits on the main queue.
    armed: bool,
    /// The most recent time the action ran.
    fired: Option<Instant>,
}

impl Debounce {
    const fn new() -> Self {
        Self {
            first: None,
            last: None,
            armed: false,
            fired: None,
        }
    }
}

static DIRECTORY_DEBOUNCE: Mutex<Debounce> = Mutex::new(Debounce::new());
static CONFIG_DEBOUNCE: Mutex<Debounce> = Mutex::new(Debounce::new());

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Records one event. After a quiet period, `action` runs at once on the
/// main queue. In a burst, it runs when the events stop (or at the maximum
/// wait).
fn debounce(slot: &'static Mutex<Debounce>, action: fn()) {
    let now = Instant::now();
    let mut d = lock(slot);
    let idle = !d.armed && d.fired.is_none_or(|t| now - t >= quiet_time());
    if idle {
        d.fired = Some(now);
        drop(d);
        app::after_now(action);
        return;
    }
    d.last = Some(now);
    d.first.get_or_insert(now);
    if std::mem::replace(&mut d.armed, true) {
        return;
    }
    drop(d);
    app::after(quiet_time(), move || debounce_check(slot, action));
}

fn debounce_check(slot: &'static Mutex<Debounce>, action: fn()) {
    let mut d = lock(slot);
    let (Some(first), Some(last)) = (d.first, d.last) else {
        d.armed = false;
        return;
    };
    let quiet = quiet_time();
    let deadline = (last + quiet).min(first + max_wait(quiet));
    let now = Instant::now();
    if now < deadline {
        drop(d);
        app::after(deadline - now, move || debounce_check(slot, action));
        return;
    }
    *d = Debounce {
        fired: Some(now),
        ..Debounce::new()
    };
    drop(d);
    action();
}

// ---- Streams ----

enum Role {
    Directory,
    /// The config files and their most recent fingerprints.
    Config(Mutex<Vec<(PathBuf, Fingerprint)>>),
}

/// The data of one stream, for its callback.
struct StreamInfo {
    role: Role,
    /// The watched directories as bytes, without a `/` at the end: the
    /// given form and the real form (FSEvents reports real paths, for
    /// example `/private/var` for `/var`).
    dirs: Vec<Vec<u8>>,
}

/// The size, time, and identity of a config file and of its symlink.
type Fingerprint = [i64; 6];

fn fingerprint(path: &Path) -> Fingerprint {
    let mut f = [0; 6];
    if let Ok(m) = std::fs::metadata(path) {
        f[0] = m.ino() as i64;
        f[1] = m.mtime();
        f[2] = m.mtime_nsec();
        f[3] = m.size() as i64;
    }
    if let Ok(m) = std::fs::symlink_metadata(path) {
        f[4] = m.ino() as i64;
        f[5] = m.mtime_nsec();
    }
    f
}

fn trim_slash(path: &[u8]) -> &[u8] {
    match path {
        [rest @ .., b'/'] if !rest.is_empty() => rest,
        _ => path,
    }
}

impl StreamInfo {
    /// True when the event is for a watched directory, or when FSEvents asks
    /// for a scan of a tree that contains a watched directory.
    fn matches(&self, path: &[u8], flags: u32) -> bool {
        let path = trim_slash(path);
        let rescan = flags & EVENT_RESCAN != 0;
        self.dirs.iter().any(|dir| {
            dir.as_slice() == path
                || (rescan
                    && (path == b"/"
                        || path.is_empty()
                        || (dir.starts_with(path) && dir.get(path.len()) == Some(&b'/'))))
        })
    }

    fn fire(&self) {
        match &self.role {
            Role::Directory => debounce(&DIRECTORY_DEBOUNCE, app::auto_refresh),
            Role::Config(files) => {
                // Directory-level events do not name the file. Reload only
                // when a config file really changed.
                let mut files = lock(files);
                let mut changed = false;
                for (path, old) in files.iter_mut() {
                    let new = fingerprint(path);
                    if new != *old {
                        *old = new;
                        changed = true;
                    }
                }
                if changed {
                    debounce(&CONFIG_DEBOUNCE, app::auto_reload_config);
                }
            }
        }
    }
}

unsafe extern "C" fn stream_callback(
    _stream: *const c_void,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const u32,
    _ids: *const u64,
) {
    if info.is_null() || paths.is_null() || flags.is_null() {
        return;
    }
    // SAFETY: `info` is the `StreamInfo` of this stream. It lives until the
    // stream releases it, and the stream calls back only before that.
    let info = unsafe { &*info.cast::<StreamInfo>() };
    let paths = paths.cast::<*const c_char>();
    for i in 0..count {
        // SAFETY: FSEvents gives `count` C strings and `count` flags.
        let (path, flag) = unsafe { (CStr::from_ptr(*paths.add(i)), *flags.add(i)) };
        if info.matches(path.to_bytes(), flag) {
            info.fire();
            return;
        }
    }
}

unsafe extern "C" fn release_info(info: *const c_void) {
    if !info.is_null() {
        // SAFETY: `info` comes from `Box::into_raw` in `Stream::start`, and
        // FSEvents releases it once.
        drop(unsafe { Box::from_raw(info.cast::<StreamInfo>().cast_mut()) });
    }
}

/// A running FSEvents stream.
struct Stream(*mut c_void);

// SAFETY: Only the serial watcher queue uses the stream pointer.
unsafe impl Send for Stream {}

impl Stream {
    /// Creates and starts a stream for `watch` on `queue`.
    fn start(queue: &DispatchQueue, watch: &[PathBuf], info: StreamInfo) -> Option<Stream> {
        let strings: Vec<Retained<NSString>> = watch
            .iter()
            .filter_map(|p| p.to_str())
            .map(NSString::from_str)
            .collect();
        if strings.is_empty() {
            return None;
        }
        // `NSArray` of `NSString` is toll-free bridged to `CFArray` of `CFString`.
        let array = NSArray::from_retained_slice(&strings);
        let info = Box::into_raw(Box::new(info));
        let context = StreamContext {
            version: 0,
            info: info.cast(),
            retain: None,
            release: Some(release_info),
            copy_description: None,
        };
        // SAFETY: The arguments are valid: a CFArray of paths, a context
        // that owns `info`, and a callback with the correct signature.
        let stream = unsafe {
            FSEventStreamCreate(
                std::ptr::null(),
                stream_callback,
                &context,
                Retained::as_ptr(&array).cast(),
                SINCE_NOW,
                LATENCY,
                FLAG_WATCH_ROOT,
            )
        };
        if stream.is_null() {
            // SAFETY: The stream does not exist, so nothing else owns `info`.
            drop(unsafe { Box::from_raw(info) });
            return None;
        }
        let stream = Stream(stream);
        let queue_ptr: *const DispatchQueue = queue;
        // SAFETY: `stream` is valid. The queue lives for the whole app.
        let started = unsafe {
            FSEventStreamSetDispatchQueue(stream.0, queue_ptr.cast());
            FSEventStreamStart(stream.0) != 0
        };
        started.then_some(stream)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: The stream is valid. This runs on the serial queue of the
        // callbacks, so no callback runs at the same time. The release
        // frees the `StreamInfo`.
        unsafe {
            FSEventStreamStop(self.0);
            FSEventStreamInvalidate(self.0);
            FSEventStreamRelease(self.0);
        }
    }
}

#[derive(Default)]
struct Streams {
    directory: Option<(PathBuf, Stream)>,
    config: Option<Stream>,
}

static QUEUE: OnceLock<DispatchRetained<DispatchQueue>> = OnceLock::new();
static STREAMS: Mutex<Option<Streams>> = Mutex::new(None);

fn queue() -> &'static DispatchQueue {
    QUEUE.get_or_init(|| DispatchQueue::new("better-finder.watcher", None))
}

/// Runs `work` on the watcher queue with the streams.
fn on_queue(work: impl FnOnce(&DispatchQueue, &mut Streams) + Send + 'static) {
    let q = queue();
    q.exec_async(move || {
        let mut streams = lock(&STREAMS);
        work(q, streams.get_or_insert_with(Streams::default));
    });
}

/// Creates the watcher queue. Call it once at the start.
pub fn start() {
    let _ = queue();
}

/// The bytes of `path` and of its real path, without a `/` at the end.
fn forms(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = vec![path.to_path_buf()];
    if let Ok(real) = path.canonicalize() {
        if real != path {
            out.push(real);
        }
    }
    out.into_iter()
        .map(|p| {
            let bytes = trim_slash(p.as_os_str().as_bytes()).to_vec();
            (p, bytes)
        })
        .collect()
}

/// Watches `path` instead of the previous directory.
pub fn watch_directory(path: &Path) {
    let path = path.to_path_buf();
    on_queue(move |queue, streams| {
        if streams.directory.as_ref().is_some_and(|(p, _)| *p == path) {
            return;
        }
        // Stop the old stream first.
        streams.directory = None;
        let forms = forms(&path);
        let info = StreamInfo {
            role: Role::Directory,
            dirs: forms.iter().map(|(_, b)| b.clone()).collect(),
        };
        let watch: Vec<PathBuf> = forms.into_iter().map(|(p, _)| p).collect();
        streams.directory = Stream::start(queue, &watch, info).map(|s| (path, s));
    });
}

/// Watches the config files `files`. A missing file is fine: the watch is
/// on its directory. For a symlink, the directory of the real file is
/// watched too.
pub fn watch_config(files: Vec<PathBuf>) {
    on_queue(move |queue, streams| {
        streams.config = None;
        let mut dirs: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        let mut add_dir = |dir: &Path| {
            if dir.is_dir() {
                for form in forms(dir) {
                    if !dirs.iter().any(|(_, b)| *b == form.1) {
                        dirs.push(form);
                    }
                }
            }
        };
        for file in &files {
            if let Some(dir) = file.parent() {
                add_dir(dir);
            }
            if let Ok(real) = file.canonicalize() {
                if let Some(dir) = real.parent() {
                    add_dir(dir);
                }
            }
        }
        let fingerprints = files.iter().map(|f| (f.clone(), fingerprint(f))).collect();
        let info = StreamInfo {
            role: Role::Config(Mutex::new(fingerprints)),
            dirs: dirs.iter().map(|(_, b)| b.clone()).collect(),
        };
        let watch: Vec<PathBuf> = dirs.into_iter().map(|(p, _)| p).collect();
        streams.config = Stream::start(queue, &watch, info);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(dirs: &[&str]) -> StreamInfo {
        StreamInfo {
            role: Role::Directory,
            dirs: dirs.iter().map(|d| d.as_bytes().to_vec()).collect(),
        }
    }

    #[test]
    fn matches_only_the_watched_directory() {
        let i = info(&["/Users/a/Downloads"]);
        assert!(i.matches(b"/Users/a/Downloads/", 0));
        assert!(i.matches(b"/Users/a/Downloads", 0));
        assert!(!i.matches(b"/Users/a/Downloads/sub/", 0));
        assert!(!i.matches(b"/Users/a/", 0));
        assert!(!i.matches(b"/Users/a/Downloads2/", 0));
    }

    #[test]
    fn rescan_of_a_parent_matches() {
        let i = info(&["/Users/a/Downloads"]);
        assert!(i.matches(b"/Users/a/", 0x1));
        assert!(i.matches(b"/", 0x4));
        assert!(!i.matches(b"/Users/a/Down/", 0x1));
        assert!(!i.matches(b"/Users/a/Downloads/sub/", 0x1));
    }

    #[test]
    fn quiet_time_follows_the_read_time() {
        set_last_read(Duration::from_millis(10));
        assert_eq!(quiet_time(), Duration::from_millis(300));
        set_last_read(Duration::from_millis(400));
        assert_eq!(quiet_time(), Duration::from_millis(800));
        assert_eq!(max_wait(quiet_time()), Duration::from_millis(3200));
        set_last_read(Duration::from_secs(10));
        assert_eq!(quiet_time(), Duration::from_secs(2));
        set_last_read(Duration::ZERO);
    }
}
