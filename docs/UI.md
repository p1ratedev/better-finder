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

- `NSOutlineView` with the source list style. Three group rows: `Favorites`, `Locations`, and `Remote`. A group without rows does not show.
- **Favorites** come from the config: `sidebar-item = <path>`. The key can occur many times. `sidebar-item = clear` removes the defaults. Defaults: `/Applications`, `~/Desktop`, `~/Documents`, `~/Downloads`, `~`. A path that does not exist does not show.
- The label is the display name from `NSFileManager displayNameAtPath:` (localized, so `~` shows the user name).
- **Locations** (`sidebar-locations = true`): iCloud Drive (when `~/Library/Mobile Documents/com~apple~CloudDocs` exists), the mounted volumes that are not hidden (the boot volume first), and the Trash (`~/.Trash`). The list updates when a volume mounts or unmounts (`NSWorkspace` notifications). Get the volume list on a background thread.
- **Remote** comes from the config: `remote = [user@]host[:path]`. The key can occur many times. `remote = clear` removes all remotes. No `remote` key gives no Remote group. The label is the config value (`admin@mac-mini:/Users/admin`), with the SF Symbol `network`. The rows need no disk access and no network. See "Remote folders".
- **Icons**: SF Symbols, template style, like Finder. `house` (home), `menubar.dock.rectangle` (Desktop), `doc` (Documents), `arrow.down.circle` (Downloads), `app.dashed` or the nearest Finder match (Applications), `film`, `music.note`, `photo`, `icloud`, `internaldrive`, `externaldrive`, `trash`, `folder` (other folders).
- The row of the current folder is selected. If no row matches, nothing is selected. In a remote folder, the row of the same host is selected: the row with the longest absolute start path that contains the folder, else the first row of the host.
- A click on a row goes to that folder. A remote row opens its start folder (the action `remote:<value>`).
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
- **Left side:** the path from the volume root as breadcrumbs, for example `Macintosh HD > Users > admin > Downloads`. In a remote folder, the first crumb is the host: `admin@mac-mini > Users > admin`. Each crumb has a small icon (14 pt). When there is no space, the middle crumbs truncate first. A crumb never goes under the message. A click on a crumb goes to that folder, on the same host.
- **Right side:** `578 items, 6 hidden`, then `123 GB available`. Get the free space with `statfs` on the loader thread. Then the pending keys (accent) and the messages: info messages fade after 3 s, and errors stay until the next action.
- Config: `path-bar = true`. When it is false, only the status text shows.

## Rename and new folder in the list

- `rename` opens an `NSTextField` over the name of the cursor row. It uses the same font and selects only the stem. `Enter` commits with the current safety rules (`file_ops::rename_no_replace`, the source path stored at open). `Escape`, a loss of focus, or a scroll cancels.
- `new_folder` creates `untitled folder` at once (`untitled folder 2` and so on when the name exists), selects it, and opens the inline rename on it, like Finder.

## Go to Folder (SHOULD)

The `goto_prompt` action shows a floating panel at the top center of the content, like the Finder Go to Folder panel: a rounded material background, a 13 pt field, and up to 8 matching folders below the field. `Tab` completes, `up` and `down` select a match, and `Enter` goes. The bottom prompt is the fallback.

## Location and filter field

The `filter` action (`/`, `cmd+f`) focuses one field: the toolbar search field, or the bottom prompt when `toolbar = false`. The placeholder is `Filter or go to a path`. The text selects the mode:

- **Path mode:** the text starts with `~`, contains `/`, or is `.` or `..`.
  - Up to 8 matching folders show below the field, like the Go to Folder panel.
  - `Tab` completes the longest common prefix. `up` and `down` select a match.
  - `Enter` on a folder goes to it. `Enter` on a file goes to its parent folder and selects the file. A missing path shows `No such folder`, and the field stays open.
  - After a navigation, the field clears and the list gets the focus.
  - The path check runs on a background thread, as in the Go to Folder panel.
- **Filter mode:** all other text filters the current folder live, as before.

