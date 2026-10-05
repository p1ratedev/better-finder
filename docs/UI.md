# Better Finder: UI design (Finder parity)

This document is the design contract for the window UI. It extends `ARCHITECTURE.md`.

## Goal

By default, Better Finder looks like Finder on macOS 26. The config file can change every part, like Ghostty. Every UI element also has an action and a key. The mouse works, but you do not need it.

Speed rules stay the same: no disk IO on the main thread, and the frame cost does not depend on the folder size.

## Window layout

```
+-----------------------------------------------------------------------------+
| (o)(o)(o)  < >   ~/Downloads                                 [ Filter     ] |  toolbar (unified)
+----------------+------------------------------------------------------------+
|  Favorites     |  Name                         Size   Kind          Date Mod |  column header
|  > Applications|  [ic] 2026FD                    --   Folder        Today at |
|  v Downloads   |  [ic] archive.zip           532 KB   ZIP archive   28 Sep 2 |  list rows
|  > Desktop     |  ...                                                        |
|                |                                                            |
|  Locations     |                                                            |
|  > iCloud Drive|                                                            |
|  > Macintosh HD|  Macintosh HD > Users > admin > Downloads   578 items, 6 hi |  path and status bar
+----------------+------------------------------------------------------------+
   sidebar (glass)                      content
```

### AppKit structure

- `NSWindow`: titled, closable, miniaturizable, resizable, full-size content view. `toolbarStyle` is unified.
- `contentViewController` is an `NSSplitViewController`:
  - Item 0: `NSSplitViewItem` with the sidebar behavior (`sidebarWithViewController:`). On macOS 26 this gives the floating glass sidebar. Do not paint over it.
  - Item 1: the content. From top to bottom: the column header (pinned), the `NSScrollView` with `ListView`, and the path and status bar.
- The split view has an autosave name. The sidebar width persists.
- The list content scrolls under the toolbar, like Finder. Use the safe area and the automatic content insets. The column header stays pinned below the toolbar.

## Theme `system` (the new default)

`theme = system` uses the macOS semantic colors. It follows the appearance and the accent color live (`viewDidChangeEffectiveAppearance`, `NSSystemColorsDidChangeNotification`).

| Element | Color |
| --- | --- |
| List background | `controlBackgroundColor` |
| Names (files, folders, symlinks, executables) | `labelColor`. Finder does not color names. |
| Names of hidden entries | `labelColor` at 50% alpha |
| Size, kind, and date columns | `secondaryLabelColor` |
| Cursor row, list is first responder and window is key | `selectedContentBackgroundColor`, text `alternateSelectedControlTextColor` |
| Cursor row, other states | `unemphasizedSelectedContentBackgroundColor`, text `labelColor` |
| Column header text | `secondaryLabelColor`. The sort column uses `labelColor`. |
| Separators | `separatorColor` |
| Path and status bar text | `secondaryLabelColor` |
| Accent (filter text, pending keys) | `controlAccentColor` |

A color key in the config file (for example `background = #1e1e2e`) replaces that color in all themes, `system` too. `theme = auto | light | dark` keep the custom palettes (the Ghostty look).

## List

| Metric | Value |
| --- | --- |
| Font | System font, 13 pt (the `font-family` and `font-size` keys apply) |
| Row height | 20 pt for 13 pt text (`round(font_size * 1.55)`); `row-height` overrides it |
| Leading padding | 16 pt, then the icon (16 x 16), a 6 pt gap, then the name |
| Cursor row | Rounded rectangle, radius 5 pt, inset 8 pt from the left and right edges |
| Name column | All the width that the other columns do not use. Truncation in the middle. |
| Size column | 80 pt, right-aligned |
| Kind column | 130 pt, left-aligned, truncation in the middle |
| Date columns | 170 pt, left-aligned |
| Gap between columns | 12 pt |

Column text:

- **Size**: `NSByteCountFormatter`, file count style, so the text follows the locale (`2,5 MB` in Brazil). Folders and packages show `--`.
- **Kind**: `UTType` localized description, cached by extension (`PDF Document`, `ZIP archive`). A folder is `Folder`. A symlink is `Alias`. A file with the execute bit and no extension is the description of `public.unix-executable`. Unknown types are `Document`.
- **Dates**: `date-format = finder` (the new default) uses one cached `NSDateFormatter`: medium date style, short time style, relative formatting. This gives `Today at 00:32` and `28 Sep 2026 at 13:02` in the system locale. Other values of `date-format` use `bf_core::format::format_time`.

## Column header

- 24 pt high, pinned. System font 11 pt. A bottom separator line, and thin vertical separators between the columns.
- Titles: `Name`, `Size`, `Kind`, `Date Modified`, `Date Created`, `Extension`.
- The sort column shows a chevron (SF Symbol `chevron.up` or `chevron.down`) after its title.
- A click on the sort column reverses the order. A click on another column sorts by it in the normal direction. `Kind` sorts by extension.
- Config: `column-header = true`.

## Sidebar

