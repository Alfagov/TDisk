use crate::app::App;
use crate::filesystem::Progress;
use number_prefix::NumberPrefix;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph};
use std::sync::atomic::Ordering;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const BG: Color = Color::Rgb(15, 20, 29);
const PANEL: Color = Color::Rgb(22, 29, 40);
const BORDER: Color = Color::Rgb(62, 78, 96);
const TEXT: Color = Color::Rgb(233, 239, 245);
const MUTED: Color = Color::Rgb(160, 176, 195);
const ACCENT: Color = Color::Rgb(111, 211, 235);
const SELECTED: Color = Color::Rgb(36, 63, 82);
const WARNING: Color = Color::Rgb(245, 196, 107);

fn panel(title: impl Into<Line<'static>>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .title_style(Style::default().fg(MUTED))
        .style(Style::default().bg(PANEL).fg(TEXT))
        .title(title)
}

pub fn draw(f: &mut Frame, app: &mut App, progress: &Progress) {
    app.refresh();
    let area = f.area();
    f.render_widget(
        Block::default().style(Style::default().bg(BG).fg(TEXT)),
        area,
    );
    // Reserve the command bar first, so help and quit survive short terminal windows.
    let commands = command_lines(app, area.width.saturating_sub(2) as usize);
    let footer_height = (commands.len() as u16 + 2).min(area.height);
    let sections =
        Layout::vertical([Constraint::Min(0), Constraint::Length(footer_height)]).split(area);
    let footer = panel(" Commands ");
    f.render_widget(Paragraph::new(commands).block(footer), sections[1]);
    let body = Layout::vertical([
        Constraint::Length(if sections[0].height >= 10 { 4 } else { 0 }),
        Constraint::Min(0),
        Constraint::Length(if sections[0].height >= 7 { 1 } else { 0 }),
    ])
    .split(sections[0]);
    draw_header(f, app, progress, body[0]);
    draw_entries(f, app, body[1]);
    draw_selection(f, app, body[2]);
    if app.show_help {
        draw_help(f, app, sections[0]);
    }
}

fn draw_header(f: &mut Frame, app: &App, progress: &Progress, area: Rect) {
    if area.is_empty() {
        return;
    }
    let (label, color, summary) = if app.error.is_some() {
        (
            "SCAN ERROR",
            WARNING,
            "Some results may be unavailable".to_string(),
        )
    } else if app.scanning {
        (
            "SCANNING",
            WARNING,
            format!(
                "{} found · {} files · {} folders",
                format_size(progress.bytes.load(Ordering::Relaxed)),
                progress.files.load(Ordering::Relaxed),
                progress.directories.load(Ordering::Relaxed)
            ),
        )
    } else {
        let node = app.current_node();
        (
            "READY",
            ACCENT,
            format!(
                "{} allocated · {} items",
                format_size(node.map_or(0, |n| n.size)),
                app.order.len()
            ),
        )
    };
    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!(" {label} "),
                Style::default().fg(BG).bg(color).bold(),
            ),
            Span::styled(format!("  {summary}"), Style::default().fg(MUTED)),
        ]),
        Line::from(Span::styled(
            fit_path(
                &app.current_path().to_string_lossy(),
                area.width.saturating_sub(2) as usize,
            ),
            Style::default().fg(TEXT),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines).block(panel(Line::from(vec![
            Span::styled(" TDisk ", Style::default().fg(ACCENT).bold()),
            Span::styled("/ Disk usage ", Style::default().fg(MUTED)),
        ]))),
        area,
    );
}

