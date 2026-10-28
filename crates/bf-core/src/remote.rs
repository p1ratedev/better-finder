//! Remote folders over SSH: the pure parts, with unit tests.
//!
//! The app (`bf-app/src/remote.rs`) runs the system `ssh` binary on a
//! background thread. This module never starts a process. It has:
//!
//! - `Remote`: the value of the `remote` config key and of the `remote:`
//!   action, `[user@]host[:path]`.
//! - The arguments of `ssh`, and the local paths: the control socket and
//!   the cache copy of a remote file.
//! - The shell scripts that run on the remote host, with safe quotes for
//!   the remote path.
//! - The parser of the listing, and the messages for the errors.
//!
//! # Transport
//!
//! The app runs `ssh <options> -- <destination> /bin/sh -s` and writes a
//! script to the standard input of `ssh`. So only the words `/bin/sh -s`
//! go through the login shell of the remote user (zsh, bash, fish, and so
//! on), and the script always runs in a POSIX shell. The script is one
//! `{ ... }` group: the shell reads all of it before it runs a command.
//!
//! # Listing format
//!
//! A login shell can print text before the script runs (for example an
//! `echo` in `.zshenv`). So the output of each script starts with
//! `MARKER`, and the parser ignores the bytes before it. After the marker,
//! the listing has NUL-terminated fields:
//!
//! ```text
//! <folder> NUL
//! <mode> SP <size> SP <mtime> SP <birth> SP <flags> SP <name> NUL   (one for each entry)
//! NUL
//! <name> NUL                (one for each symlink to a folder)
//! NUL
//! end NUL
//! ```
//!
//! - `<folder>` is the real absolute path of the folder (`$PWD`).
//! - `<mode>` is the `ls -l` text of the mode, for example `drwxr-xr-x`.
//! - `<mtime>` and `<birth>` are seconds since 1970, with an optional
//!   fraction. A `<birth>` of `0` or `-` is not known.
//! - `<flags>` is the decimal `st_flags` value (BSD), or `0`.
//! - A name never holds NUL or `/`, and an empty field ends a section. So
//!   the format is safe for all file names. The `end` field shows that the
//!   output is complete.
//!
//! macOS and the BSDs make the records with BSD `stat -f` and a `/` after
//! each record (`/` is not valid in a name), then `tr` changes `/` to NUL.
//! Other systems use GNU `stat --printf`. The script reads `uname -s` to
//! choose, so the app needs no second round trip.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::fs::{Entry, EntryKind};

/// The bytes before the output of each script. The parser ignores the
/// bytes before them.
pub const MARKER: &[u8] = b"\0bf-remote-1\0";

/// The `UF_HIDDEN` file flag (BSD). Finder hides files with this flag.
const UF_HIDDEN: u32 = 0x8000;

/// The exit codes of the scripts. `ssh` itself exits with 255.
pub const EXIT_NOT_FOUND: i32 = 3;
pub const EXIT_NO_ACCESS: i32 = 4;
pub const EXIT_NOT_A_FOLDER: i32 = 5;
pub const EXIT_NOT_A_FILE: i32 = 6;
/// The exit code of `ssh` when the connection or the login fails.
pub const EXIT_SSH: i32 = 255;

/// The longest control socket path that `ssh` accepts on macOS: a socket
/// address has 104 bytes for the path (with the NUL at the end), and `ssh`
/// adds 17 characters (`.` and 16 random characters) to the name while it
/// makes a new socket.
pub const MAX_CONTROL_PATH: usize = 104 - 1 - 17;

/// One remote folder: the value of the `remote` key, `[user@]host[:path]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Remote {
    /// The SSH destination, `[user@]host`. The host can be an alias from
    /// `~/.ssh/config`. An IPv6 address has no brackets here.
    pub destination: String,
    /// The start folder: an absolute path, `~`, or `~/<path>`. The `~` is
    /// the home folder of the user on the remote host. Default: `~`.
    pub path: String,
}

impl Remote {
    /// Parses `[user@]host[:path]`. An IPv6 address goes in brackets:
    /// `admin@[fd7a::1]:/Users`. An empty path is the home folder (`~`).
    pub fn parse(text: &str) -> Result<Remote, String> {
        let text = text.trim();
        let bad = |reason: &str| format!("invalid remote \"{text}\": {reason}");
        if text.is_empty() {
            return Err("remote needs a host, for example admin@mac-mini".to_string());
        }
        // The user part ends at the first `@` before any `:`, `[`, or `/`.
        let (user, rest) = match text.find(['@', ':', '[', '/']) {
            Some(i) if text.as_bytes()[i] == b'@' => (Some(&text[..i]), &text[i + 1..]),
            _ => (None, text),
        };
        let (host, path, bracketed) = if let Some(inner) = rest.strip_prefix('[') {
            let end = inner.find(']').ok_or_else(|| bad("a [ has no ]"))?;
            let path = match &inner[end + 1..] {
                "" => None,
                after => Some(
                    after
                        .strip_prefix(':')
                        .ok_or_else(|| bad("use : between the host and the path"))?,
                ),
            };
            (&inner[..end], path, true)
        } else {
            match rest.split_once(':') {
                Some((host, path)) => (host, Some(path), false),
                None => (rest, None, false),
            }
        };
        if let Some(user) = user {
            if !valid_name(user, false) {
                return Err(bad("the user name is not valid"));
            }
        }
        if !valid_name(host, bracketed) {
            return Err(bad("the host is not valid"));
        }
        let path = clean_start_path(path.unwrap_or("").trim()).map_err(|r| bad(&r))?;
        let destination = match user {
            Some(user) => format!("{user}@{host}"),
            None => host.to_string(),
        };
        Ok(Remote { destination, path })
    }
}

