//! The mounted volumes and the eject flow.
//!
//! - `list` reads the mounted volumes that are not hidden, the boot volume
//!   first. It accesses the disk: `refresh` runs it on a background thread,
//!   and `app::set_volumes` gets the result on the main queue.
//! - One observer gets the `NSWorkspace` mount, unmount, and rename
//!   notifications. The app then reads the list again and builds the
//!   sidebar again. An unmount also goes to `app::volume_unmounted`.
//! - `spawn_eject` ejects a volume on a background thread with
//!   `NSWorkspace unmountAndEjectDeviceAtURL:error:`. When the volume is
//!   busy, `lsof` finds the apps that use it (3 s timeout). A forced eject
//!   runs `diskutil unmount force`. After the unmount, `diskutil eject`
//!   ejects the disk when it is not internal (a USB disk or a disk image).
//!   The main thread never waits for these steps.

use std::cell::OnceCell;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::{MainThreadOnly, Message as _, define_class, msg_send, sel};
use objc2_app_kit::{
    NSRunningApplication, NSWorkspace, NSWorkspaceDidMountNotification,
    NSWorkspaceDidRenameVolumeNotification, NSWorkspaceDidUnmountNotification,
    NSWorkspaceVolumeLocalizedNameKey, NSWorkspaceVolumeURLKey,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSError, NSFileManager, NSNotification, NSNumber, NSObject,
    NSObjectProtocol, NSString, NSURL, NSURLResourceKey, NSURLVolumeIsEjectableKey,
    NSURLVolumeIsInternalKey, NSURLVolumeIsLocalKey, NSURLVolumeIsRemovableKey,
    NSURLVolumeIsRootFileSystemKey, NSURLVolumeLocalizedNameKey, NSUnderlyingErrorKey,
    NSVolumeEnumerationOptions,
};

use crate::app;
use crate::debug::trace;
use crate::file_ops;

/// The time limit of `lsof`.
const LSOF_TIMEOUT: Duration = Duration::from_secs(3);
/// The time limit of one `diskutil` command.
const DISKUTIL_TIMEOUT: Duration = Duration::from_secs(30);
const LSOF: &str = "/usr/sbin/lsof";
const DISKUTIL: &str = "/usr/sbin/diskutil";

/// One mounted volume.
#[derive(Clone, Debug, PartialEq)]
pub struct Volume {
    /// The mount point, for example `/Volumes/USB`.
    pub path: PathBuf,
    /// The localized name, for example `Macintosh HD`.
    pub name: String,
    /// The SF Symbol: `internaldrive`, `externaldrive`, or `network`.
    pub symbol: &'static str,
    /// The boot volume (`NSURLVolumeIsRootFileSystemKey`). It never ejects.
    pub root: bool,
}

impl Volume {
    /// True for all volumes except the boot volume.
    pub fn ejectable(&self) -> bool {
        !self.root
    }
}

/// The mounted volumes that are not hidden, the boot volume first. It
/// accesses the disk: call it on a background thread.
pub fn list() -> Vec<Volume> {
    let manager = NSFileManager::defaultManager();
    // SAFETY: The resource keys are immutable statics from Foundation.
    let (name_key, internal_key, local_key, root_key) = unsafe {
        (
            NSURLVolumeLocalizedNameKey,
            NSURLVolumeIsInternalKey,
            NSURLVolumeIsLocalKey,
            NSURLVolumeIsRootFileSystemKey,
        )
    };
    let keys = NSArray::from_slice(&[name_key, internal_key, local_key, root_key]);
    let Some(urls) = manager.mountedVolumeURLsIncludingResourceValuesForKeys_options(
        Some(&keys),
        NSVolumeEnumerationOptions::SkipHiddenVolumes,
    ) else {
        return Vec::new();
    };
    let mut volumes: Vec<Volume> = Vec::new();
    for url in urls.iter() {
        let Some(path) = url.to_file_path() else {
            continue;
        };
        let Ok(values) = url.resourceValuesForKeys_error(&keys) else {
            continue;
        };
        let flag = |key: &NSURLResourceKey| {
            values
                .objectForKey(key)
                .and_then(|v| v.downcast::<NSNumber>().ok())
                .map(|n| n.boolValue())
        };
        let name = values
            .objectForKey(name_key)
            .and_then(|v| v.downcast::<NSString>().ok())
            .map(|s| s.to_string())
            .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let symbol = if flag(local_key) == Some(false) {
            "network"
        } else if flag(internal_key) == Some(true) {
            "internaldrive"
        } else {
            "externaldrive"
        };
        let root = flag(root_key) == Some(true) || path == Path::new("/");
        volumes.push(Volume {
            path,
            name,
            symbol,
            root,
        });
    }
    // The boot volume first.
    volumes.sort_by_key(|v| !v.root);
    volumes
}

