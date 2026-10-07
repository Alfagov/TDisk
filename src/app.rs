use crate::deletion::{self, TrashCandidate};
use crate::filesystem::{FileNode, Progress, ScanEvent, VolumeUsage};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

pub enum TrashDialog {
    Confirm(TrashCandidate),
    Notice(String),
}

pub struct App {
    pub root: Option<FileNode>,
    pub volume_usage: Option<VolumeUsage>,
    path: Vec<usize>,
    parent_scrolls: Vec<usize>,
    pub root_path: PathBuf,
    pub order: Vec<usize>,
    pub selected_index: usize,
    pub scroll: usize,
    pub scanning: bool,
    pub error: Option<String>,
    pub show_help: bool,
    pub help_scroll: u16,
    pub help_max_scroll: u16,
    pub page_height: usize,
    pub trash_dialog: Option<TrashDialog>,
    pub trash_dialog_scroll: u16,
    pub trash_dialog_max_scroll: u16,
    pub trash_confirmation_visible: bool,
    pub trashing: bool,
    pub trash_status: Option<String>,
    pub totals_stale: bool,
    trash_request: Option<TrashCandidate>,
    trash_index: Option<usize>,
    rescan_requested: bool,
    view_dirty: bool,
}

impl App {
    pub fn new(root_path: PathBuf) -> Self {
        Self {
            root: None,
            volume_usage: None,
            path: Vec::new(),
            parent_scrolls: Vec::new(),
            root_path,
            order: Vec::new(),
            selected_index: 0,
            scroll: 0,
            scanning: true,
            error: None,
            show_help: false,
            help_scroll: 0,
            help_max_scroll: 0,
            page_height: 1,
            trash_dialog: None,
            trash_dialog_scroll: 0,
            trash_dialog_max_scroll: 0,
            trash_confirmation_visible: false,
            trashing: false,
            trash_status: None,
            totals_stale: false,
            trash_request: None,
            trash_index: None,
            rescan_requested: false,
            view_dirty: false,
        }
    }

    pub fn apply(&mut self, event: ScanEvent) {
        match event {
            ScanEvent::VolumeUsage(usage) => self.volume_usage = Some(usage),
            ScanEvent::Started(root) => {
                self.root = Some(root);
                self.view_dirty = true;
            }
            ScanEvent::Directory { index, node } => {
                if let Some(root) = &mut self.root {
                    root.size = root.size - root.children[index].size + node.size;
                    root.errors += node.errors;
                    root.children[index] = node;
                    self.view_dirty |= self.path.is_empty() || self.path.first() == Some(&index);
                }
            }
            ScanEvent::Finished => {
                self.scanning = false;
                self.trash_status = None;
                if let Some(root) = &mut self.root {
                    root.pending = false;
                }
            }
            ScanEvent::Failed(message) => {
                self.scanning = false;
                self.trash_status = None;
                self.error = Some(message);
            }
        }
    }

    pub fn update_progress(&mut self, progress: &Progress) {
        if !self.scanning {
            return;
        }
        if let Some(sizes) = progress.root_sizes.get()
            && let Some(root) = &mut self.root
        {
            let mut changed = false;
            for (child, size) in root.children.iter_mut().zip(sizes) {
                if child.pending {
                    let size = size.load(Ordering::Relaxed);
                    changed |= child.size != size;
                    child.size = size;
                }
            }
            root.size = root.children.iter().map(|child| child.size).sum();
            self.view_dirty |= changed && self.path.is_empty();
        }
    }

    pub fn current_node(&self) -> Option<&FileNode> {
        let mut node = self.root.as_ref()?;
        for &index in &self.path {
            node = node.children.get(index)?;
        }
        Some(node)
    }

    pub fn selected_node(&self) -> Option<&FileNode> {
        self.current_node()?
            .children
            .get(*self.order.get(self.selected_index)?)
    }

    pub fn can_go_up(&self) -> bool {
        !self.path.is_empty()
    }

    pub fn can_open(&self) -> bool {
        self.selected_node().is_some_and(|node| node.is_dir)
    }

    pub fn can_trash(&self) -> bool {
        cfg!(target_os = "macos")
            && !self.scanning
            && !self.trashing
            && self
                .selected_node()
                .is_some_and(|node| !node.pending && node.skip_reason.is_none())
    }