## Help overlay keys

The help overlay (`help`, `?`) takes the keyboard focus while it is open.

- Key events go through the user keymap. In the overlay, the movement actions scroll the overlay: `cursor_down` and `cursor_up` (one line), `half_page_down` and `half_page_up`, `page_down` and `page_up`, `cursor_top` and `cursor_bottom`. Scrolling uses the smooth scroll rules below.
- `cancel`, `help`, and the key `q` close the overlay. `quit` still quits. The overlay ignores all other actions.
- When the overlay closes, the keyboard focus goes back to the view that had it before: the list or the sidebar. The app reads the focus before it hides the overlay, because AppKit gives the focus of a hidden view to the window.
- The mouse wheel also scrolls the overlay.
- SHOULD: `/` in the overlay focuses a small filter field at the top. The filter matches the keys, the action names, and the descriptions.

## Smooth scroll (vim rules)

The page actions follow vim. The view and the cursor move together, so the cursor keeps its row on the screen.

- `half_page_down`: `n = max(1, visible_rows / 2)`. The view scrolls down by `min(n, rows left below the view)`. The cursor moves down by `n`, clamped to the last entry. `half_page_up` is the same in the other direction.
- `page_down` and `page_up`: the same rule with `n = max(1, visible_rows - 2)`.
- `cursor_top` and `cursor_bottom`: the view and the cursor go to the start or the end, with the same animation.
- `cursor_down` and `cursor_up` (`j`, `k`, arrows) do not animate. When the cursor reaches the 2-row margin, the view scrolls by one row at once. Key repeat stays crisp.
- New default keys: `ctrl+f` (`page_down`) and `ctrl+b` (`page_up`), like vim.

Animation:

- One animation driver (a display link, or a 120 Hz timer as a fallback) moves the scroll position of the clip view and the y position of the cursor highlight from their current values to the targets. The curve is ease-out (cubic).
- The pane cursor (the logical cursor) changes at once. So `open`, Quick Look, `trash`, and all other actions use the target entry.
- A new scroll action during an animation starts from the current animated position (a retarget). There is no queue and no jump back.
- Any other key, a mouse scroll, a window resize, a directory change, or a filter change ends the animation at its target at once.
- During the animation, the list draws the highlight at the animated position. It draws the row text in the normal colors, then clips to the highlight and draws the text again in the selection colors. So the text under the moving highlight stays readable.
- One animation frame costs less than 2 ms. The view redraws only the changed areas.
- When the macOS setting Reduce Motion is on (`accessibilityDisplayShouldReduceMotion`), nothing animates.
- Config: `smooth-scroll = true`, `smooth-scroll-duration = 150` (milliseconds, 0 to 1000; 0 turns the animation off).

The scroll arithmetic is a pure function in bf-core (for example a `viewport` module), with unit tests for the edges: the first and last rows, short lists, and a list shorter than the view.

## Drag and drop

### Drag out of the list

- A drag of a row starts when the mouse moves 4 pt or more from a single click. A double click does not start a drag.
- The drag item is the file URL of the entry. The drag pasteboard gets `public.file-url`, `NSFilenamesPboardType`, and the URL types, like a drag from Finder. So Finder, Telegram, Mail, and browsers accept the drop.
- The drag picture is the icon and the name of the row in the normal colors. Without a picture, the drag shows the file icon.
- The list offers copy, move, link, and generic. The app that gets the drop chooses one: Finder moves the file on the same volume and copies it to another volume, and the option key forces a copy.

### Drop on the list

The list takes drops of files (`public.file-url`) from Finder, from other apps, and from the list itself.

| Drop place | What the drop does | Ring |
| --- | --- | --- |
| A folder row | Puts the files in that folder | Around the row |
| An app row (`.app`) | Opens the files with that app | Around the row |
| A file row, a package row, or the empty space | Puts the files in the current folder | Around the list |

The mode follows Finder (`bf_core::drop::choose`):