/// Reads the volume list on a background thread. `app::set_volumes` gets
/// the result on the main queue.
pub fn refresh() {
    let spawned = std::thread::Builder::new()
        .name("bf-volumes".into())
        .spawn(|| {
            let volumes = list();
            app::after_now(move || app::set_volumes(volumes));
        });
    if spawned.is_err() {
        trace!("volumes: cannot start a thread");
    }
}

// ---- Mount notifications ----

define_class!(
    // SAFETY:
    // - NSObject has no subclass requirements.
    // - VolumeObserver does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BFVolumeObserver"]
    struct VolumeObserver;

    // SAFETY: `NSObjectProtocol` has no safety requirements. The signatures match.
    unsafe impl NSObjectProtocol for VolumeObserver {}

    impl VolumeObserver {
        #[unsafe(method(volumesChanged:))]
        fn volumes_changed(&self, _notification: &NSNotification) {
            trace!(
                "volumes: mount or rename after {} ms",
                crate::debug::since_start().as_millis()
            );
            app::after_now(app::volumes_changed);
        }

        #[unsafe(method(volumeUnmounted:))]
        fn volume_unmounted(&self, notification: &NSNotification) {
            unmounted(notification);
        }
    }
);

/// An unmount notification: the app leaves the volume when it shows it.
fn unmounted(notification: &NSNotification) {
    let Some((path, name)) = unmounted_volume(notification) else {
        trace!("volumes: unmount without a volume URL");
        app::after_now(app::volumes_changed);
        return;
    };
    trace!(
        "volumes: unmount of {} after {} ms",
        path.display(),
        crate::debug::since_start().as_millis()
    );
    app::after_now(move || app::volume_unmounted(path, name));
}

/// The mount point and the name of the volume in an unmount notification.
fn unmounted_volume(notification: &NSNotification) -> Option<(PathBuf, Option<String>)> {
    let info = notification.userInfo()?;
    // SAFETY: The keys are immutable statics from AppKit.
    let (url_key, name_key) =
        unsafe { (NSWorkspaceVolumeURLKey, NSWorkspaceVolumeLocalizedNameKey) };
    let url = info.objectForKey(url_key)?.downcast::<NSURL>().ok()?;
    let path = url.to_file_path()?;
    let name = info
        .objectForKey(name_key)
        .and_then(|v| v.downcast::<NSString>().ok())
        .map(|s| s.to_string());
    Some((path, name))
}

thread_local! {
    static OBSERVER: OnceCell<Retained<VolumeObserver>> = const { OnceCell::new() };
}

/// Starts to observe the mount, unmount, and rename notifications. Call it
/// once at the start.
pub fn observe(mtm: MainThreadMarker) {
    let observer: Retained<VolumeObserver> = {
        let this = VolumeObserver::alloc(mtm).set_ivars(());
        // SAFETY: `init` is the designated initializer of NSObject.
        unsafe { msg_send![super(this), init] }
    };
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: The notification names are immutable statics. The observer
    // lives in a thread-local for the whole life of the app.
    unsafe {
        for name in [
            NSWorkspaceDidMountNotification,
            NSWorkspaceDidRenameVolumeNotification,
        ] {
            center.addObserver_selector_name_object(
                &observer,
                sel!(volumesChanged:),
                Some(name),
                None,
            );
        }
        center.addObserver_selector_name_object(
            &observer,
            sel!(volumeUnmounted:),
            Some(NSWorkspaceDidUnmountNotification),
            None,
        );
    }
    OBSERVER.with(|o| {
        let _ = o.set(observer);
    });
}

