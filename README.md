# Better Finder

A fast, keyboard-first file explorer for macOS. It is written in Rust. You configure it with a dotfile, like Ghostty.

- **Fast.** Native AppKit. A custom list view draws only the visible rows. Directory reads run on a background thread.
- **Keyboard first.** It has vim keys and Finder keys by default. You can bind any key or key sequence to any action.
- **Configured with a dotfile.** One plain text file, `~/.config/better-finder/config`. The app reloads it without a restart.
- **Native Quick Look.** Press `Space` to preview a file, like in Finder.

## Build and run

```sh
cargo run --release -p bf-app -- ~/Downloads
```

To make an app bundle in `dist/`:

```sh
scripts/bundle.sh
open "dist/Better Finder.app"
```

## Configuration

The config file is `~/.config/better-finder/config`. If `XDG_CONFIG_HOME` is set, the file is `$XDG_CONFIG_HOME/better-finder/config`. Press `cmd+,` to open the file. The app creates it from a template when it does not exist. Press `cmd+shift+,` to reload it.

```ini
# Look
font-family = "JetBrains Mono"
font-size = 14
theme = auto
background = #1e1e2e
foreground = #cdd6f4
selection-background = #45475a
background-opacity = 0.95

# Behavior
show-hidden = false
sort-by = name
directories-first = true
columns = size,modified
date-format = relative
terminal = Ghostty

# Keys
keybind = g>p=goto:~/Apps/personal-projects
keybind = d=unbind
keybind = ctrl+n=cursor_down
keybind = ctrl+p=cursor_up
```

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
| `ctrl+d` `ctrl+u` | Half page down and up |
| `Space` | Quick Look |
| `/` | Filter |
| `.`, `cmd+shift+.` | Show or hide hidden files |
| `gh` `gd` `ge` `go` `ga` `gc` `gr` | Go to home, Downloads, Desktop, Documents, Applications, `~/.config`, `/` |
| `cmd+shift+g` | Go to a folder (prompt) |
| `sn` `ss` `sm` `se` `sr` | Sort by name, size, date, extension; reverse the order |
| `r` | Rename |
| `cmd+shift+n` | New folder |
| `cmd+Backspace` | Move to Trash |
| `cc` `cn` | Copy the path or the name |
| `T` | Open a terminal here |
| `?` | Show all key bindings |
| `cmd+,` `cmd+shift+,` | Open and reload the config |

## Architecture

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