- No modifier key: a move on the same volume, a copy to another volume.
- Option: a copy. Control: an alias. Command: a move.
- A move to another volume copies the files and keeps the sources. The app never deletes a source after a copy.

The drop does nothing (`bf_core::drop::check`) when:

- it is a move into the folder that has the files already (for example a row on its own folder), or
- it copies or moves a folder into itself or into one of its subfolders. A drop on the row of a dragged folder then goes to the current folder.

The file work never replaces a file. A name conflict gives the Finder "Keep Both" name (`file 2.txt`). A copy into the same folder gives the duplicate name (`file copy.txt`). The work runs on a background thread, then the list reloads and the status bar shows the result.

Later: drops on the sidebar, spring-loaded folders (a folder opens during a drag), and a scroll of the list at its edges during a drag.

## Volumes and eject

### Locations list

- The Locations group shows the boot volume, external disks, disk images, and network shares (the mounted volumes that are not hidden). iCloud Drive and the Trash stay at their places.
- Each volume except the boot volume (`NSURLVolumeIsRootFileSystemKey`) has an eject button (SF Symbol `eject`, secondary color) at the trailing edge of its row, like Finder. A click on the button ejects that volume.
- The list updates on the `NSWorkspace` mount, unmount, and rename notifications. A new volume shows within 1 s after macOS mounts it. Read the volume list on a background thread.
- When a volume unmounts for any reason (our eject, an eject in Finder, a disconnected cable):
  - If the current folder is on that volume, the app goes to the home folder and shows `"X" was ejected`.
  - Quick Look closes when it shows a file on that volume. A rename or new-folder prompt on that volume cancels.
  - SHOULD: the back and forward history forgets the folders on that volume.

### The eject action

- Action `eject`. Default key `cmd+e`, like Finder. It never runs on key repeat.
- The target, in this order:
  1. When the sidebar has the focus: the selected Locations row.
  2. The entry under the cursor, when it is the mount point of a volume that is not the boot volume (for example in `/Volumes`).
  3. The volume of the current folder, when it is not the boot volume.
  4. Else the message `Nothing to eject here`.
- Steps:
  1. When the current folder is on the target volume, go to the home folder first. Stop the directory watcher stream on the volume, and close Quick Look when it shows a file on the volume. So Better Finder itself does not keep the volume busy.
  2. Show `Ejecting "X"...` until the result comes.
  3. On a background thread, call `NSWorkspace unmountAndEjectDeviceAtURL:error:`. It unmounts all volumes of the same device, like Finder. It can take seconds, so it never runs on the main thread.
  4. Success: show `Ejected "X"`. The unmount notification updates the sidebar.
  5. The volume is busy (for example `EBUSY`): on a background thread, find the processes with open files on the volume (`lsof -F pc +f -- <mount point>`, 3 s timeout). Show `"X" is in use by <app names>. Press <eject key> again within 5 s to force the eject. Unsaved data in those apps can be lost.` The key text comes from the user keymap.
  6. A second `eject` of the same volume within 5 s forces the eject (`diskutil unmount force <mount point>`, then `diskutil eject <mount point>` for a disk), on a background thread.
  7. Other failures: `Cannot eject "X": <reason>`.

## Default keys added

| Keys | Action | Note |
| --- | --- | --- |
| `cmd+ctrl+s` | `toggle_sidebar` | Finder |
| `tab` | `focus_sidebar` | |
| `cmd+f` | `filter` | Finder search |
| `cmd+e` | `eject` | Finder |
| `cmd+shift+h` | `goto:~` | Finder |
| `cmd+shift+d` | `goto:~/Desktop` | Finder |
| `cmd+shift+o` | `goto:~/Documents` | Finder |
| `cmd+alt+l` | `goto:~/Downloads` | Finder |
| `cmd+shift+a` | `goto:/Applications` | Finder |
| `cmd+shift+u` | `goto:/Applications/Utilities` | Finder |
| `cmd+shift+i` | `goto:~/Library/Mobile Documents/com~apple~CloudDocs` | Finder |
| `cmd+c` | `copy` | Finder. Only when the list has the focus |
| `cmd+v` | `paste` | Finder. Only when the list has the focus |
| `cmd+d` | `duplicate` | Finder |
| `cmd+ctrl+a` | `make_alias` | Finder |
| `cmd+i` | `get_info` | Finder |