// ---- Eject ----

/// The result of an eject.
#[derive(Debug, Clone, PartialEq)]
pub enum EjectOutcome {
    Ejected,
    /// The volume is busy. The names of the apps that use it (the list can
    /// be empty).
    Busy(Vec<String>),
    /// Another failure, with the reason.
    Failed(String),
}

/// Ejects the volume at `mount` on a new thread. Then `done` gets the
/// result on that thread. `force` runs `diskutil unmount force`, then
/// `diskutil eject` for a disk.
pub fn spawn_eject(mount: PathBuf, force: bool, done: impl FnOnce(EjectOutcome) + Send + 'static) {
    let done = Arc::new(Mutex::new(Some(done)));
    let thread_done = done.clone();
    let spawned = std::thread::Builder::new()
        .name("bf-eject".into())
        .spawn(move || {
            let started = Instant::now();
            let outcome = if force {
                force_eject(&mount)
            } else {
                eject(&mount)
            };
            trace!(
                "eject {} (force {force}): {outcome:?} after {} ms",
                mount.display(),
                started.elapsed().as_millis()
            );
            let done = thread_done
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(done) = done {
                done(outcome);
            }
        });
    if let Err(err) = spawned {
        let done = done.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(done) = done {
            done(EjectOutcome::Failed(format!(
                "cannot start a thread: {err}"
            )));
        }
    }
}

/// The normal eject, like Finder: it unmounts all volumes of the device,
/// then ejects the device. When it fails, `lsof` looks for the apps that
/// use the volume.
fn eject(mount: &Path) -> EjectOutcome {
    let Some(url) = NSURL::from_directory_path(mount) else {
        return EjectOutcome::Failed("the path is not valid".to_string());
    };
    // Get the device before the unmount: after it, the path is gone.
    let device = external_device(mount);
    let Err(err) = NSWorkspace::sharedWorkspace().unmountAndEjectDeviceAtURL_error(&url) else {
        eject_device(device);
        return EjectOutcome::Ejected;
    };
    let busy = is_busy(&err);
    let reason = err.localizedDescription().to_string();
    trace!(
        "eject: error domain {} code {} (busy {busy}): {reason}",
        err.domain(),
        err.code()
    );
    let apps = users_of(mount);
    if busy || !apps.is_empty() {
        EjectOutcome::Busy(apps)
    } else {
        EjectOutcome::Failed(reason)
    }
}

/// `kDAReturnBusy` of DiskArbitration.
const DA_RETURN_BUSY: u32 = 0xF8DA_0002;
/// `fBsyErr` of the Carbon file manager.
const FILE_BUSY_ERROR: isize = -47;

/// True when the error, or an underlying error, says that the volume is busy.
fn is_busy(err: &NSError) -> bool {
    let mut current = Some(err.retain());
    while let Some(e) = current {
        let domain = e.domain().to_string();
        let code = e.code();
        let busy = (domain == "NSPOSIXErrorDomain" && code == libc::EBUSY as isize)
            || code == FILE_BUSY_ERROR
            || code == DA_RETURN_BUSY as isize
            || code == DA_RETURN_BUSY as i32 as isize;
        if busy {
            return true;
        }
        // SAFETY: The key is an immutable static from Foundation.
        let key = unsafe { NSUnderlyingErrorKey };
        current = e
            .userInfo()
            .objectForKey(key)
            .and_then(|v| v.downcast::<NSError>().ok());
    }
    false
}

/// The forced eject: `diskutil unmount force`, then `diskutil eject` for
/// a disk that is not internal. A network volume and an internal disk only
/// unmount.
fn force_eject(mount: &Path) -> EjectOutcome {
    // Get the device before the unmount: after it, the path is gone.
    let device = external_device(mount);
    let unmount = [
        OsStr::new("unmount"),
        OsStr::new("force"),
        mount.as_os_str(),
    ];
    if let Err(reason) = run_checked(DISKUTIL, &unmount) {
        return EjectOutcome::Failed(reason);
    }
    eject_device(device);
    EjectOutcome::Ejected
}