fn draw_entries(f: &mut Frame, app: &mut App, area: Rect) {
    let count = app.order.len();
    let position = if count == 0 {
        " 0 items ".to_string()
    } else {
        format!(" {} / {} ", app.selected_index + 1, count)
    };
    let block = panel(" Files & folders ").title_top(Line::from(position).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let table_header = usize::from(inner.width >= 36 && inner.height >= 2);
    let rows = (inner.height as usize).saturating_sub(table_header);
    app.page_height = rows.max(1);
    if app.selected_index < app.scroll {
        app.scroll = app.selected_index;
    }
    if rows > 0 && app.selected_index >= app.scroll + rows {
        app.scroll = app.selected_index + 1 - rows;
    }
    app.scroll = app.scroll.min(count.saturating_sub(rows));

    if count == 0 {
        let back_hint = if app.can_go_up() {
            "Press Esc to go back."
        } else {
            "Press ? for help, or q to quit."
        };
        let message = if let Some(error) = &app.error {
            format!("Unable to scan\n{error}\n\nPress q to quit, or ? for help.")
        } else if app.current_node().is_some_and(|node| node.errors > 0) {
            format!("Folder could not be fully read.\nThe size shown is incomplete.\n\n{back_hint}")
        } else if app.current_node().is_some_and(|node| node.pending) || app.root.is_none() {
            format!("Reading this folder…\nSizes will appear as scanning completes.\n\n{back_hint}")
        } else {
            format!("This folder is empty.\n\n{back_hint}")
        };
        f.render_widget(
            Paragraph::new(message)
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED))
                .wrap(ratatui::widgets::Wrap { trim: false }),
            Rect {
                y: inner.y + inner.height.min(2),
                height: inner.height.saturating_sub(2),
                ..inner
            },
        );
        return;
    }
    let width = inner.width as usize;
    let detailed = width >= 76;
    let show_size = width >= 28;
    let name_width = width.saturating_sub(if detailed {
        41
    } else if show_size {
        14
    } else {
        0
    });
    if table_header > 0 {
        let mut label = format!("{} {:>12}", fit_name("   NAME", name_width), "ON DISK");
        if detailed {
            label.push_str("    SHARE OF FOLDER");
        }
        f.render_widget(
            Paragraph::new(label).style(Style::default().fg(MUTED).bg(BG)),
            Rect { height: 1, ..inner },
        );
    }
    let node = app.current_node().unwrap();
    // Only visible entries are formatted: keep large directories cheap to redraw.
    let items: Vec<_> = app
        .order
        .iter()
        .enumerate()
        .skip(app.scroll)
        .take(rows)
        .map(|(row, &index)| {
            let child = &node.children[index];
            let selected = row == app.selected_index;
            let name = format!(
                "{} {}{}",
                if selected { "›" } else { " " },
                child.name.to_string_lossy(),
                if child.is_dir { "/" } else { "" }
            );
            let mut spans = vec![Span::styled(
                fit_name(&name, name_width),
                Style::default().fg(if selected {
                    TEXT
                } else if child.is_dir {
                    ACCENT
                } else {
                    TEXT
                }),
            )];
            if show_size {
                let size = if child.pending {
                    "scanning…".into()
                } else if child.errors > 0 {
                    format!("{}*", format_size(child.size))
                } else {
                    format_size(child.size)
                };
                spans.push(Span::styled(
                    format!(" {size:>12} "),
                    Style::default().fg(if child.pending || child.errors > 0 {
                        WARNING
                    } else {
                        TEXT
                    }),
                ));
            }
            if detailed {
                if child.pending {
                    spans.push(Span::styled("  size pending", Style::default().fg(MUTED)));
                } else {
                    let percent = if node.size > 0 {
                        child.size as f64 / node.size as f64 * 100.0
                    } else {
                        0.0
                    };
                    let filled = ((percent / 100.0 * 18.0).round() as usize).min(18);
                    spans.push(Span::styled(
                        format!(
                            " {}{} {:>5.1}%",
                            "━".repeat(filled),
                            "─".repeat(18 - filled),
                            percent
                        ),
                        Style::default().fg(if selected { ACCENT } else { MUTED }),
                    ));
                }
            }
            ListItem::new(Line::from(spans)).style(if selected {
                Style::default().bg(SELECTED).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            })
        })
        .collect();
    f.render_widget(
        List::new(items),
        Rect {
            y: inner.y + table_header as u16,
            height: rows as u16,
            ..inner
        },
    );
}

fn draw_selection(f: &mut Frame, app: &App, area: Rect) {
    let text = if let Some(error) = &app.error {
        format!(" Scan error · {error}")
    } else if let Some(node) = app.current_node()
        && node.errors > 0
    {
        format!(
            " Partial results · {} skipped/errors · * marks an incomplete size",
            node.errors
        )
    } else if let Some(node) = app.selected_node() {
        let kind = if node.is_dir { "Folder" } else { "File" };
        let detail = if node.pending {
            "size pending".to_string()
        } else {
            format!("{} on disk", format_size(node.size))
        };
        format!(" {kind} · {detail} · {}", node.name.to_string_lossy())
    } else {
        " Sizes show allocated disk space · symlinks are excluded".into()
    };
    f.render_widget(
        Paragraph::new(fit_name(&text, area.width as usize)).style(Style::default().fg(MUTED)),
        area,
    );
}