## bf-core changes for this design

- `ThemeMode::System`, the new default. `resolved_theme` gives the dark or light palette for `System` as a fallback. The app uses the semantic colors.
- `Column::Kind` and `Column::Created`. `Entry::created: Option<SystemTime>` from the `lstat` birth time (no extra syscall). The default `columns` is `size,kind,modified`.
- `date-format = finder` is valid and is the default. `format_time` gives `Today at 14:32`, `Yesterday at 09:10`, and `%-d %b %Y at %H:%M` for it, as a fallback for tests and the CLI.
- `effective_row_height()` is `round(font_size * 1.55)`.
- Config keys: `sidebar`, `sidebar-width` (120 to 400), `sidebar-item` (repeatable, `clear`), `sidebar-locations`, `toolbar`, `column-header`, `path-bar`.
- Actions: `toggle_sidebar`, `focus_sidebar`. The default keys in the table above.
- The config template documents all the new keys.

## Context menu

A right click or a control click on the list shows a context menu with the Finder items and order of macOS 15 (module `context_menu`).

- A click on a row moves the cursor to the row, like a single click, and the list gets the keyboard focus. Then the menu of that entry shows.
- A click below the last row shows the menu of the current folder. The cursor does not move.
- The app has one cursor and no multi-selection. The items of the row menu act on the entry under the cursor.
- An automatic refresh can change the rows while the menu is open. An item acts only when its entry or its folder is still in the list. If not, the status bar shows an error.
- A name in an item title is in curly quotes and is truncated in the middle to 40 characters.
- The menu sets `enabled` itself (`autoenablesItems` is off). One target object gets all clicks. The action runs after the menu closes.

### Row menu

| Item | Action | Note |
| --- | --- | --- |
| Open | `open` | |
| Open With > | | Files and packages, not plain folders. See below. |
| Show Package Contents | `goto:<path>` | Packages only |
| --- | | |
| Move to Trash | `trash` | |
| --- | | |
| Get Info | `get_info` | |
| Rename | `rename` | |
| Compress “name” | `compress` | |
| Duplicate | `duplicate` | |
| Make Alias | `make_alias` | |
| Quick Look “name” | `quick_look` | |
| --- | | |
| Copy “name” | `copy` | |
| Copy as Pathname | `copy_path` | |
| Share… | `share` | |
| --- | | |
| Show in Finder | `reveal_in_finder` | |
| Services > | | AppKit adds it at the end (see "Services" below). |

Open With:

- The default app first, as `<App> (default)`, then a separator.
- The other apps from `NSWorkspace URLsForApplicationsToOpenURL:`, sorted by display name (without `.app`), each with its 16 x 16 icon, then a separator.
- `Other…` shows an `NSOpenPanel` sheet at `/Applications` that accepts only apps. The chosen app opens the file.
- The file opens with `NSWorkspace openURLs:withApplicationAtURL:configuration:completionHandler:`. An error shows in the status bar.
- The app list and the icons take 10 to 30 ms. So the submenu fills when it opens (`menuNeedsUpdate:`), like Finder, and the context menu shows at once.

### Folder menu

| Item | Action | Note |
| --- | --- | --- |
| New Folder | `new_folder` | |
| --- | | |
| Get Info | `get_info` | Of the current folder |
| --- | | |
| Paste Item, Paste N Items | `paste` | Disabled when the general pasteboard has no file URLs |
| --- | | |
| Sort By > | `sort:<key>` | Name, Size, Date Modified, Date Created, Extension, a separator, and Reverse Order (`toggle_sort_reverse`). A checkmark shows the current key and the reverse order. |
| Show Hidden Files | `toggle_hidden` | A checkmark when hidden files show |
| --- | | |
| Copy as Pathname | `copy_path` | Of the current folder |
| Open in Terminal | `open_terminal` | |
| Show in Finder | `reveal_in_finder` | Of the current folder |