/// The device of the volume at `mount` (for example `/dev/disk5s1`), when
/// the disk is not internal: a USB disk or a disk image. A disk image has
/// no internal flag, but it is ejectable. `None` for an internal disk, a
/// network volume, and when the check fails.
fn external_device(mount: &Path) -> Option<String> {
    let device = file_ops::mounted_from(mount).filter(|d| d.starts_with("/dev/disk"))?;
    let url = NSURL::from_directory_path(mount)?;
    // SAFETY: The resource keys are immutable statics from Foundation.
    let (internal_key, ejectable_key, removable_key) = unsafe {
        (
            NSURLVolumeIsInternalKey,
            NSURLVolumeIsEjectableKey,
            NSURLVolumeIsRemovableKey,
        )
    };
    let keys = NSArray::from_slice(&[internal_key, ejectable_key, removable_key]);
    let values = url.resourceValuesForKeys_error(&keys).ok()?;
    let flag = |key: &NSURLResourceKey| {
        values
            .objectForKey(key)
            .and_then(|v| v.downcast::<NSNumber>().ok())
            .map(|n| n.boolValue())
    };
    let internal = flag(internal_key);
    let external = internal != Some(true)
        && (internal == Some(false)
            || flag(ejectable_key) == Some(true)
            || flag(removable_key) == Some(true));
    trace!(
        "eject: {} is on {device}, external {external}",
        mount.display()
    );
    external.then_some(device)
}

/// Ejects the disk of an unmounted volume, like Finder. `NSWorkspace`
/// unmounts an APFS volume but keeps its disk: the APFS container stays,
/// and a disk image stays attached. `diskutil eject` ejects the disk and
/// its container. The volume is unmounted already, so a failure here only
/// goes to the trace.
fn eject_device(device: Option<String>) {
    let Some(device) = device.filter(|d| Path::new(d).exists()) else {
        return;
    };
    if let Err(reason) = run_checked(DISKUTIL, &[OsStr::new("eject"), OsStr::new(&device)]) {
        trace!("eject: the disk {device} stays: {reason}");
    }
}

/// Runs `program`. A failure gives the first line of its output.
fn run_checked(program: &str, args: &[&OsStr]) -> Result<(), String> {
    let run = run(program, args, DISKUTIL_TIMEOUT)?;
    trace!(
        "eject: {program} {args:?}: success {} {:?} {:?}",
        run.success,
        run.stdout.trim(),
        run.stderr.trim()
    );
    if run.success {
        return Ok(());
    }
    if run.timed_out {
        return Err(format!("{program} did not finish"));
    }
    let line = [&run.stderr, &run.stdout]
        .into_iter()
        .flat_map(|text| text.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("unknown error");
    Err(line.to_string())
}

/// The names of the apps with open files on the volume at `mount`:
/// `lsof -F pc +f -- <mount>`, with a 3 s timeout. An app has its
/// localized name, another process has its command name.
fn users_of(mount: &Path) -> Vec<String> {
    let args = [
        OsStr::new("-F"),
        OsStr::new("pc"),
        OsStr::new("+f"),
        OsStr::new("--"),
        mount.as_os_str(),
    ];
    let run = match run(LSOF, &args, LSOF_TIMEOUT) {
        Ok(run) => run,
        Err(err) => {
            trace!("eject: {err}");
            return Vec::new();
        }
    };
    if run.timed_out {
        trace!("eject: lsof timed out");
    }
    let mut names: Vec<String> = parse_lsof(&run.stdout)
        .into_iter()
        .map(|(pid, command)| app_name(pid).unwrap_or(command))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    trace!("eject: lsof found {names:?}");
    names
}

/// The localized name of the app with the process `pid`.
fn app_name(pid: i32) -> Option<String> {
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
    app.localizedName().map(|n| n.to_string())
}

/// The (process ID, command) pairs in `lsof -F pc` output. A `p` line
/// starts a process; its `c` line gives the command. Other lines are files.
fn parse_lsof(text: &str) -> Vec<(i32, String)> {
    let mut out = Vec::new();
    let mut pid = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.trim().parse().ok();
        } else if let Some(command) = line.strip_prefix('c') {
            if let Some(pid) = pid.take() {
                out.push((pid, command.to_string()));
            }
        }
    }
    out
}