impl fmt::Display for Remote {
    /// The text form that `parse` reads again: `admin@mac-mini:/Users`.
    /// The default path `~` does not show.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&destination_text(&self.destination))?;
        if self.path != "~" {
            write!(f, ":{}", self.path)?;
        }
        Ok(())
    }
}

/// A user or host name: letters, digits, `.`, `_`, and `-`, and not a
/// first `-` (so `ssh` cannot read it as an option). In brackets, a host
/// can also have `:` and `%` (an IPv6 address with a zone).
fn valid_name(name: &str, bracketed: bool) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name != "."
        && name != ".."
        && name.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '.' | '_' | '-')
                || (bracketed && matches!(c, ':' | '%'))
        })
}

/// The start path of a remote: `~` for an empty path, a clean absolute
/// path, or `~` and a clean relative path.
fn clean_start_path(path: &str) -> Result<String, String> {
    if path.is_empty() || path == "~" {
        return Ok("~".to_string());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        let rest = crate::fs::normalize(Path::new(rest));
        if rest.starts_with("..") {
            return Err("the path must not go above ~".to_string());
        }
        if rest == Path::new(".") {
            return Ok("~".to_string());
        }
        return Ok(format!("~/{}", rest.display()));
    }
    if path.starts_with('/') {
        return Ok(crate::fs::normalize(Path::new(path)).display().to_string());
    }
    Err("the path must start with / or ~".to_string())
}

/// `user@host`, with brackets around an IPv6 address.
fn destination_text(destination: &str) -> String {
    let (user, host) = match destination.split_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, destination),
    };
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    match user {
        Some(user) => format!("{user}@{host}"),
        None => host,
    }
}

/// The text of a remote folder for the title, the messages, and
/// `copy_path`: `host:/path`. `Remote::parse` reads it again.
pub fn display(destination: &str, path: &Path) -> String {
    format!("{}:{}", destination_text(destination), path.display())
}

/// The remote path for text that the user typed in the remote folder
/// `cwd` (the location field). `~` and `~/x` stay: the remote host knows
/// its home folder. An absolute path, and a path relative to `cwd`, become
/// clean absolute paths.
pub fn resolve_input(input: &str, cwd: &Path) -> PathBuf {
    let input = input.trim();
    if input.is_empty() {
        return cwd.to_path_buf();
    }
    if input == "~" || input.starts_with("~/") {
        return match clean_start_path(input) {
            Ok(path) => PathBuf::from(path),
            Err(_) => PathBuf::from("~"),
        };
    }
    crate::fs::normalize(&cwd.join(input))
}

// ---- ssh ----

