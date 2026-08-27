use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent};
use ratatui_dnd::{Act, Sortable};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::mindmap::{self, Kind, MindTree, NodeId, Target};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Editing(NodeId),
}

/// Every way the document can change. The TUI's drag/drop and edit
/// handlers build one of these and hand it to [`MindApp::dispatch`]
/// just like `--api` does — one path, so a script and a drag can never
/// disagree about what a move means.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Re-parent and/or reposition a node: `parent` is the new
    /// parent's id (`null` for the top level), `index` is where among
    /// that parent's other children it lands, counted with the moved
    /// node itself left out.
    Move {
        id: NodeId,
        parent: Option<NodeId>,
        index: usize,
    },
    /// Replace a node's own text.
    SetText { id: NodeId, text: String },
    /// Append a new, empty top-level list item — the only way to start
    /// a document from nothing, since every other request needs a node
    /// that already exists to act on.
    AddItem,
    /// Select a node, or clear the selection with `null`.
    Select { id: Option<NodeId> },
    /// The whole tree.
    State,
    /// Write the document to the path given on the command line.
    Save,
    /// Undo the last change that touched the document.
    Undo,
    /// Redo the last change undo stepped back through.
    Redo,
}

impl Request {
    fn mutates(&self) -> bool {
        !matches!(
            self,
            Request::Select { .. } | Request::State | Request::Save | Request::Undo | Request::Redo
        )
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Added { id: NodeId },
    State { roots: Vec<TreeNode> },
    Saved { path: String },
    Undone { done: bool },
    Redone { done: bool },
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TreeNode {
    pub id: NodeId,
    pub kind: String,
    pub text: String,
    pub children: Vec<TreeNode>,
}

fn tree_node(tree: &MindTree, id: NodeId) -> TreeNode {
    let node = tree.node(id).expect("id came from this tree");
    TreeNode {
        id,
        kind: match node.kind {
            Kind::Heading(_) => "heading".to_string(),
            Kind::ListItem(_) => "list_item".to_string(),
        },
        text: node.text.clone(),
        children: node.children.iter().map(|&c| tree_node(tree, c)).collect(),
    }
}

const UNDO_LIMIT: usize = 100;

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
    undo_stack: Vec<Vec<String>>,
    redo_stack: Vec<Vec<String>>,
    /// The save file's mtime as of our own last read or write of it, so
    /// a change from outside — a script, `--api`, another editor — can
    /// be told apart from an echo of our own save.
    file_mtime: Option<std::time::SystemTime>,
    /// Whether the document has changed since it was last loaded or
    /// saved — an external change must never overwrite this, or a
    /// script racing a human's in-progress, not-yet-saved edit would
    /// silently win.
    dirty: bool,
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
        let file_mtime = mtime_of(&save_path);

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
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            file_mtime,
            dirty: false,
        }
    }

    /// Picks up a change made to the save file from outside this
    /// process, the same way an undoable edit does, so it can be
    /// Ctrl+Z'd away if it wasn't wanted. Skipped mid-gesture, so an
    /// incoming change can't yank a node out from under an active drag
    /// or a keystroke out of an active edit — and skipped outright, not
    /// just deferred, while there are local changes not yet saved: an
    /// outside change and an unsaved local one are a genuine conflict,
    /// and silently picking the outside one would erase work the human
    /// never got a chance to save.
    pub fn reload_if_changed(&mut self) {
        if self.mode != Mode::Normal || self.sort.held().is_some() {
            return;
        }
        let Some(mtime) = mtime_of(&self.save_path) else { return };
        if Some(mtime) == self.file_mtime {
            return;
        }
        if self.dirty {
            self.file_mtime = Some(mtime);
            self.status = "external change waiting — save (s) first to keep your own edits".to_string();
            return;
        }
        self.file_mtime = Some(mtime);
        match std::fs::read_to_string(&self.save_path) {
            Ok(content) => {
                self.push_undo();
                self.lines = content.lines().map(String::from).collect();
                self.tree = mindmap::parse(&self.lines);
                self.selected = None;
                self.dirty = false;
                self.status = format!("reloaded {} (changed externally)", self.save_path.display());
            }
            Err(e) => self.status = format!("failed to reload {}: {e}", self.save_path.display()),
        }
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(self.lines.clone());
        if self.undo_stack.len() > UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn undo(&mut self) -> bool {
        let Some(prev) = self.undo_stack.pop() else {
            self.status = "nothing to undo".to_string();
            return false;
        };
        self.redo_stack.push(std::mem::replace(&mut self.lines, prev));
        self.tree = mindmap::parse(&self.lines);
        self.selected = None;
        self.mode = Mode::Normal;
        self.dirty = true;
        self.status = "undone".to_string();
        true
    }

    fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            self.status = "nothing to redo".to_string();
            return false;
        };
        self.undo_stack.push(std::mem::replace(&mut self.lines, next));
        self.tree = mindmap::parse(&self.lines);
        self.selected = None;
        self.mode = Mode::Normal;
        self.dirty = true;
        self.status = "redone".to_string();
        true
    }

    pub fn save(&mut self) {
        let content = if self.lines.is_empty() {
            String::new()
        } else {
            self.lines.join("\n") + "\n"
        };
        match std::fs::write(&self.save_path, content) {
            Ok(()) => {
                self.status = format!("saved {}", self.save_path.display());
                self.file_mtime = mtime_of(&self.save_path);
                self.dirty = false;
            }
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    /// The single place every document mutation goes through, whether
    /// it came from a mouse drag, a keystroke, or `--api`.
    pub fn dispatch(&mut self, req: Request) -> Result<Response, String> {
        if req.mutates() {
            self.push_undo();
            self.dirty = true;
        }
        match req {
            Request::Move { id, parent, index } => {
                self.tree.node(id).ok_or_else(|| format!("no such node: {id}"))?;
                if let Some(pid) = parent {
                    self.tree.node(pid).ok_or_else(|| format!("no such node: {pid}"))?;
                }
                let target = Target { parent, index };
                let mv = self
                    .tree
                    .plan_move(id, &target)
                    .ok_or("can't nest a heading under a list item")?;
                self.lines = mindmap::apply_move(&self.lines, &mv);
                self.tree = mindmap::parse(&self.lines);
                if self.selected == Some(id) {
                    self.selected = None;
                }
                Ok(Response::Ok)
            }
            Request::SetText { id, text } => {
                self.tree.node(id).ok_or_else(|| format!("no such node: {id}"))?;
                mindmap::set_text(&mut self.lines, &self.tree, id, &text);
                self.tree = mindmap::parse(&self.lines);
                Ok(Response::Ok)
            }
            Request::AddItem => {
                self.lines.push("- ".to_string());
                self.tree = mindmap::parse(&self.lines);
                let id = *self.tree.roots.last().expect("just pushed a root list item");
                self.selected = Some(id);
                Ok(Response::Added { id })
            }
            Request::Select { id } => {
                self.selected = id;
                Ok(Response::Ok)
            }
            Request::State => {
                let roots = self.tree.roots.clone();
                Ok(Response::State {
                    roots: roots.into_iter().map(|id| tree_node(&self.tree, id)).collect(),
                })
            }
            Request::Save => {
                self.save();
                Ok(Response::Saved { path: self.save_path.display().to_string() })
            }
            Request::Undo => Ok(Response::Undone { done: self.undo() }),
            Request::Redo => Ok(Response::Redone { done: self.redo() }),
        }
    }

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        match self.sort.on_mouse(ev) {
            Act::Click(id) => {
                let _ = self.dispatch(Request::Select { id: Some(id) });
            }
            Act::Drop { key, slot, .. } => {
                let flat = self.tree.flat(Some(key));
                let target = self.tree.resolve_target(&flat, slot);
                match self.dispatch(Request::Move { id: key, parent: target.parent, index: target.index }) {
                    Ok(_) => {
                        self.status.clear();
                        self.mode = Mode::Normal;
                    }
                    Err(msg) => self.status = msg,
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
                    let text = std::mem::take(&mut self.editing_text);
                    let _ = self.dispatch(Request::SetText { id, text });
                    self.mode = Mode::Normal;
                    self.selected = None;
                }
                KeyCode::Backspace => {
                    self.editing_text.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.editing_text.push(c);
                }
                _ => {}
            },
            Mode::Normal => match key.code {
                KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.undo();
                }
                KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.redo();
                }
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Char('s') => {
                    let _ = self.dispatch(Request::Save);
                }
                // The only way to get a first node into an empty
                // document — everything else acts on one that already
                // exists.
                KeyCode::Char('a') => {
                    if let Ok(Response::Added { id }) = self.dispatch(Request::AddItem) {
                        self.editing_text.clear();
                        self.mode = Mode::Editing(id);
                    }
                }
                KeyCode::Enter => {
                    if let Some(id) = self.selected
                        && let Some(node) = self.tree.node(id)
                    {
                        self.editing_text = node.text.clone();
                        self.mode = Mode::Editing(id);
                    }
                }
                KeyCode::Esc => {
                    if self.selected.is_some() {
                        let _ = self.dispatch(Request::Select { id: None });
                    } else {
                        self.should_quit = true;
                    }
                }
                _ => {}
            },
        }
    }
}

/// Parse and apply one `--api` request, already decoded from JSON. `kind`
/// (the request's own `type` tag) labels the envelope so a batch reply
/// can be matched back up.
pub fn run_one(app: &mut MindApp, kind: &str, value: serde_json::Value) -> serde_json::Value {
    match serde_json::from_value::<Request>(value) {
        Ok(req) => match app.dispatch(req) {
            Ok(resp) => serde_json::json!({"id": kind, "result": resp}),
            Err(message) => serde_json::json!({"id": kind, "error": {"message": message}}),
        },
        Err(e) => serde_json::json!({"id": kind, "error": {"message": e.to_string()}}),
    }
}

fn mtime_of(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}
