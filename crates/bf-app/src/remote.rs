//! The SSH transport of remote folders (docs/UI.md, "Remote folders").
//!
//! - The app runs the system `ssh` (`/usr/bin/ssh`) with the arguments of
//!   `bf_core::remote::ssh_args`. The user config (`~/.ssh/config`), the
//!   keys, the agent, and `known_hosts` apply. `BatchMode` makes a login
//!   that needs a prompt fail at once, with a message.
//! - A script of `bf_core::remote` goes to the standard input of `ssh`.
//! - The error output of `ssh` goes to a file without a name, not to a
//!   pipe. A `ControlPersist` master continues in the background after the
//!   first command, and some versions of `ssh` keep their error output
//!   open there. A pipe then does not end for 10 minutes.
//! - All functions here block. Call them on a background thread.
//!
//! The environment variable `BETTER_FINDER_SSH` replaces `/usr/bin/ssh`,
//! for UI tests without a network (CONTRIBUTING.md).

use std::borrow::Cow;
use std::fs::File;
use std::io::{self, Read as _, Seek as _, SeekFrom, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use bf_core::remote::{self, Job, Listing};

use crate::app;
use crate::debug::trace;

/// The system OpenSSH client.
const SSH: &str = "/usr/bin/ssh";

/// The most bytes before the marker of a copy (text of the login shell).
const MAX_PREAMBLE: usize = 1 << 20;

fn ssh_program() -> PathBuf {
    std::env::var_os("BETTER_FINDER_SSH")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(SSH))
}

/// The private folder of the control sockets: `bf-ssh-<uid>` in the temp
/// folder of the user, with mode 0700. `None` when the folder is not safe
/// (a symlink, another owner, or open to other users): then each command
/// opens its own connection.
fn control_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
        // SAFETY: `getuid` has no preconditions and cannot fail.
        let uid = unsafe { libc::getuid() };
        let dir = std::env::temp_dir().join(format!("bf-ssh-{uid}"));
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
        let md = std::fs::symlink_metadata(&dir).ok()?;
        let safe = md.is_dir() && md.uid() == uid && md.mode() & 0o077 == 0;
        if !safe {
            trace!(
                "remote: {} is not private: no shared connection",
                dir.display()
            );
        }
        safe.then_some(dir)
    })
    .as_deref()
}

/// The folder of the local copies of remote files:
/// `~/Library/Caches/com.p1ratedev.better-finder/remote`.
pub fn cache_root() -> PathBuf {
    bf_core::fs::home_dir().join("Library/Caches/com.p1ratedev.better-finder/remote")
}