    fn request_trash(&mut self) {
        let result = if self.scanning {
            Err("Wait for scanning to finish before moving an item to Trash.".into())
        } else if let Some(node) = self.selected_node() {
            if node.pending || node.skip_reason.is_some() {
                Err("Pending or excluded entries cannot be moved to Trash. Select a scanned file or folder.".into())
            } else {
                deletion::prepare(
                    &self.current_path().join(&node.name),
                    &self.root_path,
                    node.size,
                    node.errors > 0,
                )
            }
        } else {
            Err("Select a file or folder first.".into())
        };
        self.trash_index = self.order.get(self.selected_index).copied();
        self.trash_dialog = Some(match result {
            Ok(candidate) => TrashDialog::Confirm(candidate),
            Err(message) => TrashDialog::Notice(message),
        });
        self.trash_dialog_scroll = 0;
        self.trash_dialog_max_scroll = 0;
        self.trash_confirmation_visible = false;
    }

    pub fn take_trash_request(&mut self) -> Option<TrashCandidate> {
        self.trash_request.take()
    }

    pub fn finish_trash(&mut self, result: Result<(), String>) {
        self.trashing = false;
        match result {
            Ok(()) => {
                let row = self.selected_index;
                if let (Some(root), Some(index)) = (&mut self.root, self.trash_index.take()) {
                    fn remove(
                        node: &mut FileNode,
                        path: &[usize],
                        index: usize,
                    ) -> Option<(u64, u64)> {
                        let removed = if let Some((&child, tail)) = path.split_first() {
                            remove(node.children.get_mut(child)?, tail, index)?
                        } else {
                            if index >= node.children.len() {
                                return None;
                            }
                            let deleted = node.children.remove(index);
                            (deleted.size, deleted.errors)
                        };
                        node.size = node.size.saturating_sub(removed.0);
                        node.errors = node.errors.saturating_sub(removed.1);
                        Some(removed)
                    }
                    remove(root, &self.path, index);
                }
                self.reorder(None);
                self.selected_index = row.min(self.order.len().saturating_sub(1));
                self.totals_stale = true;
                self.trash_status = Some("Moved to Trash · Restore in Finder · R rescans sizes · Disk space may be unchanged until Trash is emptied".into());
            }
            Err(message) => {
                self.trash_index = None;
                self.trash_dialog = Some(TrashDialog::Notice(message));
                self.trash_dialog_scroll = 0;
                self.trash_status = Some("Move to Trash failed; results retained".into());
            }
        }
    }

    pub fn take_rescan_request(&mut self) -> bool {
        std::mem::take(&mut self.rescan_requested)
    }

    pub fn reset_for_rescan(&mut self) {
        let root_path = self.root_path.clone();
        *self = Self::new(root_path);
        self.trash_status = Some("Rescanning from the original root…".into());
    }