The folder menu uses `app::dispatch_on(action, cwd)`: `get_info`, `copy_path`, and `reveal_in_finder` act on the current folder, not on the entry under the cursor.

### File actions

Data safety comes first. These actions never replace an item and never delete an item. The disk work runs on a background thread. Then the list reads the folder again, the cursor goes to the new item when the list shows it, and the status bar shows one message.

- `copy` (`cmd+c`): puts the entry under the cursor on the general pasteboard as a file URL (`writeObjects:` with an `NSURL`). A paste in Finder, Mail, or a chat app gets the file. Message: `Copied "report.pdf"`.
- `paste` (`cmd+v`): copies the file URLs of the general pasteboard into the current folder. It never moves them.
  - A file from the current folder gets the duplicate name: `report copy.pdf`.
  - A name that exists gets the Finder "Keep Both" name: `file 2.txt`, `file 3.txt`.
  - A folder does not go into itself or into one of its subfolders. The status bar shows an error.
- `duplicate` (`cmd+d`): a copy in the same folder.
  - `report copy.pdf`, then `report copy 2.pdf`. A duplicate of `report copy.pdf` is `report copy 2.pdf`, like Finder.
  - Folders: `Folder copy`, `Folder copy 2`.
  - A package keeps its extension at the end: `Safari copy.app`.
  - A name that starts with a dot and has no other dot has no extension: `.zshrc copy`.
- `make_alias` (`cmd+ctrl+a`): a real Finder alias file (`NSURL bookmarkDataWithOptions:` with `NSURLBookmarkCreationSuitableForBookmarkFile`, then `writeBookmarkData:toURL:options:error:`): `report.pdf alias`, then `report.pdf alias 2`.
- `compress`: `/usr/bin/ditto -c -k --sequesterRsrc` on a background thread. A folder also gets `--keepParent`, so the archive holds the folder. A file does not: else the archive also holds the name of its folder. The name is `report.pdf.zip`, then `report.pdf 2.zip`. No default key.
- `get_info` (`cmd+i`): opens the Finder Info window through `osascript` on a background thread. The path goes to the script as an argument, never in the script text. Without an entry under the cursor, it shows the info of the current folder. The first time, macOS asks the user to allow Better Finder to control Finder. When macOS refuses, the status bar says: allow it in System Settings > Privacy & Security > Automation.
- `share`: shows the macOS share menu (`NSSharingServicePicker`) next to the cursor row. No default key.
- `reveal_in_finder`: shows the item in Finder (`activateFileViewerSelectingURLs:`). When the `NSFileViewer` default names Better Finder, that call comes back to Better Finder. Then the action uses a Finder script (`reveal`), like `get_info`, with the same permission message. No default key.

How the copies work:

- A copy uses `NSFileManager copyItemAtURL:toURL:error:`. It clones the file on APFS, keeps the metadata, copies a folder with its content, and copies a symlink as a link. It fails when the target exists.
- The name comes from a loop: the first name that does not exist, with an operation that fails when the name exists. A new alias or archive first makes an empty file with `O_EXCL`, so the name is ours before the work starts.
- The name rules are pure functions in `bf_core::names`. The transfer rules are in `bf_core::transfer`.
- `file_ops::transfer(sources, dest_dir, mode)` does the work of `paste` (`Copy`), `duplicate` (`Copy`), and `make_alias` (`Alias`). The mode `Move` renames an item on the same volume only. On another volume it copies, and the source stays. A move into the folder of the source does nothing. Drag and drop can use the same function.

### Keys and text fields

The keymap gets no keys while a prompt is open, and `ListView` acts on a key equivalent only when the list is the first responder. So in the filter, rename, and go-to fields, `cmd+c` and `cmd+v` copy and paste text as before: the Edit menu items (`copy:`, `paste:`) handle them.

### Services