/// The result of a process run.
struct Run {
    success: bool,
    timed_out: bool,
    stdout: String,
    stderr: String,
}

/// Runs `program` and waits at most `timeout`. Then it stops the process.
/// Call it only on a background thread.
fn run(program: &str, args: &[&OsStr], timeout: Duration) -> Result<Run, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    let stdout = read_all(child.stdout.take());
    let stderr = read_all(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                timed_out = true;
                let _ = child.kill();
                break child.wait().ok();
            }
            Err(_) => break None,
        }
    };
    let text =
        |handle: Option<JoinHandle<String>>| handle.and_then(|h| h.join().ok()).unwrap_or_default();
    Ok(Run {
        success: !timed_out && status.is_some_and(|s| s.success()),
        timed_out,
        stdout: text(stdout),
        stderr: text(stderr),
    })
}

/// Reads a pipe to its end on a thread, so that a full pipe never blocks
/// the process.
fn read_all(pipe: Option<impl Read + Send + 'static>) -> Option<JoinHandle<String>> {
    let mut pipe = pipe?;
    std::thread::Builder::new()
        .name("bf-pipe".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            // A read error keeps the bytes that came before it.
            let _ = pipe.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
        .ok()
}

/// The text of the busy message. `key` is the eject key of the keymap.
pub fn busy_text(name: &str, apps: &[String], key: Option<&str>) -> String {
    let users = if apps.is_empty() {
        String::new()
    } else {
        format!(" by {}", apps.join(", "))
    };
    let again = match key {
        Some(key) => format!("Press {key} again"),
        None => "Eject it again".to_string(),
    };
    let loss = if apps.is_empty() {
        "Unsaved data can be lost."
    } else {
        "Unsaved data in those apps can be lost."
    };
    format!("\"{name}\" is in use{users}. {again} within 5 s to force the eject. {loss}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsof_output() {
        let text = "p123\ncQuick Look\nf5\nf6\np456\nctail\nf3\n";
        assert_eq!(
            parse_lsof(text),
            [(123, "Quick Look".to_string()), (456, "tail".to_string())]
        );
        assert!(parse_lsof("").is_empty());
        // A `c` line without a process does not count.
        assert!(parse_lsof("cnothing\n").is_empty());
    }

    #[test]
    fn busy_message() {
        let apps = ["Preview".to_string(), "tail".to_string()];
        assert_eq!(
            busy_text("USB", &apps, Some("cmd+e")),
            "\"USB\" is in use by Preview, tail. Press cmd+e again within 5 s to force \
             the eject. Unsaved data in those apps can be lost."
        );
        assert_eq!(
            busy_text("USB", &[], None),
            "\"USB\" is in use. Eject it again within 5 s to force the eject. \
             Unsaved data can be lost."
        );
    }

    #[test]
    fn the_boot_volume_is_first_and_not_ejectable() {
        let volumes = list();
        let first = volumes.first().expect("the boot volume");
        assert_eq!(first.path, PathBuf::from("/"));
        assert!(first.root && !first.ejectable());
    }

    #[test]
    fn run_stops_at_the_timeout() {
        let started = Instant::now();
        let slow =
            run("/bin/sleep", &[OsStr::new("5")], Duration::from_millis(200)).expect("sleep runs");
        assert!(slow.timed_out && !slow.success);
        assert!(started.elapsed() < Duration::from_secs(2));
        let fast =
            run("/bin/echo", &[OsStr::new("hi")], Duration::from_secs(5)).expect("echo runs");
        assert!(fast.success && !fast.timed_out);
        assert_eq!(fast.stdout, "hi\n");
    }
}