    /// Returns true only for an explicit quit command. Help captures navigation.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        if key.code == KeyCode::Char('q') {
            return !self.trashing;
        }
        if self.trashing {
            return false;
        }
        if self.trash_dialog.is_some() {
            match key.code {
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.trash_dialog = None;
                    self.trash_index = None;
                }
                KeyCode::Enter if matches!(self.trash_dialog, Some(TrashDialog::Notice(_))) => {
                    self.trash_dialog = None;
                    self.trash_index = None;
                }
                // Return and repeated keys never confirm deletion. The warning must
                // have been rendered through its final line before y becomes active.
                KeyCode::Char('y')
                    if key.kind == KeyEventKind::Press && self.trash_confirmation_visible =>
                {
                    if let Some(TrashDialog::Confirm(candidate)) = self.trash_dialog.take() {
                        self.trash_request = Some(candidate);
                        self.trashing = true;
                        self.trash_status = Some("Moving selected item to Trash…".into());
                    }
                }
                KeyCode::Down => {
                    self.trash_dialog_scroll = self
                        .trash_dialog_scroll
                        .saturating_add(1)
                        .min(self.trash_dialog_max_scroll)
                }
                KeyCode::Up => {
                    self.trash_dialog_scroll = self.trash_dialog_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => {
                    self.trash_dialog_scroll = self
                        .trash_dialog_scroll
                        .saturating_add(5)
                        .min(self.trash_dialog_max_scroll)
                }
                KeyCode::PageUp => {
                    self.trash_dialog_scroll = self.trash_dialog_scroll.saturating_sub(5)
                }
                KeyCode::Home => self.trash_dialog_scroll = 0,
                KeyCode::End => self.trash_dialog_scroll = self.trash_dialog_max_scroll,
                _ => {}
            }
            return false;
        }
        if matches!(key.code, KeyCode::Char('?') | KeyCode::F(1)) {
            self.show_help = !self.show_help;
            self.help_scroll = 0;
            return false;
        }
        if self.show_help {
            match key.code {
                KeyCode::Esc => self.show_help = false,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1).min(self.help_max_scroll)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => {
                    self.help_scroll = self.help_scroll.saturating_add(5).min(self.help_max_scroll)
                }
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(5),
                KeyCode::Home => self.help_scroll = 0,
                KeyCode::End => self.help_scroll = self.help_max_scroll,
                _ => {}
            }
            return false;
        }
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => self.next(),
            KeyCode::Up | KeyCode::Char('k') => self.previous(),
            KeyCode::PageDown => {
                self.selected_index = self
                    .selected_index
                    .saturating_add(self.page_height)
                    .min(self.order.len().saturating_sub(1))
            }
            KeyCode::PageUp => {
                self.selected_index = self.selected_index.saturating_sub(self.page_height)
            }
            KeyCode::Home => self.selected_index = 0,
            KeyCode::End => self.selected_index = self.order.len().saturating_sub(1),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.drill_down(),
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => self.go_up(),
            KeyCode::Char('d') | KeyCode::Delete if key.kind == KeyEventKind::Press => {
                self.request_trash()
            }
            KeyCode::Char('R') if !self.scanning => self.rescan_requested = true,
            KeyCode::Char('r') if self.can_go_up() => {
                let child = self.path[0];
                self.scroll = self.parent_scrolls[0];
                self.path.clear();
                self.parent_scrolls.clear();
                self.reorder(Some(child));
            }
            _ => {}
        }
        false
    }

    pub fn current_path(&self) -> PathBuf {
        let mut path = self.root_path.clone();
        if let Some(mut node) = self.root.as_ref() {
            for &index in &self.path {
                node = &node.children[index];
                path.push(&node.name);
            }
        }
        path
    }

    pub fn refresh(&mut self) {
        if self.view_dirty {
            let selected = self.order.get(self.selected_index).copied();
            self.reorder(selected);
        }
    }

    fn reorder(&mut self, selected: Option<usize>) {
        self.order = if let Some(node) = self.current_node() {
            let mut order: Vec<_> = (0..node.children.len()).collect();
            order.sort_unstable_by(|&a, &b| {
                node.children[b]
                    .size
                    .cmp(&node.children[a].size)
                    .then_with(|| node.children[a].name.cmp(&node.children[b].name))
            });
            order
        } else {
            Vec::new()
        };
        self.selected_index = selected
            .and_then(|index| self.order.iter().position(|&i| i == index))
            .unwrap_or(0);
        self.view_dirty = false;
    }

    pub fn drill_down(&mut self) {
        if let Some(&index) = self.order.get(self.selected_index)
            && self
                .current_node()
                .is_some_and(|node| node.children[index].is_dir)
        {
            self.parent_scrolls.push(self.scroll);
            self.path.push(index);
            self.scroll = 0;
            self.reorder(None);
        }
    }

    pub fn go_up(&mut self) {
        if let Some(index) = self.path.pop() {
            self.scroll = self.parent_scrolls.pop().unwrap_or(0);
            self.reorder(Some(index));
        }
    }

    pub fn next(&mut self) {
        if self.selected_index + 1 < self.order.len() {
            self.selected_index += 1;
        }
    }

    pub fn previous(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_borrows_tree_and_preserves_selection_across_updates() {
        let mut root = FileNode::new("root".into(), 0, true);
        root.children = vec![
            FileNode::new("a".into(), 0, true),
            FileNode::new("b".into(), 0, true),
        ];
        let mut app = App::new("root".into());
        app.apply(ScanEvent::Started(root));
        app.refresh();
        app.next();
        let mut b = FileNode::new("b".into(), 1024, true);
        b.pending = false;
        b.children.push(FileNode::new("file".into(), 1024, false));
        app.apply(ScanEvent::Directory { index: 1, node: b });
        app.refresh();
        assert_eq!(app.order[app.selected_index], 1);
        let ptr = &app.root.as_ref().unwrap().children[1] as *const FileNode;
        app.drill_down();
        assert_eq!(app.current_node().unwrap() as *const FileNode, ptr);
        assert_eq!(app.current_path(), PathBuf::from("root/b"));
        app.go_up();
        assert_eq!(app.order[app.selected_index], 1);
        app.previous();
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn partial_folder_sizes_are_visible_before_completion_and_not_added_twice() {
        let mut root = FileNode::new("root".into(), 5, true);
        root.children = vec![
            FileNode::new("folder".into(), 0, true),
            FileNode::new("file".into(), 5, false),
        ];
        let mut app = App::new("root".into());
        app.apply(ScanEvent::Started(root));
        app.refresh();
        let progress = Progress::default();
        progress
            .root_sizes
            .set(vec![
                std::sync::atomic::AtomicU64::new(10),
                std::sync::atomic::AtomicU64::new(5),
            ])
            .unwrap();
        app.update_progress(&progress);
        app.refresh();
        assert_eq!(app.root.as_ref().unwrap().size, 15);
        assert_eq!(app.root.as_ref().unwrap().children[0].size, 10);
        assert!(app.root.as_ref().unwrap().children[0].pending);
        progress.root_sizes.get().unwrap()[0].store(20, Ordering::Relaxed);
        app.update_progress(&progress);
        assert_eq!(app.root.as_ref().unwrap().size, 25);
        let mut finished = FileNode::new("folder".into(), 30, true);
        finished.pending = false;
        app.apply(ScanEvent::Directory {
            index: 0,
            node: finished,
        });
        app.update_progress(&progress);
        assert_eq!(app.root.as_ref().unwrap().size, 35);
        app.apply(ScanEvent::Finished);
        app.update_progress(&progress);
        assert_eq!(app.root.as_ref().unwrap().size, 35);
    }

    #[test]
    fn pending_folder_refreshes_when_its_scan_finishes() {
        let mut root = FileNode::new("root".into(), 0, true);
        root.children.push(FileNode::new("pending".into(), 0, true));
        let mut app = App::new("root".into());
        app.apply(ScanEvent::Started(root));
        app.refresh();
        app.drill_down();
        assert!(app.order.is_empty());
        let mut done = FileNode::new("pending".into(), 10, true);
        done.children.push(FileNode::new("file".into(), 10, false));
        done.pending = false;
        app.apply(ScanEvent::Directory {
            index: 0,
            node: done,
        });
        app.refresh();
        assert_eq!(app.order.len(), 1);
        assert!(!app.current_node().unwrap().pending);
        app.drill_down();
        assert_eq!(app.current_path(), PathBuf::from("root/pending"));
    }
}

#[cfg(test)]
mod keyboard_tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app() -> App {
        let mut root = FileNode::new("root".into(), 0, true);
        root.children = (0..100)
            .map(|i| FileNode::new(format!("{i:03}").into(), 0, true))
            .collect();
        let mut app = App::new("root".into());
        app.apply(ScanEvent::Started(root));
        app.apply(ScanEvent::Finished);
        app.refresh();
        app.page_height = 12;
        app
    }

    #[test]
    fn page_navigation_clamps_and_back_restores_position() {
        let mut app = app();
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(app.selected_index, 12);
        app.handle_key(key(KeyCode::End));
        assert_eq!(app.selected_index, 99);
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(app.selected_index, 99);
        app.scroll = 88;
        app.handle_key(key(KeyCode::Enter));
        assert!(app.can_go_up());
        assert!(app.order.is_empty());
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(app.selected_index, 0);
        assert!(!app.handle_key(key(KeyCode::Esc)));
        assert_eq!(app.selected_index, 99);
        assert_eq!(app.scroll, 88);
        app.handle_key(key(KeyCode::Home));
        app.handle_key(key(KeyCode::PageUp));
        assert_eq!(app.selected_index, 0);
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Char('r')));
        assert!(!app.can_go_up());
        assert!(
            !app.handle_key(key(KeyCode::Esc)),
            "Esc at root must not quit"
        );
    }

    #[test]
    fn help_captures_navigation_and_quit_remains_explicit() {
        let mut app = app();
        app.handle_key(key(KeyCode::F(1)));
        app.help_max_scroll = 10;
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.selected_index, 0);
        assert_eq!(app.help_scroll, 1);
        assert!(!app.can_go_up());
        app.handle_key(key(KeyCode::End));
        assert_eq!(app.help_scroll, 10);
        app.handle_key(key(KeyCode::Esc));
        assert!(!app.show_help);
        app.handle_key(key(KeyCode::Char('?')));
        assert!(app.show_help);
        assert!(app.handle_key(key(KeyCode::Char('q'))));
        assert!(app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        let mut release = key(KeyCode::Char('q'));
        release.kind = KeyEventKind::Release;
        assert!(!app.handle_key(release));
    }
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) mod trash_tests {
    use super::*;
    use crate::deletion::tests::Fixture;

    pub(crate) fn fixture_app() -> (Fixture, App) {
        let fixture = Fixture::new();
        let folder = fixture.0.join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("old.txt"), b"old").unwrap();
        std::fs::write(folder.join("keep.txt"), b"keep").unwrap();
        let mut branch = FileNode::new("folder".into(), 7, true);
        branch.pending = false;
        branch.children = vec![
            FileNode::new("old.txt".into(), 3, false),
            FileNode::new("keep.txt".into(), 4, false),
        ];
        let mut root = FileNode::new("root".into(), 7, true);
        root.children.push(branch);
        let mut app = App::new(fixture.0.clone());
        app.apply(ScanEvent::Started(root));
        app.apply(ScanEvent::Finished);
        app.refresh();
        app.drill_down();
        app.selected_index = app.order.iter().position(|&i| i == 0).unwrap();
        (fixture, app)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn cancel_return_repeat_and_unread_warning_never_request_deletion() {
        let (_fixture, mut app) = fixture_app();
        app.handle_key(key(KeyCode::Char('d')));
        assert!(matches!(app.trash_dialog, Some(TrashDialog::Confirm(_))));
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Char('y')));
        assert!(app.take_trash_request().is_none());
        app.trash_confirmation_visible = true;
        let mut repeat = key(KeyCode::Char('y'));
        repeat.kind = KeyEventKind::Repeat;
        app.handle_key(repeat);
        assert!(app.take_trash_request().is_none());
        app.handle_key(key(KeyCode::Esc));
        assert!(app.trash_dialog.is_none());
        assert!(app.current_path().join("old.txt").exists());
        app.scanning = true;
        app.handle_key(key(KeyCode::Char('d')));
        assert!(matches!(app.trash_dialog, Some(TrashDialog::Notice(_))));
        app.trash_confirmation_visible = true;
        app.handle_key(key(KeyCode::Char('y')));
        assert!(app.take_trash_request().is_none());
    }

    #[test]
    fn confirmed_result_updates_ancestors_without_losing_current_location() {
        let (fixture, mut app) = fixture_app();
        app.handle_key(key(KeyCode::Delete));
        app.trash_confirmation_visible = true;
        app.handle_key(key(KeyCode::Char('y')));
        let candidate = app.take_trash_request().unwrap();
        assert!(app.trashing);
        // Simulate a successful recoverable move entirely inside the test fixture.
        std::fs::rename(&candidate.path, fixture.0.join("simulated-trash")).unwrap();
        app.handle_key(key(KeyCode::Left));
        assert!(
            app.can_go_up(),
            "navigation is held until the result is applied"
        );
        app.finish_trash(Ok(()));
        assert!(!app.trashing);
        assert!(app.can_go_up());
        assert_eq!(app.root.as_ref().unwrap().size, 4);
        assert_eq!(app.current_node().unwrap().size, 4);
        assert_eq!(app.current_node().unwrap().children.len(), 1);
        assert_eq!(app.selected_node().unwrap().name, "keep.txt");
        assert!(app.totals_stale);
        app.handle_key(key(KeyCode::Char('R')));
        assert!(app.take_rescan_request());
        app.reset_for_rescan();
        assert!(app.scanning && !app.totals_stale);
        assert!(!app.can_go_up());
    }

    #[test]
    fn failed_operation_keeps_tree_and_explains_error() {
        let (_fixture, mut app) = fixture_app();
        app.handle_key(key(KeyCode::Char('d')));
        app.trash_confirmation_visible = true;
        app.handle_key(key(KeyCode::Char('y')));
        app.take_trash_request().unwrap();
        app.finish_trash(Err("Permission denied".into()));
        assert_eq!(app.root.as_ref().unwrap().size, 7);
        assert_eq!(app.current_node().unwrap().children.len(), 2);
        assert!(!app.totals_stale);
        assert!(
            matches!(&app.trash_dialog, Some(TrashDialog::Notice(message)) if message.contains("Permission denied"))
        );
    }
}