- The list is a Services requestor. `validRequestorForSendType:returnType:` accepts a file URL (`public.file-url`) or a string, with no return type, when an entry is under the cursor. `writeSelectionToPasteboard:types:` writes the file URL and the path.
- The app registers these send types at launch (`registerServicesMenuSendTypes:returnTypes:`).
- AppKit then adds a Services submenu at the end of the row menu by itself.
- The folder menu has no Services: the Services of the list act on the entry under the cursor, not on the folder.

### Test without a mouse

`--debug-keys "@menu:3 @menu:bg"` builds the menu of row 3 (the cursor moves to it) and the menu of the folder, and prints the items to stdout: submenus are indented, `[x]` is a checkmark, `(disabled)` is a disabled item, and `---` is a separator.

## Remote folders

A remote folder is a folder on another computer, read over SSH. The list, the keys, and the sort and filter code are the same as for a local folder.

### Config and action

- `remote = [user@]host[:path]`, repeatable (see "Sidebar"). `bf_core::remote::Remote::parse` reads the value:
  - `user@` is optional. The host is a name, an IP address, or a `Host` alias from `~/.ssh/config`. An IPv6 address goes in brackets: `admin@[fd7a::1]:/Users`.
  - The user and the host have only letters, digits, `.`, `_`, and `-` (and `:` and `%` in brackets), and do not start with `-`. So `ssh` never reads them as an option.
  - The path starts with `/` or `~`. Without a path, the folder is the home folder (`~`). The parser cleans the path (`/a/../b` gives `/b`).
  - A bad value gives a diagnostic with its line number. The other remotes stay.
- The action `remote:<value>` opens a folder on a host. The sidebar rows and the crumbs use it, and a key can bind to it: `keybind = g>m=remote:admin@mac-mini:/Users/admin`. The parser checks the value when the config loads.

### Transport

- The app runs `/usr/bin/ssh` (the system OpenSSH). It adds no SSH library and no async runtime. The user config, the keys, the agent, and `known_hosts` apply.
- `ssh -T -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=2 ... -- <destination> /bin/sh -s`. `BatchMode` makes a login that needs a prompt (a password, a passphrase, an unknown host key) fail at once. The app never waits for input.
- A shared connection: `ControlMaster=auto`, `ControlPersist=10m`, and `ControlPath=$TMPDIR/bf-ssh-<uid>/<hash>`. The first command opens the connection. The commands of the next 10 minutes use it, so a folder loads in one round trip. The hash is 16 hex digits of the destination, because macOS limits a socket path to 104 bytes and `ssh` adds 17 characters while it makes a socket. The app makes the folder with mode 0700 and uses it only when it is private. Else each command opens its own connection.
- The connection only runs the script: `ClearAllForwardings=yes`, `ForwardAgent=no`, `ForwardX11=no`, `PermitLocalCommand=no`, and `RemoteCommand=none`.
- The app writes a script to the standard input of `ssh`. Only the words `/bin/sh -s` go through the login shell of the remote user (zsh, bash, fish). The script runs in a POSIX shell. The path goes in single quotes (`'it'\''s'`), and `~` becomes `"$HOME"`.
- The error output of `ssh` goes to a file without a name, not to a pipe: the shared connection runs on in the background and can keep the error output open.
- All `ssh` work runs on background threads: the loader thread for a listing, a `bf-fetch` thread for a copy.

### Listing