fn command_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let enabled = !app.order.is_empty();
    let commands = if app.show_help {
        vec![
            ("Esc", "Close help", true),
            ("↑↓", "Scroll", true),
            ("q", "Quit", true),
        ]
    } else if width < 60 {
        vec![
            ("?", "Help", true),
            ("q", "Quit", true),
            ("↑↓", "Move", enabled),
            ("↵", "Open", app.can_open()),
            ("Esc", "Back", app.can_go_up()),
        ]
    } else {
        vec![
            ("↑↓", "Move", enabled),
            ("Enter", "Open", app.can_open()),
            ("Esc/←", "Back", app.can_go_up()),
            ("PgUp/Dn", "Page", enabled),
            ("Home/End", "Jump", enabled),
            ("r", "Root", app.can_go_up()),
            ("?", "Help", true),
            ("q", "Quit", true),
        ]
    };
    let mut lines = Vec::new();
    let mut spans = Vec::new();
    let mut used = 0;
    for (key, action, active) in commands {
        let length = key.width() + action.width() + 5;
        if used > 0 && used + length > width {
            lines.push(Line::from(std::mem::take(&mut spans)));
            used = 0;
        }
        let color = if active { ACCENT } else { MUTED };
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default()
                .fg(if active { BG } else { PANEL })
                .bg(color)
                .bold(),
        ));
        spans.push(Span::styled(
            format!(" {action}  "),
            Style::default().fg(if active { TEXT } else { MUTED }),
        ));
        used += length;
    }
    if !spans.is_empty() {
        lines.push(Line::from(spans));
    }
    lines
}

const HELP: &[(&str, &str)] = &[
    ("↑ / ↓   or   k / j", "Move selection"),
    ("Enter / → / l", "Open selected folder"),
    ("Esc / ← / h / Backspace", "Go to parent folder"),
    ("Page Up / Page Down", "Move one screen"),
    ("Home / End", "Jump to first / last item"),
    ("r", "Return to the scan root"),
    ("? / F1", "Toggle this help"),
    ("q / Ctrl+C", "Quit TDisk"),
];

fn draw_help(f: &mut Frame, app: &mut App, area: Rect) {
    let width = area.width.min(72);
    let height = area.height.min(if width >= 58 { 18 } else { 24 });
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let block = panel(Line::from(Span::styled(
        " Keyboard shortcuts ",
        Style::default().fg(ACCENT).bold(),
    )));
    let inner = block.inner(popup);
    let mut lines = vec![
        Line::from(Span::styled(
            "Navigate with arrows or Vim keys.",
            Style::default().fg(MUTED),
        )),
        Line::default(),
    ];
    for &(keys, action) in HELP {
        if inner.width >= 56 {
            lines.push(Line::from(vec![
                Span::styled(format!(" {keys:<26}"), Style::default().fg(ACCENT).bold()),
                Span::raw(action),
            ]));
        } else {
            lines.push(Line::from(Span::styled(
                keys,
                Style::default().fg(ACCENT).bold(),
            )));
            lines.push(Line::from(format!("  {action}")));
        }
    }
    lines.extend([
        Line::default(),
        Line::from("Folders end with /. Files are not opened."),
        Line::from("scanning… = pending   * = incomplete size"),
        Line::from("Sizes update while you browse."),
        Line::default(),
        Line::from(Span::styled(
            "Esc closes help · ↑↓ scroll · q quits",
            Style::default().fg(MUTED),
        )),
    ]);
    // Wrap explicitly so scroll limits stay correct even on a narrow terminal.
    let mut wrapped = Vec::new();
    for line in lines {
        let mut spans = Vec::new();
        let mut used = 0;
        for span in line.spans {
            for ch in span.content.chars() {
                let cells = ch.width().unwrap_or(0);
                if used > 0 && used + cells > inner.width as usize {
                    wrapped.push(Line::from(std::mem::take(&mut spans)));
                    used = 0;
                }
                spans.push(Span::styled(ch.to_string(), span.style));
                used += cells;
            }
        }
        wrapped.push(Line::from(spans));
    }
    app.help_max_scroll = wrapped
        .len()
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    app.help_scroll = app.help_scroll.min(app.help_max_scroll);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(wrapped)
            .scroll((app.help_scroll, 0))
            .block(block),
        popup,
    );
}

fn fit_name(name: &str, width: usize) -> String {
    let clean: String = name
        .chars()
        .map(|ch| if ch.is_control() { '�' } else { ch })
        .collect();
    let mut result = String::new();
    let truncated = clean.width() > width;
    let budget = width.saturating_sub(usize::from(truncated));
    let mut used = 0;
    for ch in clean.chars() {
        let cells = ch.width().unwrap_or(0);
        if used + cells > budget {
            break;
        }
        result.push(ch);
        used += cells;
    }
    if truncated && width > 0 {
        result.push('…');
        used += 1;
    }
    result.extend(std::iter::repeat_n(' ', width.saturating_sub(used)));
    result
}

