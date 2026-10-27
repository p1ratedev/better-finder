//! Better Finder: the macOS app. The implementation follows docs/ARCHITECTURE.md.
//!
//! `main` reads the command line. The `+` commands print reference
//! information and exit at once, without `NSApplication`. All other
//! command lines start the app.

mod app;
mod columns;
mod context_menu;
mod debug;
mod editor;
mod file_ops;
mod file_viewer;
mod goto_panel;
mod header;
mod help;
mod icons;
mod list_view;
mod loader;
mod prompt;
mod quicklook;
mod sidebar;
mod smooth;
mod status_bar;
mod theme;
mod toolbar;
mod volumes;
mod watcher;
mod window;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use bf_core::config::default_config_template;
use bf_core::fs::display_path;
use bf_core::{Action, Config};

const USAGE: &str = "\
Better Finder: a fast, keyboard-first file explorer for macOS.

Usage:
  better-finder [PATH]               Open PATH (a folder, or a file in its folder)
  better-finder +show-config         Print the config file path and its text
  better-finder +show-config --default
                                     Print all config keys with defaults and help
  better-finder +list-keybinds       Print the active key bindings
  better-finder +list-actions        Print all actions
  better-finder +validate-config     Check the config file; exit 1 on errors
  better-finder --help | --version

Developer options:
  --debug-screenshot <png>           Save the window as a PNG after the first load, then quit
  --debug-keys \"<t1> <t2> ...\"       Feed key triggers (keymap syntax) before the screenshot.
                                     @<ms> waits, @load waits for a load,
                                     @w<w>x<h> sets the window size, @eject:<name>
                                     clicks the eject button of a sidebar row,
                                     @menu:<row> and @menu:bg print the context menu
                                     of a row or of the folder. Prefixes:
                                     ~ key repeat, ! keymap also with the prompt open,
                                     ^ same turn as the previous key (no wait for a load)
  --debug-appearance light|dark      Force the appearance (for the system and auto themes)

Environment:
  BETTER_FINDER_TRACE=1              Print timings to stderr
  BETTER_FINDER_SCREENCAPTURE=own    The screenshot shows the real window (glass too).
                                     The value 1 uses screencapture (needs the permission)
  XDG_CONFIG_HOME                    The config is $XDG_CONFIG_HOME/better-finder/config
";

enum Command {
    Run {
        path: Option<String>,
        screenshot: Option<PathBuf>,
        keys: Option<String>,
        appearance: Option<bool>,
    },
    Help,
    Version,
    ShowConfig {
        default: bool,
    },
    ListKeybinds,
    ListActions,
    ValidateConfig,
}

fn parse_args(args: Vec<String>) -> Result<Command, String> {
    let mut path = None;
    let mut screenshot = None;
    let mut keys = None;
    let mut appearance = None;
    let mut command = None;
    let mut default = false;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--default" => default = true,
            "--debug-screenshot" => {
                let value = iter.next().ok_or("--debug-screenshot needs a file path")?;
                screenshot = Some(PathBuf::from(value));
            }
            "--debug-keys" => {
                keys = Some(iter.next().ok_or("--debug-keys needs a list of triggers")?);
            }
            "--debug-appearance" => {
                appearance = match iter.next().as_deref() {
                    Some("light") => Some(false),
                    Some("dark") => Some(true),
                    _ => return Err("--debug-appearance needs light or dark".to_string()),
                };
            }
            "+show-config" => command = Some(Command::ShowConfig { default: false }),
            "+list-keybinds" => command = Some(Command::ListKeybinds),
            "+list-actions" => command = Some(Command::ListActions),
            "+validate-config" => command = Some(Command::ValidateConfig),
            // macOS adds a process serial number to apps that Finder starts.
            s if s.starts_with("-psn_") => {}
            s if s.starts_with('+') => return Err(format!("unknown command: {s}")),
            s if s.starts_with('-') && s != "-" => return Err(format!("unknown option: {s}")),
            _ => {
                if path.is_some() {
                    return Err(format!("too many paths: {arg}"));
                }
                path = Some(arg);
            }
        }
    }
    match command {
        Some(Command::ShowConfig { .. }) => Ok(Command::ShowConfig { default }),
        Some(other) => Ok(other),
        None if default => Err("--default goes with +show-config".to_string()),
        None => Ok(Command::Run {
            path,
            screenshot,
            keys,
            appearance,
        }),
    }
}

/// Writes to stdout. A closed pipe (`| head`) is not an error.
fn print(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

fn main() -> ExitCode {
    debug::mark_start();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse_args(args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("better-finder: {message}\nRun `better-finder --help` for the usage.");
            return ExitCode::from(2);
        }
    };
    match command {
        Command::Help => {
            print(USAGE);
            ExitCode::SUCCESS
        }
        Command::Version => {
            print(&format!("better-finder {}\n", env!("CARGO_PKG_VERSION")));
            ExitCode::SUCCESS
        }
        Command::ShowConfig { default: true } => {
            print(&default_config_template());
            ExitCode::SUCCESS
        }
        Command::ShowConfig { default: false } => show_config(),
        Command::ListKeybinds => {
            let result = Config::load();
            for d in &result.diagnostics {
                eprintln!("warning: {d}");
            }
            let mut text = String::new();
            for (trigger, action) in result.config.keymap.bindings() {
                text.push_str(&format!("keybind = {trigger}={action}\n"));
            }
            print(&text);
            ExitCode::SUCCESS
        }
        Command::ListActions => {
            let catalog = Action::catalog();
            let width = catalog
                .iter()
                .map(|(name, _)| name.len())
                .max()
                .unwrap_or(0);
            let mut text = String::new();
            for (name, description) in catalog {
                text.push_str(&format!("{name:width$}  {description}\n"));
            }
            print(&text);
            ExitCode::SUCCESS
        }
        Command::ValidateConfig => validate_config(),
        Command::Run {
            path,
            screenshot,
            keys,
            appearance,
        } => {
            let keys = match keys.as_deref().map(debug::parse_keys).transpose() {
                Ok(keys) => keys.unwrap_or_default(),
                Err(message) => {
                    eprintln!("better-finder: {message}");
                    return ExitCode::from(2);
                }
            };
            app::run(app::RunOptions {
                path,
                screenshot,
                keys,
                appearance,
            })
        }
    }
}

fn show_config() -> ExitCode {
    let path = Config::default_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            print(&format!("# {}\n{text}", path.display()));
            if !text.ends_with('\n') && !text.is_empty() {
                print("\n");
            }
            ExitCode::SUCCESS
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            print(&format!(
                "no config file: {}\nRun `better-finder +show-config --default` to see all keys.\n",
                path.display()
            ));
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("better-finder: cannot read {}: {err}", path.display());
            ExitCode::FAILURE
        }
    }
}

fn validate_config() -> ExitCode {
    let path = Config::default_path();
    let result = Config::load();
    if result.diagnostics.is_empty() {
        print(&format!("OK: {}\n", path.display()));
        if !path.exists() {
            print(&format!(
                "note: {} does not exist; the defaults apply.\n",
                display_path(&path)
            ));
        }
        ExitCode::SUCCESS
    } else {
        let mut text = String::new();
        for d in &result.diagnostics {
            text.push_str(&format!("{d}\n"));
        }
        print(&text);
        ExitCode::FAILURE
    }
}
