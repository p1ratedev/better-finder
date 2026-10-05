# Better Finder: architecture

This document is the design contract for the code. Read it before you change the code.

## Goals

1. **Fast.** The app starts at once. A key press gives a new frame at once. The main thread never waits for the disk.
2. **Keyboard first.** Every function has a key binding. The mouse works, but you do not need it.
3. **Config in a dotfile, like Ghostty.** You use one plain text file to change the look, the behavior, and all key bindings. The app reloads the file without a restart.
4. **Native macOS.** The app uses the real Quick Look panel, the real file icons, the real Trash, and the default app of each file.

## Performance targets

Measure with a release build (`cargo build --release`).

| Measure | Target |
| --- | --- |
| Launch to first frame with entries | < 150 ms |
| Read and sort a directory with 10 000 entries | < 50 ms |
| Key press to new frame | < 8 ms |
| Scroll a list of 100 000 entries | No dropped frames |
| Disk IO on the main thread | Only for one-file operations (rename, new folder, trash) |

## Crates

```
crates/
  bf-core/   Pure Rust. No UI code, no macOS framework code. Unit tests cover it.
  bf-app/    The macOS app. AppKit through objc2. Binary name: better-finder.
```

This is the Ghostty model: a core library and a thin native shell. The shell converts AppKit events to core calls. The shell draws the core state.

### bf-core modules

| Module | Job |
| --- | --- |
| `action` | The `Action` enum. Text form `name` or `name:param`. |
| `keymap` | Trigger syntax, the binding trie, key sequences (`g>d`), the default bindings. |
| `config` | The parser for the config file, the theme, the documented config template. |
| `fs` | `read_dir`, `Entry`, hidden and package rules, path helpers. |
| `sort` | Natural sort, sort keys. |
| `filter` | Live filter (substring or fuzzy, smart case). |
| `format` | Size and date text. |
| `pane` | Navigation state: directory, entries, cursor, history, cursor memory, filter. |

The public API of `bf-core` is the contract with `bf-app`. The doc comments in the source are the specification.

### bf-app modules

| Module | Job |
| --- | --- |
| `main` | CLI arguments. The `+` commands. Starts `NSApplication`. |
| `app` | The app delegate, the main menu, the app state, the dispatch of actions. |
| `window` | The `NSWindow`, the layout of the list, the status bar, and the prompt. |
| `list_view` | A custom `NSView` in an `NSScrollView`. It draws only the visible rows. It handles keys and the mouse. It is the controller of the Quick Look panel. |
| `status_bar` | One line at the bottom: counts, sort, hidden flag, filter, pending keys, messages. |
| `prompt` | One `NSTextField` line for filter, go to folder, rename, and new folder. |
| `quicklook` | `QLPreviewPanel` data source and delegate. |
| `icons` | File icons from `NSWorkspace`, with a cache. |
| `theme` | `Rgba` to `NSColor`, fonts, row metrics. |
| `loader` | Runs `bf_core::fs::read_dir` on a background thread. Sends the result to the main queue. |
| `watcher` | Watches the current directory and the config files. Debounces the events. |

## Main flows

### Key press

1. `ListView::keyDown:` (and `performKeyEquivalent:` for keys with `cmd`) makes a `KeyEvent`:
   - `base`: `charactersByApplyingModifiers:` with no modifiers, or a `NamedKey` from the key code.
   - `shifted`: `charactersIgnoringModifiers` when shift is down.
   - `mods`: from `modifierFlags`.
2. `KeymapState::feed` gives `Action`, `Pending`, or `Unbound`.
3. `app::dispatch(action)` changes the `Pane` or does an operation, then asks for a redraw.

When the prompt is open, the text field gets the keys. `Enter` accepts and `Escape` cancels. In the filter prompt, `Up` and `Down` move the list cursor.

### Directory load

1. A `Pane` navigation method returns a `LoadRequest`.
2. `loader` runs `read_dir` on a background thread.
3. The result goes back to the main queue with `dispatch2`. `Pane::finish_load` applies it, or ignores it when the generation is old.
4. The status bar shows `Loading…` only when a load takes more than 100 ms.

### Quick Look

`ListView` is in the responder chain and accepts control of `QLPreviewPanel`:

- `acceptsPreviewPanelControl:` returns YES.
- `beginPreviewPanelControl:` sets the data source and the delegate.
- `endPreviewPanelControl:` removes them.
- The data source gives one item: the file URL of the entry under the cursor.
- When the cursor moves and the panel is visible, the view calls `reloadData`.
- The delegate `previewPanel:handleEvent:` sends key events back to the keymap. So `j`, `k`, and the arrows move the cursor and change the preview, like Finder.
- `Space` and `Escape` close the panel.

### Config reload

`reload_config` and a change to a watched config file run `Config::load()` again. The app then applies the theme, fonts, keymap, sort, hidden flag, and columns without a restart. The status bar shows the first diagnostic and the number of diagnostics.

## Decisions

### AppKit through objc2, not a cross-platform GUI library

- The real Quick Look panel needs an `NSResponder` that we control. GPUI, egui, iced, and Slint own their views, so Quick Look needs hacks there.
- Native file icons, the Trash, `representedURL`, and drag and drop come from AppKit at no cost.
- An AppKit app starts in milliseconds and has a small binary.
- The app is for macOS only. Portability has no value here.

### A custom-drawn list view, not NSTableView

- Full control of the theme (all colors, the font, the row height, the selection), like Ghostty.
- A draw of only the visible rows is simple. The cost of a frame does not depend on the directory size.
- Less objc2 code: there is no table data source, no cell views, and no row views.
- Cost: no accessibility yet. It is on the roadmap.

### std::fs first, getattrlistbulk later

`read_dir` and one `lstat` for each entry are fast on APFS. Finder uses `getattrlistbulk(2)`, which gets names and attributes in batches. It is the next step when a benchmark shows that `lstat` is the bottleneck.

## Roadmap

- Multi-select and marks, copy, cut, and paste of files.
- Drag and drop to and from other apps.
- Tabs and split panes.
- A preview column (Miller columns), like Finder column view.
- Built-in theme files (`theme = <name>`), like Ghostty.
- `getattrlistbulk` for directory reads.
- Accessibility (`NSAccessibility` for the list view).
- Fuzzy jump to a directory (zoxide style).
- Key bindings for non-Latin keyboard layouts (translate the key code with `UCKeyTranslate` and an ASCII-capable layout).
