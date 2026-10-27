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
| One frame of a smooth scroll | < 2 ms |
| Scroll a list of 100 000 entries | No dropped frames |
| Disk IO on the main thread | Only for one-file operations (rename, new folder, trash) |

**Open work:** The Finder-style UI raised the launch time to about 165 ms (measured). The target is 150 ms. The team must reduce it.

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
| `color` | `Rgba` colors and the parser for color values in themes. |
| `fs` | `read_dir`, `Entry`, hidden and package rules, path helpers. |
| `sort` | Natural sort, sort keys. |
| `filter` | Live filter (substring or fuzzy, smart case). The test that tells a path from a filter in the location field. |
| `format` | Size and date text. |
| `pane` | Navigation state: directory, entries, cursor, history, cursor memory, filter. |
| `viewport` | Scroll arithmetic with the vim rules: half page, page, top, bottom, and the 2-row margin of line moves. Pure functions. |
| `names` | New names for copies, aliases, and archives, like Finder: `report copy.pdf`, `file 2.txt`, `report.pdf alias`, `report.pdf.zip`. Pure functions. |
| `transfer` | The rules of a copy, move, or alias into a folder: the name style, the "inside itself" test, and the status text. Pure functions. |
| `drop` | The Finder rules of a drop: the transfer mode from the offered operations and the volumes, and the checks that stop a drop (a move into the same folder, a folder into itself). Pure functions. |
| `viewer` | The open requests of other apps: from the paths to one folder and the name to put the cursor on. Pure functions, except `Kind::of`. |

The public API of `bf-core` is the contract with `bf-app`. The doc comments in the source are the specification.

### bf-app modules

| Module | Job |
| --- | --- |
| `main` | CLI arguments. The `+` commands. Starts `NSApplication`. |
| `app` | The app delegate, the main menu, the app state, the dispatch of actions. |
| `window` | The `NSWindow`, the split view (sidebar and content), the column header, the list, the prompt, the path and status bar, and the help overlay. |
| `list_view` | A custom `NSView` in an `NSScrollView`. It draws only the dirty rows. It handles keys and the mouse, and gives the context menu (`menuForEvent:`). It is a drag source (a file URL for each dragged row) and a drop target (`bf_core::drop`, then `file_ops::transfer`). It is the controller of the Quick Look panel. During a smooth scroll, a small subview shows the moving cursor highlight. |
| `columns` | The text of the list columns: size, Kind, and dates. It follows the locale, like Finder. |
| `header` | The column header above the list. A click on a title sorts the list. |
| `sidebar` | The sidebar (`NSOutlineView`, source list style) with Favorites and Locations. A background thread builds the model. A volume row has an eject button. |
| `volumes` | The mounted volumes (read on a background thread) and the mount, unmount, and rename notifications. The eject runs on a background thread: `NSWorkspace` ejects the volume, `lsof` finds the apps that keep it busy, and `diskutil` forces the eject. |
| `toolbar` | The unified toolbar: back, forward, and the location and filter field. The window title shows the path. |
| `status_bar` | One line at the bottom. It draws the path bar (crumbs), and the counts, free space, filter, pending keys, and messages. |
| `prompt` | The text inputs: the bottom line (the location and filter field when the toolbar is off), the inline rename field, and the toolbar field. One delegate serves all of them. |
| `goto_panel` | The Go to Folder panel with up to 8 matching folders. `Tab` completes the path. The same view without a field is the match list of the location field. |
| `help` | The key binding help overlay. It takes the keyboard focus. The movement keys of the keymap scroll it. `cancel`, `help`, and `q` close it. |
| `smooth` | The smooth scroll driver for the list and the help overlay. A display link moves the clip view and the cursor highlight with an ease-out curve. |
| `file_ops` | File system helpers with data-safety rules, for example a rename that never replaces an item. `transfer` copies, moves, or makes aliases of items in a folder on a background thread, and `compress` runs `ditto`. They never replace an item. |
| `context_menu` | The context menu of the list (a right click or a control click), the Open With and Share parts of the actions, and the Services of the list. |
| `file_viewer` | The open requests of other apps (`application:openURLs:`): the paths of the URLs, the trace of the Apple Event, and the test for the `NSFileViewer` default. |
| `quicklook` | `QLPreviewPanel` data source and delegate. |
| `editor` | Opens a text file in the `editor` command, in a new window of Ghostty or Terminal. It runs `osascript`. The effect is `OpenFile`; a background thread checks for text. |
| `icons` | File icons from `NSWorkspace`, with a cache. Each icon renders one time into a bitmap at the draw size. |
| `theme` | `Rgba` to `NSColor`, fonts, row metrics. |
| `loader` | Runs `bf_core::fs::read_dir` on a background thread. Sends the result to the main queue. |
| `watcher` | Watches the current directory and the config files. Debounces the events. |
| `debug` | Developer tools: timing traces, `--debug-keys`, and `--debug-screenshot`. |

