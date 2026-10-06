//! Opens a text file in the editor of the user, in a new terminal window.
//!
//! The app asks the terminal app to open the window through `osascript`.
//! - Ghostty gets the command, the folder, and the file as separate items.
//!   The file path travels in the `BF_FILE` environment variable, so it needs no quoting.
//! - Terminal gets one shell command line. `sh_quote` quotes the paths.
//!
//! The UI thread never waits for `osascript`. The caller starts a thread that waits.

use std::ffi::OsString;
use std::path::Path;

const GHOSTTY_SCRIPT: &str = r#"on run argv
	set theCommand to item 1 of argv
	set theDir to item 2 of argv
	set theFile to item 3 of argv
	tell application "Ghostty"
		set cfg to {command:theCommand, initial working directory:theDir, environment variables:{"BF_FILE=" & theFile}}
		new window with configuration cfg
		activate
	end tell
end run"#;

const TERMINAL_SCRIPT: &str = r#"on run argv
	set theCommand to item 1 of argv
	tell application "Terminal"
		do script theCommand
		activate
	end tell
end run"#;

/// The terminal apps that the `editor` setting supports.
#[derive(Debug, PartialEq, Eq)]
enum Terminal {
    Ghostty,
    TerminalApp,
}

/// The name of the terminal app in lowercase, without a trailing `.app`.
fn normalize_terminal(terminal: &str) -> String {
    let name = terminal.trim().to_lowercase();
    name.strip_suffix(".app").unwrap_or(&name).to_string()
}

fn terminal_kind(terminal: &str) -> Option<Terminal> {
    match normalize_terminal(terminal).as_str() {
        "ghostty" => Some(Terminal::Ghostty),
        "terminal" => Some(Terminal::TerminalApp),
        _ => None,
    }
}

/// Wraps `text` in single quotes for a POSIX shell.
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The login shell of the user, or `/bin/zsh` when `SHELL` is not an absolute path.
fn login_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| shell.starts_with('/'))
        .unwrap_or_else(|| "/bin/zsh".to_string())
}

fn osascript_args(script: &str, items: &[&str]) -> Vec<OsString> {
    let mut args = vec![OsString::from("-e"), OsString::from(script)];
    args.extend(items.iter().map(OsString::from));
    args
}

/// The arguments for `/usr/bin/osascript`.
/// Gives `None` when the terminal app is not supported.
pub fn osascript_command(path: &Path, editor: &str, terminal: &str) -> Option<Vec<OsString>> {
    let kind = terminal_kind(terminal)?;
    let editor = editor.trim();
    let file = path.to_string_lossy();
    let dir = path
        .parent()
        .map(|dir| dir.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_string());
    Some(match kind {
        Terminal::Ghostty => {
            let command = format!("{} -lic 'exec {editor} \"$BF_FILE\"'", login_shell());
            osascript_args(GHOSTTY_SCRIPT, &[&command, &dir, &file])
        }
        Terminal::TerminalApp => {
            let command = format!("cd {} && exec {editor} {}", sh_quote(&dir), sh_quote(&file));
            osascript_args(TERMINAL_SCRIPT, &[&command])
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sh_quote_wraps_and_escapes() {
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn terminal_names_are_normalized() {
        assert_eq!(terminal_kind(" Ghostty.APP "), Some(Terminal::Ghostty));
        assert_eq!(terminal_kind("Terminal"), Some(Terminal::TerminalApp));
        assert_eq!(terminal_kind("iTerm"), None);
        assert_eq!(terminal_kind(""), None);
    }

    #[test]
    fn terminal_app_command_quotes_the_paths() {
        let args = osascript_command(Path::new("/tmp/a b/it's.txt"), " nvim ", "Terminal").unwrap();
        assert_eq!(
            args.last().unwrap(),
            "cd '/tmp/a b' && exec nvim '/tmp/a b/it'\\''s.txt'"
        );
    }

    #[test]
    fn ghostty_command_passes_the_file_in_the_environment() {
        let args = osascript_command(Path::new("/tmp/x.txt"), "nvim", "ghostty").unwrap();
        assert_eq!(args.len(), 5);
        assert!(
            args[2]
                .to_string_lossy()
                .ends_with("-lic 'exec nvim \"$BF_FILE\"'")
        );
        assert_eq!(args[4], "/tmp/x.txt");
    }
}
