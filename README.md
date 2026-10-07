# TDisk

A disk-usage browser for macOS, with a native SwiftUI app and a keyboard-driven TUI. Folder sizes arrive progressively while the scan runs.

Open `TDiskMac/TDiskMac.xcodeproj`, select the **TDiskMac** scheme and **My Mac**, then press **⌘R**. Xcode builds the optimized Rust scanner automatically. See [macOS app setup and architecture](TDiskMac/README.md).

```sh
cargo run --release -- /path/to/directory
```

## Navigation

The command bar stays at the bottom. Highlighted key labels show the main shortcuts; unavailable actions are muted. On narrow terminals the bar prioritizes Help, Quit, Move, Open, and Back. Press `?` or `F1` for the complete shortcut list.

| Key | Action |
| --- | --- |
| ↑ / ↓ or k / j | Move selection |
| Enter / → / l | Open selected folder |
| Esc / ← / h / Backspace | Go to parent folder |
| Page Up / Page Down | Move one visible page |
| Home / End | First / last item |
| r | Return to the original scan root |
| d / Delete | Move the selected file or folder to Trash (macOS) |
| R (Shift+r) | Rescan from the original root after scanning finishes |
| ? / F1 | Toggle help |
| q / Ctrl+C | Quit |

While help is open, Esc closes it and navigation keys scroll the help text. Esc at the scan root does nothing; quitting is always explicit. Going back restores the selected folder and previous scroll position.

## Moving items to Trash

After scanning finishes, select an ordinary file or folder and press **d** or **Delete**. A warning shows the full resolved path, scanned allocation, and that a folder's contents move with it. Press **y** to confirm, or **Esc/n** to cancel. Enter and repeated keys never confirm. In a small terminal, scroll the warning with arrows/Page Up/Page Down/End before confirmation becomes available.

TDisk uses macOS's native Move to Trash API. There is **no permanent-delete fallback**, administrator prompt, or Empty Trash command. Restore items using Finder. Moving an item to Trash usually does not free space on its volume until Trash is emptied.

The scan root and its parents, mounted-volume roots, and folders containing mounts are blocked. Important macOS directory containers such as `/System`, `/Library`, `/usr`, `/bin`, `/sbin`, `/dev`, `/etc`, `/var`, `/private`, `/Users`, `/Applications`, and `/Volumes` are protected **themselves**, along with core infrastructure containers such as `/System/Library`, `/usr/bin`, and `/private/var/db`. This protection does **not** blanket-block their contents. Files and ordinary subfolders inside `/Library`, `~/Library`, and other locations can be moved to Trash after confirmation, subject to macOS permissions.

Home folders and standard containers (Desktop, Documents, Downloads, Pictures, Music, Movies, Public, Applications, Library) are also protected themselves; their contents remain eligible. Trash folders and their contents stay protected. Startup Data-volume aliases and mixed-case paths follow the same rules. Symlinks, pending entries, and excluded entries are not eligible.

The app rechecks the selected item's filesystem identity, scan boundaries, protections, and mount inventory immediately before calling macOS. Permission failures keep the results intact and show an error. Navigation is held only while a move is in progress, so the result applies to the same selected location.

Successful moves remove the row and update its ancestor allocations. Remaining sizes are labeled as a snapshot because hard-link attribution and external changes may require a new scan. Press **R** to rebuild sizes from the original root; this returns navigation to the root. Volume usage is refreshed independently and is never reduced by an assumed amount “freed.” Move to Trash is currently unavailable on other operating systems.

The native app implements the [macOS design](design/TDisk-macOS-design.md), with a locations sidebar, sortable folder table, inspector, scan cancellation, native menus and confirmed Move to Trash. **⌘⌫** opens the warning; Escape cancels. The confirmation button must be explicitly activated, and Return alone never confirms.

Folders end with `/`. Files are displayed for inspection and are not opened. The selected row has both a highlight and a `›` marker. The item counter shows your position in the list. `scanning…` means a folder size is pending; `≥` shows the allocated bytes found so far while scanning continues; `*` marks an incomplete size. Percentages use the currently known total and may change during scanning.

The layout adapts to terminal size: smaller windows omit share bars, very narrow windows prioritize filenames, and long paths keep the final component visible. The footer remains available during scanning, in empty folders, and after errors.

## Disk usage and scan boundaries

The header reports system-provided volume used/total/available space immediately, independently of the folder scan. For `/`, this is the startup Data volume, with its mount name displayed. The folder total is labeled **file allocation** and represents the regular-file bytes discovered by the scanner.

A scan does not descend into other mounted volumes. For `/`, this excludes backing mounts under `/System/Volumes` and mounted disks under `/Volumes`, while retaining the normal `/Users`, `/Applications`, and `/Library` view. To scan an external disk or the Data volume directly, pass its own mount path as the root. Excluded entries are labeled `other volume`.

Directories are visited once per filesystem identity. Hard-linked files contribute bytes only once per scan; later references are labeled `counted elsewhere`. When links are in different folders, one folder owns their contribution. APFS clones can still share physical blocks, snapshots consume space outside the visible tree, and inaccessible files produce partial totals. Thus summed file allocation and system volume usage are different measurements.

Exact recursive totals for arbitrary existing folders still require traversal. The public `ATTR_DIR_ALLOCSIZE` and `ATTR_DIR_DATALENGTH` attributes describe the directory itself, not the bytes in its entire subtree. This app displays live lower bounds as the traversal runs rather than using directory metadata as a recursive total. See [Apple's filesystem attribute documentation](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/man/man2/getattrlist.2) and [statfs documentation](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/statfs.2.html).

## UX references

The design applies visible controls, status feedback, familiar shortcuts, and explicit exits from [Jakob Nielsen's usability heuristics](https://www.nngroup.com/articles/ten-usability-heuristics/). Responsive layouts follow [Ratatui's terminal-size guidance](https://www.ratatui.rs/faq/). Help exposes less-frequent shortcuts without filling the main view with instructions.

## Performance and validation

Only visible rows are formatted. Opening help does not copy the file tree or pause scanning. After scanning, the UI redraws on input and resize events rather than continuously.

```sh
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
cargo run --release -- --benchmark /path/to/directory
```
