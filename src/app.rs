use crate::filesystem::{FileNode, ScanEvent};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::path::PathBuf;

pub struct App {
    pub root: Option<FileNode>,
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
    view_dirty: bool,
}

impl App {
    pub fn new(root_path: PathBuf) -> Self {
        Self {
            root: None,
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
            view_dirty: false,
        }
    }

    pub fn apply(&mut self, event: ScanEvent) {
        match event {
            ScanEvent::Started(root) => {
                self.root = Some(root);
                self.view_dirty = true;
            }
            ScanEvent::Directory { index, node } => {
                if let Some(root) = &mut self.root {
                    root.size += node.size;
                    root.errors += node.errors;
                    root.children[index] = node;
                    self.view_dirty |= self.path.is_empty() || self.path.first() == Some(&index);
                }
            }
            ScanEvent::Finished => {
                self.scanning = false;
                if let Some(root) = &mut self.root {
                    root.pending = false;
                }
            }
            ScanEvent::Failed(message) => {
                self.scanning = false;
                self.error = Some(message);
            }
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
            return true;
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