/// The arguments of `ssh` for one script on `destination`:
///
/// - `BatchMode=yes`: `ssh` never asks for a password or a host key. A
///   login that needs a prompt fails at once with a message.
/// - `ConnectTimeout` and `ServerAlive*`: a host that does not answer
///   fails in seconds.
/// - `ControlMaster=auto` and `ControlPersist=10m` with the socket
///   `control`: the first command opens a connection, and the commands of
///   the next 10 minutes use it. A folder then loads in one round trip.
///   Without `control`, each command opens its own connection.
/// - No forwarding, no local command, no remote command from the user
///   config: the connection only runs the script.
///
/// The user config (`~/.ssh/config`), the keys, the agent, and
/// `known_hosts` apply as for a normal `ssh`.
pub fn ssh_args(destination: &str, control: Option<&Path>) -> Vec<OsString> {
    let mut options: Vec<OsString> = [
        "BatchMode=yes",
        "ConnectTimeout=10",
        "ServerAliveInterval=15",
        "ServerAliveCountMax=2",
        "ClearAllForwardings=yes",
        "ForwardAgent=no",
        "ForwardX11=no",
        "PermitLocalCommand=no",
        "RemoteCommand=none",
        "LogLevel=ERROR",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    match control {
        Some(path) => {
            options.push("ControlMaster=auto".into());
            let mut text = OsString::from("ControlPath=");
            text.push(path.as_os_str());
            options.push(text);
            options.push("ControlPersist=10m".into());
        }
        None => {
            options.push("ControlMaster=no".into());
            options.push("ControlPath=none".into());
        }
    }
    let mut args = vec![OsString::from("-T")];
    for option in options {
        args.push("-o".into());
        args.push(option);
    }
    for word in ["--", destination, "/bin/sh", "-s"] {
        args.push(word.into());
    }
    args
}

/// The control socket for `destination` in the private folder `dir`: a
/// 16-digit hash of the destination. `None` when the path is too long for
/// `ssh` (`MAX_CONTROL_PATH`), or when it has a character that the option
/// syntax of `ssh` changes (white space, quotes, `\`, `%`). The app then
/// opens a new connection for each command.
pub fn control_path(dir: &Path, destination: &str) -> Option<PathBuf> {
    let path = dir.join(format!("{:016x}", fnv1a(destination.as_bytes())));
    let bytes = path.as_os_str().as_bytes();
    let plain = !bytes
        .iter()
        .any(|b| b.is_ascii_whitespace() || matches!(b, b'"' | b'\'' | b'\\' | b'%'));
    (plain && bytes.len() <= MAX_CONTROL_PATH).then_some(path)
}

/// The 64-bit FNV-1a hash. It is stable between versions of the app.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The local copy of the remote file `remote` on `destination`:
/// `<root>/<host>/<remote path>`. Characters of the destination that are
/// not letters, digits, `.`, `_`, `-`, or `@` become `_`. `None` when
/// `remote` is not a clean absolute path of a file (no `.` or `..`).
pub fn cache_path(root: &Path, destination: &str, remote: &Path) -> Option<PathBuf> {
    let mut host: String = destination
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if host.is_empty() || host.starts_with('.') {
        host.insert(0, '_');
    }
    let mut out = root.join(host);
    let mut components = remote.components();
    if components.next() != Some(Component::RootDir) {
        return None;
    }
    let mut parts = 0;
    for component in components {
        match component {
            Component::Normal(part) => out.push(part),
            _ => return None,
        }
        parts += 1;
    }
    (parts > 0).then_some(out)
}

// ---- Scripts ----

/// Quotes `bytes` for a POSIX shell: `'text'`, and each `'` as `'\''`.
pub fn quote(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 2);
    out.push(b'\'');
    for &b in bytes {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

/// The shell word for a remote path: `"$HOME"` for `~`, `"$HOME"/'rest'`
/// for `~/rest`, and the quoted path for all other paths.
pub fn shell_path(path: &Path) -> Vec<u8> {
    let bytes = path.as_os_str().as_bytes();
    if bytes == b"~" {
        return b"\"$HOME\"".to_vec();
    }
    if let Some(rest) = bytes.strip_prefix(b"~/") {
        let mut out = b"\"$HOME\"/".to_vec();
        out.extend(quote(rest));
        return out;
    }
    quote(bytes)
}

/// Puts the shell word of `path` in place of `@PATH@` in `template`.
fn script(template: &str, path: &Path) -> Vec<u8> {
    let (before, after) = template.split_once("@PATH@").unwrap_or((template, ""));
    let mut out = before.as_bytes().to_vec();
    out.extend(shell_path(path));
    out.extend_from_slice(after.as_bytes());
    out
}

/// The listing script. See the module docs for the output.
const LIST_SCRIPT: &str = r#"{
d=@PATH@
if ! cd -- "$d" 2>/dev/null; then
  if [ ! -e "$d" ]; then exit 3; fi
  if [ ! -d "$d" ]; then exit 5; fi
  exit 4
fi
LC_ALL=C
export LC_ALL
printf '\0bf-remote-1\0%s\0' "$PWD"
case $(uname -s 2>/dev/null) in
Darwin|*BSD|DragonFly)
  printf '%s\0' .[!.]* ..?* * | /usr/bin/xargs -0 /usr/bin/stat -n -f '%Sp %z %m %B %f %N/' -- 2>/dev/null | /usr/bin/tr / '\000' ;;
*)
  printf '%s\0' .[!.]* ..?* * | xargs -0 stat --printf '%A %s %Y %W 0 %n\0' -- 2>/dev/null ;;
esac
printf '\0'
for f in .[!.]* ..?* *; do
  if [ -L "$f" ] && [ -d "$f" ]; then printf '%s\0' "$f"; fi
done
printf '\0end\0'
}
"#;

/// The copy script: the marker, then the bytes of the file.
const FETCH_SCRIPT: &str = r#"{
f=@PATH@
if [ -d "$f" ]; then exit 6; fi
if [ ! -e "$f" ]; then exit 3; fi
if [ ! -f "$f" ]; then exit 6; fi
if [ ! -r "$f" ]; then exit 4; fi
printf '\0bf-remote-1\0'
exec cat -- "$f"
}
"#;

/// The script that lists the folder `path` (absolute, `~`, or `~/x`).
pub fn list_script(path: &Path) -> Vec<u8> {
    script(LIST_SCRIPT, path)
}

/// The script that writes the bytes of the file `path` after `MARKER`.
pub fn fetch_script(path: &Path) -> Vec<u8> {
    script(FETCH_SCRIPT, path)
}

/// The index after the first `MARKER` in `bytes`, or `None`.
pub fn find_marker(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(MARKER.len())
        .position(|w| w == MARKER)
        .map(|i| i + MARKER.len())
}

// ---- Listing parser ----

/// A remote folder listing.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    /// The real absolute path of the folder on the remote host.
    pub dir: PathBuf,
    /// All entries, hidden entries too, in no order. Each `path` is a
    /// remote path.
    pub entries: Vec<Entry>,
}

