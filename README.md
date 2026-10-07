# TDisk

A keyboard-driven disk-usage browser. Folder sizes arrive progressively while the scan runs.

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
| ? / F1 | Toggle help |
| q / Ctrl+C | Quit |

While help is open, Esc closes it and navigation keys scroll the help text. Esc at the scan root does nothing; quitting is always explicit. Going back restores the selected folder and previous scroll position.

Folders end with `/`. Files are displayed for inspection and are not opened. The selected row has both a highlight and a `›` marker. The item counter shows your position in the list. `scanning…` means a folder size is pending; `*` marks an incomplete size. Percentages use the currently known total and may change during scanning.

The layout adapts to terminal size: smaller windows omit share bars, very narrow windows prioritize filenames, and long paths keep the final component visible. The footer remains available during scanning, in empty folders, and after errors.

## UX references

The design applies visible controls, status feedback, familiar shortcuts, and explicit exits from [Jakob Nielsen's usability heuristics](https://www.nngroup.com/articles/ten-usability-heuristics/). Responsive layouts follow [Ratatui's terminal-size guidance](https://www.ratatui.rs/faq/). Help exposes less-frequent shortcuts without filling the main view with instructions.

## Performance and validation

Only visible rows are formatted. Opening help does not copy the file tree or pause scanning. After scanning, the UI redraws on input and resize events rather than continuously.

```sh
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
cargo run --release -- --benchmark /path/to/directory
```