## Main flows

### Key press

1. `ListView::keyDown:` (and `performKeyEquivalent:` for keys with `cmd`) makes a `KeyEvent`:
   - `base`: `charactersByApplyingModifiers:` with no modifiers, or a `NamedKey` from the key code.
   - `shifted`: `charactersIgnoringModifiers` when shift is down.
   - `mods`: from `modifierFlags`.
2. `KeymapState::feed` gives `Action`, `Pending`, or `Unbound`.
3. `app::dispatch(action)` changes the `Pane` or does an operation, then asks for a redraw.

When the prompt is open, the text field gets the keys. `Enter` accepts and `Escape` cancels. In the filter prompt, `Up` and `Down` move the list cursor. A path in the filter field (`filter::is_path_query`) shows the matching folders: `Up` and `Down` select a match, `Tab` completes, and `Enter` goes.

When the help overlay is open, it has the keyboard focus. `app::help_key` sends the key through the keymap: the movement actions scroll the overlay, and the other actions do nothing (except `cancel`, `help`, and `quit`).

### Smooth scroll

1. A page action (`half_page_down`, `page_down`, `cursor_top`, and the others) gets the target from `viewport::Viewport::apply`. The pane cursor changes at once.
2. `sync_ui` gives the target to `smooth::scroll_list`. A display link moves the clip view and the highlight subview to the target in `smooth-scroll-duration` milliseconds.
3. A new page action during the animation starts from the current position (a retarget). Any other key, a mouse scroll, a resize, or a change of the list ends the animation at its target.

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

### File viewer requests

Better Finder can replace Finder as the file viewer of the Mac (README, "Use Better Finder as the file viewer"). Then other apps send it their requests:

- "Show in Finder" and `open -R <file>`: `NSWorkspace` (`activateFileViewerSelectingURLs:` and `selectFile:inFileViewerRootedAtPath:`) sends the request to the app in the `NSFileViewer` default.
- `open -a "Better Finder" <folder>`, and a folder in a default app of `public.folder`: Launch Services sends the request. `scripts/bundle.sh` declares the type in `CFBundleDocumentTypes` (role `Viewer`, rank `Alternate`). macOS 26 refuses a change of the default app of `public.folder`: `LSSetDefaultRoleHandlerForContentType` and `NSWorkspace setDefaultApplicationAtURL:toOpenContentType:` return -50 (`paramErr`), also for Finder. So `open <folder>` still goes to Finder.

On macOS 26, all of these requests arrive as the Apple Event `aevt`/`odoc`, not as the Finder event `misc`/`mvis`. `BETTER_FINDER_TRACE=1` prints the event. The flow:

1. AppKit calls `application:openURLs:` of the app delegate. When the request starts the app, the call comes before `applicationDidFinishLaunching:`.
2. `app::show_items` reads the kind of each path (`viewer::Kind::of`) on a background thread.
3. `viewer::target` gives the folder and the name: a folder opens, and a file opens its folder with the cursor on the file. The first usable path decides, because the app has one window.
4. The prompt, the help overlay, and the filter close. The folder loads, the cursor goes to the name (`pending_select`), and the window comes to the front.

Limits:

- The event is the same for "Show in Finder" and for an open request. So "Show in Finder" of a folder opens the folder. Finder selects the folder in its parent folder.
- When the request starts the app, the start folder shows for a short time before the requested folder. The first frame comes before AppKit delivers the event, and the launch time is more important.
- `open <folder>` goes to Finder (see above).
- Apps that script Finder with AppleScript still go to Finder.

`reveal_in_finder` uses `activateFileViewerSelectingURLs:`. When `NSFileViewer` names Better Finder, that call comes back to this app. So then the action runs a Finder script (`reveal`) through `osascript`, like `get_info`.

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

- Multi-select and marks, and cut and paste (a move) of files. Copy and paste of one item works.
- Drag and drop to and from other apps.
- Tabs and split panes.
- A preview column (Miller columns), like Finder column view.
- Built-in theme files (`theme = <name>`), like Ghostty.
- `getattrlistbulk` for directory reads.
- Accessibility (`NSAccessibility` for the list view).
- Fuzzy jump to a directory (zoxide style).
- Key bindings for non-Latin keyboard layouts (translate the key code with `UCKeyTranslate` and an ASCII-capable layout).