/// A number for the names of temporary files.
fn unique() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A file without a name for the error output of `ssh`.
fn error_file() -> io::Result<File> {
    let dir = std::env::temp_dir();
    loop {
        let path = dir.join(format!("bf-ssh-{}-{}.err", std::process::id(), unique()));
        match File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                let _ = std::fs::remove_file(&path);
                return Ok(file);
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
}

/// One `ssh` process that runs one script.
struct Session {
    child: Child,
    errors: File,
    destination: String,
}

impl Session {
    /// Starts `ssh` and writes `script` to its standard input.
    fn start(destination: &str, script: &[u8]) -> io::Result<Session> {
        let errors = error_file()?;
        let control = control_dir().and_then(|dir| remote::control_path(dir, destination));
        let mut child = Command::new(ssh_program())
            .args(remote::ssh_args(destination, control.as_deref()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(errors.try_clone()?))
            .spawn()
            .map_err(|err| io::Error::other(format!("cannot run ssh: {err}")))?;
        // The script is about 1 KB: the pipe takes all of it at once. When
        // `ssh` fails before it reads the script, the write fails, and the
        // exit code tells why.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(script);
        }
        Ok(Session {
            child,
            errors,
            destination: destination.to_string(),
        })
    }

    /// The message for a failed script, from the exit code and the error output.
    fn error(&mut self, job: Job, code: Option<i32>) -> io::Error {
        let mut text = Vec::new();
        if self.errors.seek(SeekFrom::Start(0)).is_ok() {
            let _ = (&self.errors).take(64 * 1024).read_to_end(&mut text);
        }
        let text = String::from_utf8_lossy(&text);
        trace!("remote: ssh exit {code:?}: {}", text.trim());
        io::Error::other(remote::error_text(job, code, &text, &self.destination))
    }
}

/// Lists the folder `path` on `destination`. `path` is absolute, `~`, or
/// `~/x`. The listing has the real path of the folder.
pub fn list(destination: &str, path: &Path) -> io::Result<Listing> {
    let started = Instant::now();
    let mut session = Session::start(destination, &remote::list_script(path))?;
    let mut out = Vec::new();
    let read = session
        .child
        .stdout
        .take()
        .map(|mut stdout| stdout.read_to_end(&mut out));
    let status = session.child.wait()?;
    if !status.success() {
        return Err(session.error(Job::List, status.code()));
    }
    if let Some(Err(err)) = read {
        return Err(err);
    }
    let listing = remote::parse_listing(&out).map_err(io::Error::other)?;
    trace!(
        "remote: {} entries of {} in {} ms ({} bytes)",
        listing.entries.len(),
        remote::display(destination, &listing.dir),
        started.elapsed().as_millis(),
        out.len()
    );
    Ok(listing)
}

/// A remote file to copy.
#[derive(Clone, Debug)]
pub struct FetchRequest {
    pub destination: String,
    /// The absolute path on the remote host.
    pub path: PathBuf,
    /// The size and the modification time from the listing.
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// A local copy with the same size and time can serve again (a regular
    /// file; not a symlink, because its listing has the data of the link).
    pub reuse: bool,
}

/// Stops a copy from another thread.
#[derive(Default)]
pub struct FetchControl {
    canceled: AtomicBool,
    /// The process of `ssh`, while the copy reads its output. 0 before and after.
    pid: AtomicI32,
}

impl FetchControl {
    /// Stops the copy: the thread stops at the next read, and `ssh` gets
    /// `SIGTERM`, so a slow connection does not keep the thread.
    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::SeqCst);
        let pid = self.pid.load(Ordering::SeqCst);
        if pid > 0 {
            // SAFETY: `kill` has no memory preconditions. The pid is a child
            // that the copy thread did not reap yet (it sets 0 first).
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }

    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }
}

fn canceled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "canceled")
}

/// Copies a remote file into the cache (`cache_root`) and gives the local
/// path. The copy is read-only (mode 0444) and has the modification time
/// of the remote file. A local copy with the same size and time serves
/// again without a download (`FetchRequest::reuse`). `progress` gets the
/// number of bytes after each read. The data goes to a hidden temporary
/// file first, and a rename puts it in place: a cut connection never
/// leaves a partial copy.
pub fn fetch(
    request: &FetchRequest,
    control: &FetchControl,
    mut progress: impl FnMut(u64),
) -> io::Result<PathBuf> {
    let target = remote::cache_path(&cache_root(), &request.destination, &request.path)
        .ok_or_else(|| io::Error::other("the remote path is not valid"))?;
    if request.reuse && is_fresh(&target, request) {
        trace!("remote: {} is in the cache", target.display());
        return Ok(target);
    }
    let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else {
        return Err(io::Error::other("the remote path is not valid"));
    };
    std::fs::create_dir_all(dir)?;
    remove_stale_parts(dir, &name.to_string_lossy());
    let part = dir.join(format!(
        ".{}.bf-part-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        unique()
    ));
    let mut file = File::options().write(true).create_new(true).open(&part)?;
    let copied = download(request, control, &mut file, &mut progress).and_then(|()| {
        if let Some(time) = request.modified {
            file.set_modified(time)?;
        }
        drop(file);
        std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o444))?;
        std::fs::rename(&part, &target)
    });
    if let Err(err) = copied {
        let _ = std::fs::remove_file(&part);
        return Err(err);
    }
    Ok(target)
}