- One script lists a folder: `cd` to the folder, then the real path (`$PWD`), then one record for each entry, then the names of the symlinks to folders.
- macOS and the BSDs use BSD `stat -f`. Linux uses GNU `stat --printf`. The script reads `uname -s` to choose. On macOS, the script uses `/usr/bin/stat`, so a GNU `stat` in the `PATH` does no harm.
- Each field ends with NUL, and an empty field ends a section. A name can hold any byte except NUL and `/`, so the format is safe for all file names. A marker comes first: text that a login file prints (for example an `echo` in `.zshenv`) comes before the marker, and the parser ignores it. The word `end` comes last: a cut output gives an error, not a short list.
- The parser (`bf_core::remote::parse_listing`) makes the same `Entry` values as `fs::read_dir`: the kind and the execute bit from the mode, the size, the modification and birth times, hidden (a name with a first `.`, or the BSD `UF_HIDDEN` flag), packages, and symlinks to folders. A record with a bad name (`/`, `.`, `..`) does not go in the list.
- The exit code of the script tells the problem: no such folder, permission denied, or not a folder. Exit code 255 is an error of `ssh` itself. `bf_core::remote::error_text` makes a short message from the error output: SSH key login failed (with the command to test), unknown or changed host key, unknown host, no answer, connection refused (is Remote Login on?), not reachable, or connection closed.
- The pane stays in the old folder when a load fails, and the status bar shows `Cannot open admin@mac-mini:~: <problem>`.

### The folder in the list

- `Pane` keeps a `Location` (an optional host and a path). Open a folder, parent, refresh, and the history keep the host. `goto:<path>` and the Favorites always go to a local folder. The history can mix hosts.
- The title is `host:/path`. The window has no proxy icon. The status bar shows no free space.
- There is no FSEvents stream for a remote folder. `refresh` (`cmd+r`) reads it again.
- Icons and Kind come from the extension only (no path icons).
- The location field and the Go to Folder panel go to a path on the same host: an absolute path, a path relative to the folder, `~`, or `~/x`. They show no matching folders and `Tab` does not complete in this version.

### Files

- `quick_look` and `open` on a remote file copy it first: `ssh` runs `cat` on the host (after the marker), and the data goes to a hidden temporary file in `~/Library/Caches/com.p1ratedev.better-finder/remote/<host>/<path>`. A rename puts the copy in place. The copy has mode 0444 (read-only) and the modification time of the remote file.
- A copy with the same size and time serves again without a download (regular files only; a symlink copies again).
- After the copy, Quick Look shows it when its entry is still under the cursor, and `open` opens it with the normal rules (the default app, or the `editor` for a text file). The status bar says `Opened a read-only copy of "<name>"`.
- When Quick Look is visible and the cursor moves to a remote file, the copy starts. A new preview copy stops the old preview copy.
- After 250 ms, the status bar shows the progress: `Downloading "<name>"… 45%`. `cancel` (`Escape`) stops all copies, and `ssh` gets `SIGTERM`. A stopped copy leaves no file. A temporary file of an app that quit during a copy goes away with the next copy of the same file.
- A package (`.app`, `.pages`) does not open in this version. Its row menu has `Show Package Contents`.

### Actions that are not available

The app never changes a remote file in this version. In a remote folder, these actions do nothing and the status bar shows `Not available on a remote folder`: `rename`, `new_folder`, `trash`, `copy`, `paste`, `duplicate`, `make_alias`, `compress`, `get_info`, `share`, `open_terminal`, and `reveal_in_finder` (`app::local_only`). In the context menu, their items are disabled, and Open With does not show. A drag does not start from a remote row, a drop on a remote folder does nothing, and the Services get no remote entry. `eject` acts only on the selected sidebar row. `copy_path` copies `host:/path`.

### Test without a network

`BETTER_FINDER_SSH=<program>` replaces `/usr/bin/ssh`. A small script that skips the options and runs the command on this Mac (`exec "$@"`) gives a "remote" host for UI tests with `--debug-keys` and `--debug-screenshot`. The unit tests run the real scripts through `/bin/sh -s` on this Mac.

## Later (not in this round)

- Groups by date (`Today`, `Yesterday`, `Previous 7 Days`), like the Finder `Use Groups` option.
- A `Date Added` column. It needs `getattrlistbulk` (`ATTR_CMNEXT_ADDEDTIME`) in `bf_core::fs::read_dir`.
- Image thumbnails as icons (QuickLookThumbnailing, on a background queue, with a cache).
- Tags in the sidebar.
- Tags in the context menu (the color dots and Tags…).
- Remote folders: upload, rename, and trash; matches and completion of remote paths in the location field; packages; a cache limit.
