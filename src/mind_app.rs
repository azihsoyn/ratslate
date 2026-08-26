use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseEvent};
use ratatui_dnd::{Act, Sortable};

use crate::mindmap::{self, MindTree, NodeId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Editing(NodeId),
}

pub struct MindApp {
    pub lines: Vec<String>,
    pub tree: MindTree,
    pub sort: Sortable<(), NodeId>,
    pub selected: Option<NodeId>,
    pub mode: Mode,
    pub editing_text: String,
    pub save_path: PathBuf,
    pub status: String,
    pub should_quit: bool,
}

impl MindApp {
    pub fn new(save_path: PathBuf) -> Self {
        let (lines, status) = if save_path.exists() {
            match std::fs::read_to_string(&save_path) {
                Ok(content) => (
                    content.lines().map(String::from).collect(),
                    format!("loaded {}", save_path.display()),
                ),
                Err(e) => (Vec::new(), format!("failed to load {}: {e}", save_path.display())),
            }
        } else {
            (Vec::new(), format!("new file {}", save_path.display()))
        };
        let tree = mindmap::parse(&lines);

        Self {
            lines,
            tree,
            sort: Sortable::new(),
            selected: None,
            mode: Mode::Normal,
            editing_text: String::new(),
            save_path,
            status,
            should_quit: false,
        }
    }

    pub fn save(&mut self) {
        let content = if self.lines.is_empty() {
            String::new()
        } else {
            self.lines.join("\n") + "\n"
        };
        match std::fs::write(&self.save_path, content) {
            Ok(()) => self.status = format!("saved {}", self.save_path.display()),
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        match self.sort.on_mouse(ev) {
            Act::Click(id) => self.selected = Some(id),
            Act::Drop { key, slot, .. } => {
                let flat = self.tree.flat(Some(key));
                let target = self.tree.resolve_target(&flat, slot);
                match self.tree.plan_move(key, &target) {
                    Some(mv) => {
                        self.lines = mindmap::apply_move(&self.lines, &mv);
                        self.tree = mindmap::parse(&self.lines);
                        self.selected = None;
                        self.mode = Mode::Normal;
                        self.status.clear();
                    }
                    None => self.status = "can't nest a heading under a list item".to_string(),
                }
            }
            _ => {}
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }

        match self.mode {
            Mode::Editing(id) => match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    mindmap::set_text(&mut self.lines, &self.tree, id, &self.editing_text);
                    self.tree = mindmap::parse(&self.lines);
                    self.mode = Mode::Normal;
                    self.selected = None;
                }
                KeyCode::Backspace => {
                    self.editing_text.pop();
                }
                KeyCode::Char(c) => self.editing_text.push(c),
                _ => {}
            },
            Mode::Normal => match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Char('s') => self.save(),
                KeyCode::Enter => {
                    if let Some(id) = self.selected
                        && let Some(node) = self.tree.node(id)
                    {
                        self.editing_text = node.text.clone();
                        self.mode = Mode::Editing(id);
                    }
                }
                KeyCode::Esc => self.selected = None,
                _ => {}
            },
        }
    }
}