/// Removes the temporary files of old copies of `name` in `dir`: an app
/// that quits during a copy leaves one. The temporary files of a process
/// that runs (this app, or a second instance) stay.
fn remove_stale_parts(dir: &Path, name: &str) {
    let prefix = format!(".{name}.bf-part-");
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for item in read.flatten() {
        let file = item.file_name();
        let Some(rest) = file.to_str().and_then(|f| f.strip_prefix(&prefix)) else {
            continue;
        };
        let pid: libc::pid_t = rest
            .split('-')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        // SAFETY: `kill` with signal 0 only checks that the process exists.
        let runs = pid > 0 && unsafe { libc::kill(pid, 0) } == 0;
        if !runs {
            trace!("remote: remove {}", item.path().display());
            let _ = std::fs::remove_file(item.path());
        }
    }
}

/// The local copy has the size and the time of the remote file.
fn is_fresh(target: &Path, request: &FetchRequest) -> bool {
    std::fs::metadata(target).is_ok_and(|md| {
        md.is_file()
            && md.len() == request.size
            && request.modified.is_some()
            && md.modified().ok() == request.modified
    })
}

/// Runs the copy script and writes the bytes after the marker to `file`.
fn download(
    request: &FetchRequest,
    control: &FetchControl,
    file: &mut File,
    progress: &mut impl FnMut(u64),
) -> io::Result<()> {
    if control.is_canceled() {
        return Err(canceled());
    }
    let mut session = Session::start(&request.destination, &remote::fetch_script(&request.path))?;
    control
        .pid
        .store(session.child.id().try_into().unwrap_or(0), Ordering::SeqCst);
    let mut stdout = session.child.stdout.take();
    let mut buf = vec![0u8; 256 * 1024];
    // The bytes before the marker, until the marker comes.
    let mut head: Option<Vec<u8>> = Some(Vec::new());
    let mut written = 0u64;
    let read: io::Result<()> = loop {
        if control.is_canceled() {
            break Err(canceled());
        }
        let Some(out) = stdout.as_mut() else {
            break Ok(());
        };
        let n = match out.read(&mut buf) {
            Ok(0) => break Ok(()),
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => break Err(err),
        };
        let data = match head.as_mut() {
            Some(bytes) => {
                bytes.extend_from_slice(&buf[..n]);
                let Some(at) = remote::find_marker(bytes) else {
                    if bytes.len() > MAX_PREAMBLE {
                        break Err(io::Error::other("the remote host did not send the file"));
                    }
                    continue;
                };
                let rest = bytes.split_off(at);
                head = None;
                Cow::Owned(rest)
            }
            None => Cow::Borrowed(&buf[..n]),
        };
        if let Err(err) = file.write_all(&data) {
            break Err(err);
        }
        written += data.len() as u64;
        progress(written);
    };
    drop(stdout);
    // Before the wait: after the wait, the pid can belong to a new process.
    control.pid.store(0, Ordering::SeqCst);
    if read.is_err() {
        let _ = session.child.kill();
    }
    let status = session.child.wait()?;
    if control.is_canceled() {
        return Err(canceled());
    }
    read?;
    if !status.success() {
        return Err(session.error(Job::Fetch, status.code()));
    }
    if head.is_some() {
        return Err(io::Error::other("the remote host did not send the file"));
    }
    Ok(())
}

/// Copies a remote file on a new thread. `app::fetch_progress` gets the
/// progress (at most 4 times each second, and only after 250 ms, so a small
/// file shows no progress), and `app::fetch_finished` gets the result. Both
/// run on the main queue.
pub fn spawn_fetch(id: u64, request: FetchRequest, control: Arc<FetchControl>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("bf-fetch".into())
        .spawn(move || {
            let started = Instant::now();
            let mut last = started;
            let total = request.size;
            let result = fetch(&request, &control, |done| {
                if last.elapsed() >= Duration::from_millis(250) {
                    last = Instant::now();
                    app::after_now(move || app::fetch_progress(id, done, total));
                }
            });
            trace!(
                "remote: copy of {} in {} ms: {:?}",
                request.path.display(),
                started.elapsed().as_millis(),
                result.as_ref().map(|p| p.display().to_string())
            );
            app::after_now(move || app::fetch_finished(id, result));
        })?;
    Ok(())
}