- `NSOutlineView` with the source list style. Two group rows: `Favorites` and `Locations`.
- **Favorites** come from the config: `sidebar-item = <path>`. The key can occur many times. `sidebar-item = clear` removes the defaults. Defaults: `/Applications`, `~/Desktop`, `~/Documents`, `~/Downloads`, `~`. A path that does not exist does not show.
- The label is the display name from `NSFileManager displayNameAtPath:` (localized, so `~` shows the user name).
- **Locations** (`sidebar-locations = true`): iCloud Drive (when `~/Library/Mobile Documents/com~apple~CloudDocs` exists), the mounted volumes that are not hidden (the boot volume first), and the Trash (`~/.Trash`). The list updates when a volume mounts or unmounts (`NSWorkspace` notifications). Get the volume list on a background thread.
- **Icons**: SF Symbols, template style, like Finder. `house` (home), `menubar.dock.rectangle` (Desktop), `doc` (Documents), `arrow.down.circle` (Downloads), `app.dashed` or the nearest Finder match (Applications), `film`, `music.note`, `photo`, `icloud`, `internaldrive`, `externaldrive`, `trash`, `folder` (other folders).
- The row of the current folder is selected. If no row matches, nothing is selected.
- A click on a row goes to that folder.
- Keys:
  - `toggle_sidebar` (default `cmd+ctrl+s`, like Finder) shows or hides the sidebar.
  - `focus_sidebar` (default `tab`) moves the focus to the sidebar, or back to the list.
  - With the focus in the sidebar: `j`, `k`, `up`, and `down` move the selection. `enter`, `l`, and `right` open the folder and give the focus back to the list. `escape`, `tab`, and `h` give the focus back to the list. Other keys go to the normal keymap.
- Config: `sidebar = true` (visible at start), `sidebar-width = 180`.

## Toolbar

- `NSToolbar`, unified style, no user customization, icons only.
- Items, from left to right:
  1. Back and forward (SF Symbols `chevron.left` and `chevron.right`). They are enabled only when history exists.
  2. The sidebar tracking separator, so the items line up with the content.
  3. The title: `display_path(cwd)`.
  4. Flexible space.
  5. `NSSearchToolbarItem` with the placeholder `Filter`. Text changes filter the list live. `Enter` keeps the filter and focuses the list. `Escape` clears the filter and focuses the list.
- The `filter` action (keys `/` and `cmd+f`) focuses the search field. When `toolbar = false`, the filter uses the bottom prompt as before.
- Config: `toolbar = true`.

## Path and status bar

- One bar at the bottom of the content, 24 pt, with a top separator line.
- **Left side:** the path from the volume root as breadcrumbs, for example `Macintosh HD > Users > admin > Downloads`. Each crumb has a small icon (14 pt). When there is no space, the middle crumbs truncate first. A click on a crumb goes to that folder.
- **Right side:** `578 items, 6 hidden`, then `123 GB available`. Get the free space with `statfs` on the loader thread. Then the pending keys (accent) and the messages: info messages fade after 3 s, and errors stay until the next action.
- Config: `path-bar = true`. When it is false, only the status text shows.

## Rename and new folder in the list

- `rename` opens an `NSTextField` over the name of the cursor row. It uses the same font and selects only the stem. `Enter` commits with the current safety rules (`file_ops::rename_no_replace`, the source path stored at open). `Escape`, a loss of focus, or a scroll cancels.
- `new_folder` creates `untitled folder` at once (`untitled folder 2` and so on when the name exists), selects it, and opens the inline rename on it, like Finder.

## Go to Folder (SHOULD)

The `goto_prompt` action shows a floating panel at the top center of the content, like the Finder Go to Folder panel: a rounded material background, a 13 pt field, and up to 8 matching folders below the field. `Tab` completes, `up` and `down` select a match, and `Enter` goes. The bottom prompt is the fallback.

## Default keys added

| Keys | Action | Note |
| --- | --- | --- |
| `cmd+ctrl+s` | `toggle_sidebar` | Finder |
| `tab` | `focus_sidebar` | |
| `cmd+f` | `filter` | Finder search |
| `cmd+shift+h` | `goto:~` | Finder |
| `cmd+shift+d` | `goto:~/Desktop` | Finder |
| `cmd+shift+o` | `goto:~/Documents` | Finder |
| `cmd+alt+l` | `goto:~/Downloads` | Finder |
| `cmd+shift+a` | `goto:/Applications` | Finder |
| `cmd+shift+u` | `goto:/Applications/Utilities` | Finder |
| `cmd+shift+i` | `goto:~/Library/Mobile Documents/com~apple~CloudDocs` | Finder |

## bf-core changes for this design

- `ThemeMode::System`, the new default. `resolved_theme` gives the dark or light palette for `System` as a fallback. The app uses the semantic colors.
- `Column::Kind` and `Column::Created`. `Entry::created: Option<SystemTime>` from the `lstat` birth time (no extra syscall). The default `columns` is `size,kind,modified`.
- `date-format = finder` is valid and is the default. `format_time` gives `Today at 14:32`, `Yesterday at 09:10`, and `%-d %b %Y at %H:%M` for it, as a fallback for tests and the CLI.
- `effective_row_height()` is `round(font_size * 1.55)`.
- Config keys: `sidebar`, `sidebar-width` (120 to 400), `sidebar-item` (repeatable, `clear`), `sidebar-locations`, `toolbar`, `column-header`, `path-bar`.
- Actions: `toggle_sidebar`, `focus_sidebar`. The default keys in the table above.
- The config template documents all the new keys.

## Later (not in this round)

- Groups by date (`Today`, `Yesterday`, `Previous 7 Days`), like the Finder `Use Groups` option.
- A `Date Added` column. It needs `getattrlistbulk` (`ATTR_CMNEXT_ADDEDTIME`) in `bf_core::fs::read_dir`.
- Image thumbnails as icons (QuickLookThumbnailing, on a background queue, with a cache).
- Tags in the sidebar.