/// Parses the output of `list_script`. A record that is not valid (for
/// example a name with `/`) does not go in the result. `Err` when the
/// output has no marker, or is not complete.
pub fn parse_listing(output: &[u8]) -> Result<Listing, String> {
    let incomplete = || "the listing from the remote host is not complete".to_string();
    let start = find_marker(output).ok_or_else(|| {
        "the remote host did not send a listing (is it macOS or Linux?)".to_string()
    })?;
    let mut fields = output[start..].split(|b| *b == 0);
    let dir = fields.next().ok_or_else(incomplete)?;
    let dir = PathBuf::from(OsStr::from_bytes(dir));
    if !dir.is_absolute() {
        return Err(incomplete());
    }
    let dir = crate::fs::normalize(&dir);
    let mut records = Vec::new();
    loop {
        match fields.next() {
            Some([]) => break,
            Some(record) => records.push(record),
            None => return Err(incomplete()),
        }
    }
    let mut dir_links: HashSet<&[u8]> = HashSet::new();
    loop {
        match fields.next() {
            Some([]) => break,
            Some(name) => {
                dir_links.insert(name);
            }
            None => return Err(incomplete()),
        }
    }
    // `end` and the NUL after it.
    if fields.next() != Some(b"end") || fields.next().is_none() {
        return Err(incomplete());
    }
    let entries = records
        .into_iter()
        .filter_map(|r| parse_record(r, &dir, &dir_links))
        .collect();
    Ok(Listing { dir, entries })
}

/// One entry record: `<mode> <size> <mtime> <birth> <flags> <name>`.
fn parse_record(record: &[u8], dir: &Path, dir_links: &HashSet<&[u8]>) -> Option<Entry> {
    let mut parts = record.splitn(6, |b| *b == b' ');
    let mode = parts.next()?;
    let size: u64 = ascii(parts.next()?)?.parse().ok()?;
    let modified = parse_time(ascii(parts.next()?)?);
    let created = parse_time(ascii(parts.next()?)?).filter(|t| *t != UNIX_EPOCH);
    let flags: u32 = ascii(parts.next()?)?.parse().ok()?;
    let raw_name = parts.next()?;
    if raw_name.is_empty() || raw_name.contains(&b'/') || raw_name == b"." || raw_name == b".." {
        return None;
    }
    if mode.len() < 10 {
        return None;
    }
    let kind = match mode[0] {
        b'd' => EntryKind::Directory,
        b'-' => EntryKind::File,
        b'l' => EntryKind::Symlink,
        _ => EntryKind::Other,
    };
    let executable = kind == EntryKind::File
        && (matches!(mode[3], b'x' | b's')
            || matches!(mode[6], b'x' | b's')
            || matches!(mode[9], b'x' | b't'));
    let name = String::from_utf8_lossy(raw_name).into_owned();
    let mut is_dir = match kind {
        EntryKind::Directory => true,
        EntryKind::Symlink => dir_links.contains(raw_name),
        _ => false,
    };
    let mut is_package = false;
    if is_dir && crate::fs::has_package_extension(&name) {
        is_dir = false;
        is_package = true;
    }
    Some(Entry {
        path: dir.join(OsStr::from_bytes(raw_name)),
        hidden: name.starts_with('.') || flags & UF_HIDDEN != 0,
        name,
        kind,
        is_dir,
        is_package,
        size: if kind == EntryKind::Directory {
            0
        } else {
            size
        },
        modified,
        created,
        executable,
    })
}

fn ascii(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes).ok()
}

/// Seconds since 1970, with an optional sign and fraction:
/// `1730000000`, `1730000000.5`, `-60`. `-` alone is not known.
fn parse_time(text: &str) -> Option<SystemTime> {
    let (negative, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (secs, fraction) = text.split_once('.').unwrap_or((text, ""));
    if secs.is_empty() || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let secs: u64 = secs.parse().ok()?;
    let digits: String = fraction.chars().take(9).collect();
    let nanos: u32 = if digits.is_empty() {
        0
    } else {
        format!("{digits:0<9}").parse().ok()?
    };
    let offset = Duration::new(secs, nanos);
    if negative {
        UNIX_EPOCH.checked_sub(offset)
    } else {
        UNIX_EPOCH.checked_add(offset)
    }
}

// ---- Errors ----

/// What a script did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    List,
    Fetch,
}

/// The message for a failed script. `code` is the exit code of `ssh`
/// (`None` after a signal). `stderr` is the error output of `ssh`.
pub fn error_text(job: Job, code: Option<i32>, stderr: &str, destination: &str) -> String {
    let host = destination_text(destination);
    match code {
        Some(EXIT_NOT_FOUND) => match job {
            Job::List => "no such folder".to_string(),
            Job::Fetch => "no such file".to_string(),
        },
        Some(EXIT_NO_ACCESS) => "permission denied".to_string(),
        Some(EXIT_NOT_A_FOLDER) => "not a folder".to_string(),
        Some(EXIT_NOT_A_FILE) => "not a file".to_string(),
        Some(127) => format!("{host} cannot run /bin/sh"),
        Some(EXIT_SSH) => ssh_error_text(stderr, &host),
        Some(code) => match last_line(stderr) {
            Some(line) => line,
            None => format!("the remote command failed (exit {code})"),
        },
        None => "ssh stopped".to_string(),
    }
}