fn fit_path(path: &str, width: usize) -> String {
    if path.width() <= width || width < 2 {
        return fit_name(path, width);
    }
    let mut tail = Vec::new();
    let mut used = 1;
    for ch in path.chars().rev() {
        let ch = if ch.is_control() { '�' } else { ch };
        let cells = ch.width().unwrap_or(0);
        if used + cells > width {
            break;
        }
        tail.push(ch);
        used += cells;
    }
    format!("…{}", tail.into_iter().rev().collect::<String>())
}

fn format_size(size: u64) -> String {
    match NumberPrefix::decimal(size as f64) {
        NumberPrefix::Standalone(bytes) => format!("{bytes} B"),
        NumberPrefix::Prefixed(prefix, n) => format!("{n:.1} {prefix}B"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::{FileNode, ScanEvent};
    use ratatui::{Terminal, backend::TestBackend};

    fn fixture_app() -> App {
        let mut root = FileNode::new("Projects".into(), 12_000_000_000, true);
        for (name, size, directory, pending) in [
            ("Archive", 6_000_000_000, true, false),
            ("Applications", 4_000_000_000, true, false),
            ("project-backup.tar", 2_000_000_000, false, false),
            ("Documents", 0, true, true),
        ] {
            let mut node = FileNode::new(name.into(), size, directory);
            node.pending = pending;
            root.children.push(node);
        }
        let mut app = App::new("/Users/alex/Projects".into());
        app.apply(ScanEvent::Started(root));
        app.refresh();
        app
    }

    fn screen(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn command_bar_and_help_adapt_to_terminal_width() {
        for width in [24, 40, 60, 80, 120] {
            let mut app = fixture_app();
            let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
            terminal
                .draw(|frame| draw(frame, &mut app, &Progress::default()))
                .unwrap();
            let text = screen(&terminal);
            assert!(text.contains("Help") && text.contains("Quit"));
            assert!(text.contains("Archive/"));
            for line in command_lines(&app, width as usize - 2) {
                assert!(line.width() <= width as usize - 2);
            }
            app.show_help = true;
            terminal
                .draw(|frame| draw(frame, &mut app, &Progress::default()))
                .unwrap();
            let text = screen(&terminal);
            assert!(text.contains("Keyboard shortcuts"));
            assert!(text.contains("Close help"));
            app.help_scroll = app.help_max_scroll;
            terminal
                .draw(|frame| draw(frame, &mut app, &Progress::default()))
                .unwrap();
            assert!(screen(&terminal).contains("quits"));
        }
    }

    // Optional buffer export lets visual QA inspect exactly what Ratatui rendered.
    #[test]
    fn visual_states() {
        for (name, width, help) in [
            ("wide", 120, false),
            ("compact", 60, false),
            ("help", 90, true),
        ] {
            let mut app = fixture_app();
            app.show_help = help;
            let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
            terminal
                .draw(|frame| draw(frame, &mut app, &Progress::default()))
                .unwrap();
            if let Ok(directory) = std::env::var("TDISK_SNAPSHOT_DIR") {
                std::fs::create_dir_all(&directory).unwrap();
                let buffer = terminal.backend().buffer();
                let mut output = format!("{} {}\n", width, 24);
                for cell in &buffer.content {
                    let rgb = |color| {
                        if let Color::Rgb(r, g, b) = color {
                            format!("{r},{g},{b}")
                        } else {
                            "233,239,245".into()
                        }
                    };
                    output.push_str(&format!(
                        "{}\t{}\t{}\n",
                        rgb(cell.fg),
                        rgb(cell.bg),
                        cell.symbol()
                    ));
                }
                std::fs::write(
                    std::path::Path::new(&directory).join(format!("{name}.cells")),
                    output,
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn unicode_names_fit_without_panicking() {
        for width in 0..30 {
            for name in ["日本語📁é.txt", "a\nb\tc", "e\u{301}", "plain"] {
                let fitted = fit_name(name, width);
                assert!(fitted.width() <= width);
                assert!(!fitted.contains('\n'));
            }
        }
    }

    #[test]
    fn scroll_reaches_last_entry_and_tiny_terminals_render() {
        let mut root = FileNode::new("root".into(), 0, true);
        root.children = (0..10000)
            .map(|i| FileNode::new(format!("entry-{i:05}").into(), 0, false))
            .collect();
        let mut app = App::new("root".into());
        app.apply(ScanEvent::Started(root));
        app.apply(ScanEvent::Finished);
        app.refresh();
        app.selected_index = 9999;
        for (width, height) in [(100, 20), (10, 8), (1, 1), (0, 0)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| draw(frame, &mut app, &Progress::default()))
                .unwrap();
            if width == 100 {
                let content: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(content.contains("entry-09999"));
            }
        }
    }
}
