# Better Finder

A fast, keyboard-first file explorer for macOS. It is written in Rust. You configure it with a dotfile, like Ghostty.

- **Fast.** Native AppKit. A custom list view draws only the visible rows. Directory reads run on a background thread.
- **Keyboard first.** It has vim keys and Finder keys by default. You can bind any key or key sequence to any action. The page keys scroll smoothly, like nvim with smooth scroll: the view and the cursor move together.
- **Configured with a dotfile.** One plain text file, `~/.config/better-finder/config`. The app reloads it without a restart.
- **Native Quick Look.** Press `Space` to preview a file, like in Finder.
- **Finder-style UI.** It has a glass sidebar with Favorites and Locations, and a toolbar with a field that filters the list or goes to a path. It has a column header with sort, a Kind column, and a path bar. The system theme follows the macOS accent color. Rename happens inline in the list.

See [docs/UI.md](docs/UI.md) for the UI design contract.

## Build and run

```sh
cargo run --release -p bf-app -- ~/Downloads
```

To make an app bundle in `dist/`:

```sh
scripts/bundle.sh
open "dist/Better Finder.app"
```

To install the app in `/Applications`, so that Spotlight and Raycast find it:

```sh
scripts/bundle.sh --install
```

The app icon is `assets/AppIcon.icns`. To change the icon, edit `scripts/make-icon.swift`, then run `swift scripts/make-icon.swift`.

## Configuration

The config file is `~/.config/better-finder/config`. If `XDG_CONFIG_HOME` is set, the file is `$XDG_CONFIG_HOME/better-finder/config`. Press `cmd+,` to open the file. The app creates it from a template when it does not exist. Press `cmd+shift+,` to reload it.

```ini
# Look
font-family = "JetBrains Mono"
font-size = 14
# system follows the macOS colors; dark gives the custom palette
theme = system
background = #1e1e2e
foreground = #cdd6f4
selection-background = #45475a
background-opacity = 0.95

# Sidebar and bars
sidebar = true
sidebar-item = ~/Apps
toolbar = true
column-header = true
path-bar = true

# Behavior
show-hidden = false
sort-by = name
directories-first = true
columns = size,kind,modified
date-format = finder
terminal = Ghostty
editor = nvim

# Scroll
smooth-scroll = true
smooth-scroll-duration = 150

# Keys
keybind = g>p=goto:~/Apps/personal-projects
keybind = d=unbind
keybind = ctrl+n=cursor_down
keybind = ctrl+p=cursor_up
```

When `editor` is set, `Enter` opens a text file in that editor in a new window of the `terminal` app (Ghostty or Terminal). Other files open in the default app. macOS asks once to allow Better Finder to control the terminal app.

These commands print reference information:

```sh
better-finder +show-config --default   # all keys, defaults, and help
better-finder +list-keybinds           # the active key bindings
better-finder +list-actions            # all actions
better-finder +validate-config         # check the config file for errors
```

## Default keys

| Keys | Action |
| --- | --- |
| `j` `k`, arrows | Move the cursor |
| `l`, `Enter`, `cmd+down` | Open a directory or a file |
| `h`, `Backspace`, `cmd+up` | Go to the parent directory |
| `H` `L`, `cmd+[` `cmd+]` | Back and forward |
| `gg` `G` | Top and bottom |
| `ctrl+d` `ctrl+u` | Half page down and up (smooth scroll) |
| `ctrl+f` `ctrl+b`, `PageDown` `PageUp` | Page down and up (smooth scroll) |
| `Space` | Quick Look |
| `/`, `cmd+f` | Filter, or go to a path: text that starts with `~`, contains `/`, or is `..` shows the matching folders. `Tab` completes, `Enter` goes |
| `Tab` | Focus the sidebar (`Tab` again gives the focus back to the list) |
| `cmd+ctrl+s` | Show or hide the sidebar |
| `.`, `cmd+shift+.` | Show or hide hidden files |
| `gh` `gd` `ge` `go` `ga` `gc` `gr` | Go to home, Downloads, Desktop, Documents, Applications, `~/.config`, `/` |
| `cmd+shift+h` `cmd+shift+d` `cmd+shift+o` `cmd+alt+l` | Finder go to: Home, Desktop, Documents, Downloads |
| `cmd+shift+a` `cmd+shift+u` `cmd+shift+i` | Finder go to: Applications, Utilities, iCloud Drive |
| `cmd+shift+g` | Go to a folder (prompt) |
| `sn` `ss` `sm` `se` `sr` | Sort by name, size, date, extension; reverse the order |
| `r` | Rename |
| `cmd+shift+n` | New folder |
| `cmd+Backspace` | Move to Trash |
| `cc` `cn` | Copy the path or the name |
| `T` | Open a terminal here |
| `?` | Show all key bindings. `j` `k` and the page keys scroll the overlay. `q`, `Escape`, or `?` closes it |
| `cmd+,` `cmd+shift+,` | Open and reload the config |

## Architecture

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