/// A short message for an error of `ssh` itself.
fn ssh_error_text(stderr: &str, host: &str) -> String {
    let text = stderr.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| text.contains(n));
    if has(&[
        "host key verification failed",
        "remote host identification has changed",
        "no matching host key",
    ]) {
        format!("unknown or changed host key. Connect one time in a terminal: ssh {host}")
    } else if has(&["permission denied", "too many authentication failures"]) {
        format!("SSH key login failed. Test: ssh -o BatchMode=yes {host} true")
    } else if has(&[
        "could not resolve hostname",
        "nodename nor servname",
        "name or service not known",
    ]) {
        format!("unknown host {host}")
    } else if has(&["operation timed out", "connection timed out", "timeout"]) {
        format!("{host} does not answer")
    } else if has(&["connection refused"]) {
        format!("{host} refused the connection. Is Remote Login (SSH) on?")
    } else if has(&["no route to host", "network is unreachable", "host is down"]) {
        format!("{host} is not reachable")
    } else if has(&["connection closed", "connection reset", "broken pipe"]) {
        format!("the connection to {host} closed")
    } else {
        last_line(stderr).unwrap_or_else(|| "ssh failed".to_string())
    }
}

/// The last line of `text` that is not empty, at most 200 characters.
fn last_line(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    Some(crate::names::truncate_middle(line, 200))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn remote(text: &str) -> Remote {
        Remote::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
    }

    #[test]
    fn parse_forms() {
        let r = remote("admin@100.91.36.106");
        assert_eq!(r.destination, "admin@100.91.36.106");
        assert_eq!(r.path, "~");
        let r = remote(" mac-mini:/Users/admin/ ");
        assert_eq!(
            (r.destination.as_str(), r.path.as_str()),
            ("mac-mini", "/Users/admin")
        );
        let r = remote("me@host:~/Projects/./x/../y");
        assert_eq!(r.path, "~/Projects/y");
        assert_eq!(remote("host:").path, "~");
        assert_eq!(remote("host:~").path, "~");
        assert_eq!(remote("host:~/").path, "~");
        assert_eq!(remote("host:/a/../..").path, "/");
        let r = remote("admin@[fd7a:115c::1]:/tmp");
        assert_eq!(
            (r.destination.as_str(), r.path.as_str()),
            ("admin@fd7a:115c::1", "/tmp")
        );
        assert_eq!(remote("[::1]").destination, "::1");
        // A path can hold `@` and `:`.
        let r = remote("host:/a@b:c");
        assert_eq!(
            (r.destination.as_str(), r.path.as_str()),
            ("host", "/a@b:c")
        );
    }

    #[test]
    fn parse_rejects_bad_values() {
        for bad in [
            "",
            "   ",
            "-oProxyCommand=x",
            "admin@-host",
            "-me@host",
            "@host",
            "admin@",
            "host name",
            "host;rm",
            "ho$st",
            "host:relative/path",
            "host:~other/x",
            "host:~/../x",
            "[::1",
            "[::1]/x",
            "..",
            "a@b@c",
        ] {
            assert!(Remote::parse(bad).is_err(), "{bad:?} must fail");
        }
        let err = Remote::parse("host:x").unwrap_err();
        assert!(err.contains("host:x") && err.contains("/ or ~"), "{err}");
    }

    #[test]
    fn display_round_trips() {
        for text in [
            "admin@100.91.36.106",
            "mac-mini:/Users/admin",
            "me@host:~/Projects",
            "admin@[fd7a::1]:/tmp",
            "[::1]",
        ] {
            let r = remote(text);
            assert_eq!(r.to_string(), text);
            assert_eq!(remote(&r.to_string()), r);
        }
        assert_eq!(remote("host:~").to_string(), "host");
        assert_eq!(
            display("admin@host", Path::new("/Users")),
            "admin@host:/Users"
        );
        assert_eq!(display("::1", Path::new("/")), "[::1]:/");
        assert_eq!(
            remote(&display("admin@::1", Path::new("/a b"))).path,
            "/a b"
        );
    }

    #[test]
    fn resolve_typed_paths() {
        let cwd = Path::new("/Users/admin/Projects");
        assert_eq!(resolve_input("..", cwd), PathBuf::from("/Users/admin"));
        assert_eq!(resolve_input("src/../docs", cwd), cwd.join("docs"));
        assert_eq!(resolve_input("/etc/./x", cwd), PathBuf::from("/etc/x"));
        assert_eq!(resolve_input("~", cwd), PathBuf::from("~"));
        assert_eq!(resolve_input("~/a/", cwd), PathBuf::from("~/a"));
        assert_eq!(resolve_input("~/../x", cwd), PathBuf::from("~"));
        assert_eq!(resolve_input("  ", cwd), cwd.to_path_buf());
    }

    #[test]
    fn quote_is_safe_for_any_text() {
        // The shell gives back the same bytes for hard names.
        for name in [
            "",
            "a b",
            "it's",
            "$(touch /tmp/bf-pwned)",
            "`id`",
            "x\ny",
            "*",
            "-n",
            "\\'\"",
            "tab\there",
        ] {
            let mut cmd = b"printf '%s' ".to_vec();
            cmd.extend(quote(name.as_bytes()));
            let out = Command::new("/bin/sh")
                .arg("-c")
                .arg(OsStr::from_bytes(&cmd))
                .output()
                .unwrap();
            assert_eq!(out.stdout, name.as_bytes(), "{name:?}");
        }
    }

    #[test]
    fn ssh_args_never_prompt_and_end_with_the_script_shell() {
        let control = Path::new("/tmp/bf/0123");
        let args = ssh_args("admin@host", Some(control));
        let text: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
        for option in [
            "BatchMode=yes",
            "ConnectTimeout=10",
            "ControlMaster=auto",
            "ControlPath=/tmp/bf/0123",
            "ControlPersist=10m",
            "RemoteCommand=none",
        ] {
            let i = text
                .iter()
                .position(|a| a == option)
                .unwrap_or_else(|| panic!("{option}"));
            assert_eq!(text[i - 1], "-o");
        }
        assert_eq!(
            text[text.len() - 4..],
            ["--", "admin@host", "/bin/sh", "-s"]
        );
        let text: Vec<String> = ssh_args("h", None)
            .iter()
            .map(|a| a.to_string_lossy().into())
            .collect();
        assert!(text.contains(&"ControlMaster=no".to_string()));
        assert!(text.contains(&"ControlPath=none".to_string()));
        assert!(!text.iter().any(|a| a.starts_with("ControlPersist")));
    }

    #[test]
    fn control_path_is_short_and_plain() {
        let dir = Path::new("/var/folders/9x/abc/T/bf-ssh-501");
        let a = control_path(dir, "admin@100.91.36.106").unwrap();
        let b = control_path(dir, "admin@100.91.36.106").unwrap();
        let c = control_path(dir, "mac-mini").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.file_name().unwrap().len(), 16);
        assert!(a.as_os_str().len() <= MAX_CONTROL_PATH);
        let long = PathBuf::from(format!("/{}", "x".repeat(80)));
        assert_eq!(control_path(&long, "h"), None);
        assert_eq!(control_path(Path::new("/tmp/a b"), "h"), None);
        assert_eq!(control_path(Path::new("/tmp/50%"), "h"), None);
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn cache_path_stays_in_the_root() {
        let root = Path::new("/cache/remote");
        assert_eq!(
            cache_path(
                root,
                "admin@100.91.36.106",
                Path::new("/Users/admin/a b.pdf")
            ),
            Some(PathBuf::from(
                "/cache/remote/admin@100.91.36.106/Users/admin/a b.pdf"
            ))
        );
        assert_eq!(
            cache_path(root, "admin@fd7a::1", Path::new("/x")),
            Some(PathBuf::from("/cache/remote/admin@fd7a__1/x"))
        );
        assert_eq!(
            cache_path(root, "..", Path::new("/x")),
            Some(PathBuf::from("/cache/remote/_../x"))
        );
        assert_eq!(
            cache_path(root, "h", Path::new("/a/../../etc/passwd")),
            None
        );
        assert_eq!(cache_path(root, "h", Path::new("relative")), None);
        assert_eq!(cache_path(root, "h", Path::new("/")), None);
        assert_eq!(cache_path(root, "h", Path::new("~/x")), None);
    }

    /// A listing like the macOS script gives: BSD records end with `/`,
    /// and `tr` makes NUL of each `/`.
    fn darwin_fixture() -> Vec<u8> {
        // Text of the login shell, with a NUL, before the marker.
        let mut out = b"Last login: junk from .zshenv\n\0x".to_vec();
        out.extend_from_slice(MARKER);
        out.extend_from_slice(b"/Users/admin\0");
        let records: &[&[u8]] = &[
            b"drwx------ 96 1730000000 1700000000 32768 Library",
            b"-rw-r--r-- 1234 1730000001 1700000001 0 notes.txt",
            b"-rwxr-xr-x 50 1730000002 0 0 run",
            b"lrwxr-xr-x 11 1730000003 1700000003 0 link to dir",
            b"lrwxr-xr-x 9 1730000004 1700000004 0 dead link",
            b"drwxr-xr-x 128 1730000005 1700000005 0 Safari.app",
            b"-rw------- 0 1730000006 1700000006 0 .zshrc",
            b"-rw-r--r-- 3 1730000007 1700000007 0 new\nline",
            b"srwxr-xr-x 0 1730000008 1700000008 0 agent.sock",
            b"-rw-r--r-- 3 1730000009 1700000009 0 a b  c ",
        ];
        for record in records {
            out.extend_from_slice(record);
            out.push(0);
        }
        out.push(0);
        out.extend_from_slice(b"link to dir\0");
        out.push(0);
        out.extend_from_slice(b"end\0");
        out
    }

    fn by_name<'a>(listing: &'a Listing, name: &str) -> &'a Entry {
        listing
            .entries
            .iter()
            .find(|e| e.name == name)
            .unwrap_or_else(|| panic!("no entry {name:?}"))
    }

    #[test]
    fn parse_darwin_listing() {
        let listing = parse_listing(&darwin_fixture()).unwrap();
        assert_eq!(listing.dir, PathBuf::from("/Users/admin"));
        assert_eq!(listing.entries.len(), 10);

        let library = by_name(&listing, "Library");
        assert_eq!(library.kind, EntryKind::Directory);
        assert!(library.is_dir && library.hidden, "UF_HIDDEN hides it");
        assert_eq!(library.size, 0);
        assert_eq!(library.path, PathBuf::from("/Users/admin/Library"));

        let notes = by_name(&listing, "notes.txt");
        assert_eq!((notes.kind, notes.size), (EntryKind::File, 1234));
        assert!(!notes.is_dir && !notes.executable && !notes.hidden);
        assert_eq!(
            notes.modified,
            Some(UNIX_EPOCH + Duration::from_secs(1_730_000_001))
        );
        assert_eq!(
            notes.created,
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_001))
        );

        let run = by_name(&listing, "run");
        assert!(run.executable);
        assert_eq!(run.created, None, "a birth time of 0 is not known");

        let link = by_name(&listing, "link to dir");
        assert_eq!(link.kind, EntryKind::Symlink);
        assert!(link.is_dir);
        assert!(!by_name(&listing, "dead link").is_dir);

        let app = by_name(&listing, "Safari.app");
        assert!(app.is_package && !app.is_dir);
        assert!(by_name(&listing, ".zshrc").hidden);
        assert_eq!(by_name(&listing, "new\nline").size, 3);
        assert_eq!(by_name(&listing, "agent.sock").kind, EntryKind::Other);
        assert_eq!(
            by_name(&listing, "a b  c ").path,
            PathBuf::from("/Users/admin/a b  c ")
        );
    }

    #[test]
    fn parse_gnu_listing() {
        let mut out = MARKER.to_vec();
        out.extend_from_slice(b"/home/me\0");
        out.extend_from_slice(b"-rw-r--r-- 10 1730000000.250000000 0 0 a.txt\0");
        out.extend_from_slice(b"drwxr-xr-x 4096 1730000000 - 0 src\0");
        out.extend_from_slice(b"-rwsr-xr-x 1 1730000000 0 0 suid\0");
        out.extend_from_slice(b"-rw-r--r-- 1 -60 0 0 old\0");
        out.extend_from_slice(b"\0\0end\0");
        let listing = parse_listing(&out).unwrap();
        assert_eq!(listing.dir, PathBuf::from("/home/me"));
        let a = by_name(&listing, "a.txt");
        assert_eq!(
            a.modified,
            Some(UNIX_EPOCH + Duration::new(1_730_000_000, 250_000_000))
        );
        assert_eq!(a.created, None);
        let src = by_name(&listing, "src");
        assert!(src.is_dir && src.size == 0 && src.created.is_none());
        assert!(by_name(&listing, "suid").executable);
        assert_eq!(
            by_name(&listing, "old").modified,
            UNIX_EPOCH.checked_sub(Duration::from_secs(60))
        );
    }

    #[test]
    fn parse_skips_bad_records_and_rejects_bad_output() {
        let mut out = MARKER.to_vec();
        out.extend_from_slice(b"/x\0");
        for bad in [
            &b"-rw-r--r-- 1 1 0 0 ../etc"[..],
            b"-rw-r--r-- 1 1 0 0 a/b",
            b"-rw-r--r-- 1 1 0 0 ..",
            b"-rw-r--r-- 1 1 0 0 ",
            b"-rw-r--r-- x 1 0 0 bad-size",
            b"-rw 1 1 0 0 short-mode",
            b"-rw-r--r-- 1 1 0",
        ] {
            out.extend_from_slice(bad);
            out.push(0);
        }
        out.extend_from_slice(b"-rw-r--r-- 1 1 0 0 good\0\0\0end\0");
        let listing = parse_listing(&out).unwrap();
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["good"]);

        assert!(parse_listing(b"no marker here").is_err());
        let full = darwin_fixture();
        // A cut at any place before the end gives an error, never a short list.
        for cut in [full.len() - 1, full.len() - 5, full.len() / 2] {
            assert!(parse_listing(&full[..cut]).is_err(), "cut at {cut}");
        }
        let mut relative = MARKER.to_vec();
        relative.extend_from_slice(b"x\0\0\0end\0");
        assert!(parse_listing(&relative).is_err());
        let mut empty = MARKER.to_vec();
        empty.extend_from_slice(b"/\0\0\0end\0");
        assert_eq!(parse_listing(&empty).unwrap().entries, []);
    }

    #[test]
    fn error_messages() {
        let host = "admin@100.91.36.106";
        let text = |code, stderr| error_text(Job::List, code, stderr, host);
        assert_eq!(text(Some(3), ""), "no such folder");
        assert_eq!(error_text(Job::Fetch, Some(3), "", host), "no such file");
        assert_eq!(text(Some(4), ""), "permission denied");
        assert_eq!(text(Some(5), ""), "not a folder");
        assert_eq!(error_text(Job::Fetch, Some(6), "", host), "not a file");
        for (stderr, part) in [
            (
                "admin@100.91.36.106: Permission denied (publickey,password).",
                "key login failed",
            ),
            ("Host key verification failed.", "host key"),
            (
                "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@",
                "host key",
            ),
            (
                "ssh: Could not resolve hostname nope: nodename nor servname provided",
                "unknown host",
            ),
            (
                "ssh: connect to host 100.91.36.106 port 22: Operation timed out",
                "does not answer",
            ),
            (
                "ssh: connect to host x port 22: Connection refused",
                "Remote Login",
            ),
            (
                "ssh: connect to host x port 22: No route to host",
                "not reachable",
            ),
            ("Connection closed by 100.91.36.106 port 22", "closed"),
            ("some new error\n\n", "some new error"),
            ("", "ssh failed"),
        ] {
            let message = text(Some(255), stderr);
            assert!(message.contains(part), "{stderr:?} gave {message:?}");
        }
        assert!(text(Some(255), "Permission denied (publickey).").contains(host));
        assert_eq!(text(Some(1), "stat: oops\n"), "stat: oops");
        assert!(text(Some(9), "").contains("exit 9"));
        assert_eq!(text(None, ""), "ssh stopped");
    }

    /// Runs `script` with `/bin/sh -s` like `ssh` does on the remote host:
    /// the script comes on the standard input. `home` is `$HOME`.
    fn run_local(script: &[u8], home: &Path) -> (Option<i32>, Vec<u8>) {
        use std::io::Write as _;
        let mut child = Command::new("/bin/sh")
            .arg("-s")
            .env("HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(script).unwrap();
        let out = child.wait_with_output().unwrap();
        (out.status.code(), out.stdout)
    }

    /// The real script on this Mac, through `/bin/sh -s`, as `ssh` runs it
    /// on the remote host. No network.
    #[test]
    fn list_script_runs_in_a_local_shell() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        // The folder name needs quotes in the shell.
        let root = home.join("it's a dir");
        std::fs::create_dir(&root).unwrap();
        let hard = [
            "plain.txt",
            "it's",
            "a b",
            "new\nline",
            "-n",
            "$(x)",
            "*",
            ".hid",
        ];
        for name in hard {
            std::fs::write(root.join(name), name.as_bytes()).unwrap();
        }
        std::fs::create_dir(root.join("sub dir")).unwrap();
        std::fs::create_dir(root.join("Tool.app")).unwrap();
        std::os::unix::fs::symlink("sub dir", root.join("to sub")).unwrap();
        std::os::unix::fs::symlink("plain.txt", root.join("to file")).unwrap();
        std::fs::set_permissions(
            root.join("plain.txt"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();

        let (code, out) = run_local(&list_script(&root), &home);
        assert_eq!(code, Some(0));
        let listing = parse_listing(&out).unwrap();
        assert_eq!(listing.dir, root);
        // The same folder from `~`: the host gives the real path.
        let (code, out) = run_local(&list_script(Path::new("~/it's a dir")), &home);
        assert_eq!(code, Some(0));
        assert_eq!(parse_listing(&out).unwrap(), listing);
        let mut names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        names.sort_unstable();
        let mut expected: Vec<&str> = hard.to_vec();
        expected.extend(["sub dir", "Tool.app", "to sub", "to file"]);
        expected.sort_unstable();
        assert_eq!(names, expected);
        let local = crate::fs::read_dir(&root).unwrap();
        for entry in &listing.entries {
            let same = local.iter().find(|e| e.name == entry.name).unwrap();
            assert_eq!(entry.path, same.path, "{:?}", entry.name);
            assert_eq!(entry.kind, same.kind, "{:?}", entry.name);
            assert_eq!(entry.is_dir, same.is_dir, "{:?}", entry.name);
            assert_eq!(entry.is_package, same.is_package, "{:?}", entry.name);
            assert_eq!(entry.size, same.size, "{:?}", entry.name);
            assert_eq!(entry.hidden, same.hidden, "{:?}", entry.name);
            assert_eq!(entry.executable, same.executable, "{:?}", entry.name);
            let secs = |t: Option<SystemTime>| {
                t.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
            };
            assert_eq!(
                secs(entry.modified),
                secs(same.modified),
                "{:?}",
                entry.name
            );
        }

        let (code, out) = run_local(&list_script(&root.join("missing")), &home);
        assert_eq!((code, out.is_empty()), (Some(EXIT_NOT_FOUND), true));
        let (code, _) = run_local(&list_script(&root.join("plain.txt")), &home);
        assert_eq!(code, Some(EXIT_NOT_A_FOLDER));
        let empty = root.join("sub dir");
        let (code, out) = run_local(&list_script(&empty), &home);
        assert_eq!(code, Some(0));
        assert_eq!(parse_listing(&out).unwrap().entries, []);
    }

    #[test]
    fn fetch_script_runs_in_a_local_shell() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let file = home.join("it's a file");
        let bytes: Vec<u8> = (0..=255u8).cycle().take(70_000).collect();
        std::fs::write(&file, &bytes).unwrap();
        for path in [file.as_path(), Path::new("~/it's a file")] {
            let (code, out) = run_local(&fetch_script(path), home);
            assert_eq!(code, Some(0), "{path:?}");
            let at = find_marker(&out).unwrap();
            assert_eq!(&out[at..], &bytes[..], "{path:?}");
        }
        let (code, _) = run_local(&fetch_script(home), home);
        assert_eq!(code, Some(EXIT_NOT_A_FILE));
        let (code, _) = run_local(&fetch_script(&home.join("missing")), home);
        assert_eq!(code, Some(EXIT_NOT_FOUND));
    }
}
