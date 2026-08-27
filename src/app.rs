use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui_dnd::{Did, Drag, Hits};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::canvas_io::{self, FileRoot};
use crate::collab::{Collab, EdgeFields, NodeFields};
use crate::mind_app::MindApp;
use crate::model::{Canvas, Color, Edge, EdgeEnd, Node, NodeKind, Shape, ShapeId, Side};

const MIN_W: u16 = 5;
const MIN_H: u16 = 3;
const UNDO_LIMIT: usize = 100;
const DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Which end of a connector a `Reattach` handle is — a grab there redraws
/// that end while the other stays put.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endpoint {
    From,
    To,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitTarget {
    Move(ShapeId),
    Resize(ShapeId, Corner),
    Connect(ShapeId),
    /// A small handle at a connector's own exit/entry point, not a box
    /// at all — dragging it re-points that end at a different box.
    Reattach(String, Endpoint),
}

impl HitTarget {
    /// The node this hit belongs to — every variant but `Reattach`,
    /// which belongs to an edge instead.
    fn node_id(&self) -> Option<&ShapeId> {
        match self {
            HitTarget::Move(id) | HitTarget::Connect(id) | HitTarget::Resize(id, _) => Some(id),
            HitTarget::Reattach(..) => None,
        }
    }
}

/// What's currently under the selection — a box or a connector, never
/// both. Doubles as the payload of `Mode::Editing`: whichever is
/// selected is what a click or an Enter would go on to edit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selected {
    Node(ShapeId),
    Edge(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Editing(Selected),
    /// Typing a file box's path rather than a text box's contents — a
    /// separate mode from `Editing` since committing it means something
    /// different (`SetFilePath`, not `SetText`).
    EditingPath(ShapeId),
}

/// Every way the board can change. The TUI's mouse and key handlers
/// build one of these and hand it to [`App::dispatch`] just like
/// `--api` does — one path, so a script and a drag can never disagree
/// about what a move means.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Place a new box, top-left at (x, y). `w`/`h` default to the usual
    /// placed size if omitted. Give `path` to place a file box (opens
    /// the embedded markdown editor on double-click) instead of a plain
    /// text one.
    Place {
        x: u16,
        y: u16,
        #[serde(default)]
        w: Option<u16>,
        #[serde(default)]
        h: Option<u16>,
        #[serde(default)]
        path: Option<String>,
    },
    /// Replace a box's text outright.
    SetText { id: ShapeId, text: String },
    /// Turn a box into a file box pointing at `path` (relative to the
    /// board's own save path), or retarget one that already is.
    /// Double-clicking a file box opens `path` in the embedded markdown
    /// editor instead of editing it as text.
    SetFilePath { id: ShapeId, path: String },
    /// Move and/or resize a box to an exact rectangle.
    SetRect { id: ShapeId, x: u16, y: u16, w: u16, h: u16 },
    /// A JSON Canvas preset "1".."6", a hex string like "#ff8800", or
    /// null to clear it.
    SetColor { id: ShapeId, color: Option<String> },
    /// "rectangle" (the default) or "rounded".
    SetShape { id: ShapeId, shape: String },
    /// Draw an arrow from one box to another.
    Connect { from: ShapeId, to: ShapeId },
    /// Remove a box and any connectors touching it.
    Delete { id: ShapeId },
    /// Label a connector, or clear its label with `null`.
    SetLabel { id: String, label: Option<String> },
    /// Remove a connector without touching the boxes it joined.
    DeleteEdge { id: String },
    /// Re-point one end of a connector at a different box: `end` is
    /// "from" or "to".
    Reattach { id: String, end: String, node: ShapeId },
    /// Select a box or a connector, or clear the selection with `null`.
    Select { id: Option<Selected> },
    /// The whole board, as JSON Canvas.
    State,
    /// Write the board to the path given on the command line.
    Save,
    /// Undo the last change that touched the board.
    Undo,
    /// Redo the last change undo stepped back through.
    Redo,
}

impl Request {
    /// Whether this changes the board in a way undo should be able to
    /// step back through. Select/State/Save/Undo/Redo themselves don't.
    fn mutates(&self) -> bool {
        !matches!(
            self,
            Request::Select { .. } | Request::State | Request::Save | Request::Undo | Request::Redo
        )
    }
}

// serde can't derive on an enum with a `String` and a `ShapeId` (also a
// `String`) sharing a tag position across variants cleanly here, so
// `Selected` gets its own small wire shape instead of leaning on serde's
// externally-tagged default, which would ask for `{"Node": "..."}`.
impl<'de> Deserialize<'de> for Selected {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize, JsonSchema)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum Wire {
            Node { id: ShapeId },
            Edge { id: String },
        }
        Ok(match Wire::deserialize(d)? {
            Wire::Node { id } => Selected::Node(id),
            Wire::Edge { id } => Selected::Edge(id),
        })
    }
}

impl Serialize for Selected {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let (kind, id) = match self {
            Selected::Node(id) => ("node", id),
            Selected::Edge(id) => ("edge", id),
        };
        let mut st = s.serialize_struct("Selected", 2)?;
        st.serialize_field("kind", kind)?;
        st.serialize_field("id", id)?;
        st.end()
    }
}

impl JsonSchema for Selected {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Selected".into()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        #[derive(JsonSchema)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        #[allow(dead_code)]
        enum Wire {
            Node { id: ShapeId },
            Edge { id: String },
        }
        g.subschema_for::<Wire>()
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Placed { id: ShapeId },
    Ok,
    State { board: FileRoot },
    Saved { path: String },
    Undone { done: bool },
    Redone { done: bool },
}

pub struct App {
    pub canvas: Canvas,
    pub drag: Drag<HitTarget>,
    pub hits: Hits<HitTarget>,
    pub edge_hits: Hits<String>,
    pub selected: Option<Selected>,
    pub mode: Mode,
    pub editing_text: String,
    pub should_quit: bool,
    pub save_path: Option<PathBuf>,
    pub status: String,
    /// Refreshed by `render` every frame, so a height grown while typing
    /// has something to clamp against without threading it through
    /// `on_key`.
    pub canvas_area: Rect,
    undo_stack: Vec<Canvas>,
    redo_stack: Vec<Canvas>,
    grab_offset: (u16, u16),
    resize_origin: Option<(ShapeId, Corner, Rect)>,
    /// A rectangle being dragged out on empty canvas: (press point,
    /// point the cursor is at now). `Request::Place` fires on release,
    /// sized from wherever the two ended up.
    drawing: Option<((u16, u16), (u16, u16))>,
    press_on_edge: Option<(String, u16, u16)>,
    /// What was last clicked (not dragged) and when, so a second click
    /// on the same thing within `DOUBLE_CLICK` opens it for editing
    /// instead of just selecting it again.
    last_click: Option<(Selected, std::time::Instant)>,
    /// The board's live CRDT merge state, kept in lockstep with
    /// `canvas` — every node- or edge-affecting dispatch mirrors into
    /// it too. `None` with no save path (nothing to merge against).
    collab: Option<Collab>,
    /// A file box opened for editing takes over the whole screen rather
    /// than living in its own separate top-level mode — `Some` while
    /// one's open, covering the canvas until it's closed (Esc) and
    /// saved back to its own file.
    pub fullscreen: Option<MindApp>,
}

impl App {
    pub fn new(save_path: Option<PathBuf>) -> Self {
        let mut canvas = Canvas::default();
        let mut status = String::new();

        if let Some(path) = &save_path {
            if path.exists() {
                match canvas_io::load(path) {
                    Ok(loaded) => {
                        canvas = loaded;
                        status = format!("loaded {}", path.display());
                    }
                    Err(e) => status = format!("failed to load {}: {e}", path.display()),
                }
            } else {
                status = format!("new file {}", path.display());
            }
        }

        let mut collab = save_path.as_deref().map(Collab::open);
        if let Some(collab) = &mut collab {
            let nodes = collab.snapshot();
            if nodes.is_empty() {
                // First run for this board: seed the CRDT from whatever
                // the JSON file already had, so later merges have a
                // common ancestor instead of starting from nothing.
                for node in &canvas.nodes {
                    collab.set_node(&node.id, &node_fields(node));
                }
            } else {
                // The CRDT already has state — possibly ahead of the
                // JSON file, if changes landed through it since the
                // last explicit save — so it wins for the node set.
                canvas.nodes = nodes.into_iter().map(|(id, f)| node_from_fields(id, f)).collect();
            }
            let edges = collab.snapshot_edges();
            if edges.is_empty() {
                for edge in &canvas.edges {
                    collab.set_edge(&edge.id, &edge_fields(edge));
                }
            } else {
                canvas.edges = edges.into_iter().map(|(id, f)| edge_from_fields(id, f)).collect();
            }
        }

        Self {
            canvas,
            drag: Drag::new(),
            hits: Hits::new(),
            edge_hits: Hits::new(),
            selected: None,
            mode: Mode::Normal,
            editing_text: String::new(),
            should_quit: false,
            save_path,
            status,
            canvas_area: Rect::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            grab_offset: (0, 0),
            resize_origin: None,
            drawing: None,
            press_on_edge: None,
            last_click: None,
            collab,
            fullscreen: None,
        }
    }

    /// Where a file box's `path` points, resolved relative to the
    /// board's own save location — the same convention JSON Canvas file
    /// nodes use elsewhere (Obsidian included).
    fn resolve_file_path(&self, path: &str) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            return p;
        }
        match self.save_path.as_ref().and_then(|s| s.parent()) {
            Some(dir) => dir.join(p),
            None => p,
        }
    }

    /// Merges in any node or edge changes another writer has made —
    /// safe to call unconditionally, since CRDT merges never discard a
    /// local change that hasn't reached disk yet. Skipped only
    /// mid-gesture, so it can't yank a box out from under an active
    /// drag or a keystroke out of an active edit.
    pub fn pull_collab(&mut self) {
        if self.mode != Mode::Normal || self.drag.moving().is_some() || self.drawing.is_some() {
            return;
        }
        let Some(collab) = &mut self.collab else { return };
        if !collab.pull() {
            return;
        }
        let nodes = collab.snapshot();
        let edges = collab.snapshot_edges();
        self.push_undo();
        self.canvas.nodes = nodes.into_iter().map(|(id, f)| node_from_fields(id, f)).collect();
        self.canvas.edges = edges.into_iter().map(|(id, f)| edge_from_fields(id, f)).collect();
        let selection_gone = match &self.selected {
            Some(Selected::Node(id)) => !self.canvas.nodes.iter().any(|n| &n.id == id),
            Some(Selected::Edge(id)) => !self.canvas.edges.iter().any(|e| &e.id == id),
            None => false,
        };
        if selection_gone {
            self.selected = None;
        }
        self.status = "merged a change from another writer".to_string();
    }

    /// Mirrors one node's current field values into the CRDT so another
    /// writer's next pull sees them — pulling first so this local
    /// process's own possibly-stale view of *other* nodes never gets
    /// blindly re-asserted over a concurrent change to them. A dispatch
    /// only ever names the one node it just touched, so that's the only
    /// key this needs to write; a wholesale re-mirror of every node on
    /// every keystroke was what let a writer with a stale in-memory copy
    /// stomp someone else's fresher edit to an unrelated (or the same)
    /// box.
    fn sync_node(&mut self, id: &ShapeId) {
        let Some(collab) = &mut self.collab else { return };
        collab.pull();
        match self.canvas.node(id) {
            Some(node) => collab.set_node(id, &node_fields(node)),
            None => collab.remove_node(id),
        }
    }

    /// Same as [`Self::sync_node`], for the one connector a dispatch
    /// just touched.
    fn sync_edge(&mut self, id: &str) {
        let Some(collab) = &mut self.collab else { return };
        collab.pull();
        match self.canvas.edge(id) {
            Some(edge) => collab.set_edge(id, &edge_fields(edge)),
            None => collab.remove_edge(id),
        }
    }

    /// Mirrors the *entire* node and edge set into the CRDT, overwriting
    /// whatever any other entry currently holds. Only safe for
    /// undo/redo, where the whole point is snapping everything back to
    /// this process's own recorded snapshot; anywhere else, prefer
    /// [`Self::sync_node`] / [`Self::sync_edge`].
    fn resync_collab_full(&mut self) {
        let Some(collab) = &mut self.collab else { return };
        collab.pull();
        let current_nodes: std::collections::HashSet<&str> = self.canvas.nodes.iter().map(|n| n.id.as_str()).collect();
        for node in &self.canvas.nodes {
            collab.set_node(&node.id, &node_fields(node));
        }
        let stale_nodes: Vec<String> = collab
            .snapshot()
            .into_iter()
            .map(|(id, _)| id)
            .filter(|id| !current_nodes.contains(id.as_str()))
            .collect();
        for id in stale_nodes {
            collab.remove_node(&id);
        }
        let current_edges: std::collections::HashSet<&str> = self.canvas.edges.iter().map(|e| e.id.as_str()).collect();
        for edge in &self.canvas.edges {
            collab.set_edge(&edge.id, &edge_fields(edge));
        }
        let stale_edges: Vec<String> = collab
            .snapshot_edges()
            .into_iter()
            .map(|(id, _)| id)
            .filter(|id| !current_edges.contains(id.as_str()))
            .collect();
        for id in stale_edges {
            collab.remove_edge(&id);
        }
    }

    /// True the second time this is called for the same target within
    /// `DOUBLE_CLICK` of the first.
    fn is_double_click(&mut self, target: Selected) -> bool {
        let now = std::time::Instant::now();
        let is_double = matches!(&self.last_click, Some((prev, at)) if *prev == target && now.duration_since(*at) < DOUBLE_CLICK);
        self.last_click = Some((target, now));
        is_double
    }

    pub fn save(&mut self) {
        let Some(path) = self.save_path.clone() else {
            self.status = "no file given on the command line — nothing to save to".to_string();
            return;
        };
        match canvas_io::save(&self.canvas, &path) {
            Ok(()) => self.status = format!("saved {}", path.display()),
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(self.canvas.clone());
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
        self.redo_stack.push(std::mem::replace(&mut self.canvas, prev));
        self.selected = None;
        self.mode = Mode::Normal;
        self.status = "undone".to_string();
        self.resync_collab_full();
        true
    }

    fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            self.status = "nothing to redo".to_string();
            return false;
        };
        self.undo_stack.push(std::mem::replace(&mut self.canvas, next));
        self.selected = None;
        self.mode = Mode::Normal;
        self.status = "redone".to_string();
        self.resync_collab_full();
        true
    }

    /// The single place every board mutation goes through, whether it
    /// came from a mouse drag, a keystroke, or `--api`.
    pub fn dispatch(&mut self, req: Request) -> Result<Response, String> {
        let mutates = req.mutates();
        if mutates {
            self.push_undo();
        }
        let mut touched: Option<ShapeId> = None;
        let mut touched_edge: Option<String> = None;
        let mut removed_edges: Vec<String> = Vec::new();
        let result = match req {
            Request::Place { x, y, w, h, path } => {
                let id = self.canvas.place_text(x, y, w, h);
                if let Some(path) = path
                    && let Some(node) = self.canvas.node_mut(&id)
                {
                    node.kind = NodeKind::File { path, subpath: None };
                }
                touched = Some(id.clone());
                Ok(Response::Placed { id })
            }
            Request::SetText { id, text } => {
                self.canvas.node(&id).ok_or_else(|| format!("no such node: {id}"))?;
                self.canvas.edit_text(&id, |t| {
                    t.clear();
                    t.push_str(&text);
                });
                touched = Some(id);
                Ok(Response::Ok)
            }
            Request::SetFilePath { id, path } => {
                let node = self.canvas.node_mut(&id).ok_or_else(|| format!("no such node: {id}"))?;
                node.kind = NodeKind::File { path, subpath: None };
                touched = Some(id);
                Ok(Response::Ok)
            }
            Request::SetRect { id, x, y, w, h } => {
                let node = self.canvas.node_mut(&id).ok_or_else(|| format!("no such node: {id}"))?;
                node.rect = Rect::new(x, y, w.max(1), h.max(1));
                touched = Some(id);
                Ok(Response::Ok)
            }
            Request::SetColor { id, color } => {
                let node = self.canvas.node_mut(&id).ok_or_else(|| format!("no such node: {id}"))?;
                node.color = color.as_deref().map(Color::parse);
                touched = Some(id);
                Ok(Response::Ok)
            }
            Request::SetShape { id, shape } => {
                let node = self.canvas.node_mut(&id).ok_or_else(|| format!("no such node: {id}"))?;
                node.shape = crate::model::Shape::parse(&shape);
                touched = Some(id);
                Ok(Response::Ok)
            }
            Request::Connect { from, to } => {
                self.canvas.node(&from).ok_or_else(|| format!("no such node: {from}"))?;
                self.canvas.node(&to).ok_or_else(|| format!("no such node: {to}"))?;
                touched_edge = Some(self.canvas.connect(from, to));
                Ok(Response::Ok)
            }
            Request::Delete { id } => {
                removed_edges = self
                    .canvas
                    .edges
                    .iter()
                    .filter(|e| e.from == id || e.to == id)
                    .map(|e| e.id.clone())
                    .collect();
                self.canvas.delete(&id);
                if self.selected == Some(Selected::Node(id.clone())) {
                    self.selected = None;
                }
                touched = Some(id);
                Ok(Response::Ok)
            }
            Request::SetLabel { id, label } => {
                let edge = self.canvas.edge_mut(&id).ok_or_else(|| format!("no such connector: {id}"))?;
                edge.label = label.filter(|l| !l.is_empty());
                touched_edge = Some(id);
                Ok(Response::Ok)
            }
            Request::DeleteEdge { id } => {
                self.canvas.delete_edge(&id);
                if self.selected == Some(Selected::Edge(id.clone())) {
                    self.selected = None;
                }
                removed_edges.push(id);
                Ok(Response::Ok)
            }
            Request::Reattach { id, end, node } => {
                self.canvas.node(&node).ok_or_else(|| format!("no such node: {node}"))?;
                let edge = self.canvas.edge_mut(&id).ok_or_else(|| format!("no such connector: {id}"))?;
                match end.as_str() {
                    "from" if edge.to == node => return Err("can't connect a box to itself".to_string()),
                    "from" => edge.from = node,
                    "to" if edge.from == node => return Err("can't connect a box to itself".to_string()),
                    "to" => edge.to = node,
                    other => return Err(format!("end must be \"from\" or \"to\", got {other:?}")),
                }
                touched_edge = Some(id);
                Ok(Response::Ok)
            }
            Request::Select { id } => {
                self.selected = id;
                Ok(Response::Ok)
            }
            Request::State => Ok(Response::State { board: canvas_io::to_file(&self.canvas) }),
            Request::Save => {
                self.save();
                Ok(Response::Saved {
                    path: self.save_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
                })
            }
            Request::Undo => Ok(Response::Undone { done: self.undo() }),
            Request::Redo => Ok(Response::Redone { done: self.redo() }),
        };
        if result.is_ok() {
            if let Some(id) = &touched {
                self.sync_node(id);
            }
            if let Some(id) = &touched_edge {
                self.sync_edge(id);
            }
            for id in &removed_edges {
                self.sync_edge(id);
            }
        }
        result
    }

    /// The rectangle a drag-to-place is currently outlining, clamped
    /// the same way the box it creates on release will be.
    pub fn drawing_preview(&self) -> Option<Rect> {
        let (start, end) = self.drawing?;
        let x = start.0.min(end.0);
        let y = start.1.min(end.1);
        let w = (start.0.max(end.0) - x + 1).max(MIN_W);
        let h = (start.1.max(end.1) - y + 1).max(MIN_H);
        Some(Rect::new(x, y, w, h))
    }

    /// Where a resize-in-progress would land, for the preview ghost.
    pub fn resize_preview(&self, bounds: Rect) -> Option<(ShapeId, Rect)> {
        let Some(HitTarget::Resize(id, corner)) = self.drag.moving() else {
            return None;
        };
        let (origin_id, origin_corner, origin_rect) = self.resize_origin.as_ref()?;
        if origin_id != id || origin_corner != corner {
            return None;
        }
        let (cx, cy) = self.drag.cursor()?;
        Some((id.clone(), resized_rect(*origin_rect, *corner, cx, cy, bounds)))
    }

    /// Write whatever is in the scratch buffer back to whatever is
    /// being edited and leave edit mode. Called before any new press is
    /// acted on, so clicking away from an edit-in-progress can never
    /// lose it the way jumping straight to a different mode used to.
    fn commit_edit(&mut self) {
        match self.mode.clone() {
            Mode::Editing(target) => {
                let text = std::mem::take(&mut self.editing_text);
                let _ = match target {
                    Selected::Node(id) => self.dispatch(Request::SetText { id, text }),
                    Selected::Edge(id) => self.dispatch(Request::SetLabel { id, label: Some(text) }),
                };
            }
            Mode::EditingPath(id) => {
                let path = std::mem::take(&mut self.editing_text);
                let _ = self.dispatch(Request::SetFilePath { id, path });
            }
            Mode::Normal => return,
        }
        self.mode = Mode::Normal;
    }

    fn begin_edit(&mut self, target: Selected) {
        if let Selected::Node(id) = &target
            && let Some(NodeKind::File { path, .. }) = self.canvas.node(id).map(|n| &n.kind)
        {
            if path.is_empty() {
                self.status = "no path set on this file box yet — press f to set one".to_string();
                return;
            }
            self.fullscreen = Some(MindApp::new(self.resolve_file_path(path)));
            return;
        }
        self.editing_text = match &target {
            Selected::Node(id) => match self.canvas.node(id).map(|n| &n.kind) {
                Some(NodeKind::Text(t)) => t.clone(),
                _ => String::new(),
            },
            Selected::Edge(id) => self.canvas.edge(id).and_then(|e| e.label.clone()).unwrap_or_default(),
        };
        self.mode = Mode::Editing(target);
    }

    /// Grows the box being edited so a newline, or text wrapping past
    /// its width, never runs past the bottom border. Only grows — a
    /// box does not shrink back down as text is deleted.
    fn grow_to_fit(&mut self, id: &ShapeId) {
        let Some(node) = self.canvas.node(id) else { return };
        let inner_width = node.rect.width.saturating_sub(2).max(1);
        let needed = wrapped_height(&self.editing_text, inner_width);
        if needed <= node.rect.height {
            return;
        }
        let (x, y, w) = (node.rect.x, node.rect.y, node.rect.width);
        let max_h = self.canvas_area.bottom().saturating_sub(y).max(MIN_H);
        let _ = self.dispatch(Request::SetRect { id: id.clone(), x, y, w, h: needed.min(max_h) });
    }

    /// Whatever box is registered at this cell right now, ignoring a
    /// `Reattach` handle if that's what's actually there — a drop
    /// target has to be a box, never another connector's own handle.
    fn node_at(&self, x: u16, y: u16) -> Option<ShapeId> {
        self.hits.at(x, y)?.0.node_id().cloned()
    }

    /// The node closest to this cell — a drop that missed a box by a
    /// little should still connect, not silently do nothing.
    fn nearest_node(&self, x: u16, y: u16) -> Option<ShapeId> {
        self.canvas
            .nodes
            .iter()
            .min_by_key(|n| rect_distance(n.rect, x, y))
            .map(|n| n.id.clone())
    }

    pub fn on_mouse(&mut self, ev: MouseEvent, canvas_area: Rect) {
        let mut hit = self.hits.at(ev.column, ev.row);

        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            self.commit_edit();

            // Shift turns a grab on a box into a connector draw instead
            // of a move — the same body hit, reinterpreted.
            if ev.modifiers.contains(KeyModifiers::SHIFT)
                && let Some((HitTarget::Move(id), rect)) = &hit
            {
                hit = Some((HitTarget::Connect(id.clone()), *rect));
            }

            match &hit {
                Some((HitTarget::Move(_), rect)) => {
                    self.grab_offset = (
                        ev.column.saturating_sub(rect.x),
                        ev.row.saturating_sub(rect.y),
                    );
                    self.drawing = None;
                    self.press_on_edge = None;
                }
                Some(_) => {
                    self.drawing = None;
                    self.press_on_edge = None;
                }
                None => {
                    if let Some((edge_id, _)) = self.edge_hits.at(ev.column, ev.row) {
                        self.press_on_edge = Some((edge_id, ev.column, ev.row));
                        self.drawing = None;
                    } else {
                        self.press_on_edge = None;
                        self.drawing = canvas_area
                            .contains(Position::new(ev.column, ev.row))
                            .then_some(((ev.column, ev.row), (ev.column, ev.row)));
                    }
                }
            }
        }

        if let MouseEventKind::Drag(MouseButton::Left) = ev.kind
            && let Some((start, _)) = self.drawing
        {
            let cx = ev.column.clamp(canvas_area.x, canvas_area.right().saturating_sub(1));
            let cy = ev.row.clamp(canvas_area.y, canvas_area.bottom().saturating_sub(1));
            self.drawing = Some((start, (cx, cy)));
        }

        match self.drag.on_mouse(ev, hit) {
            Did::Click(HitTarget::Reattach(edge_id, _)) => {
                let selected = Selected::Edge(edge_id);
                let _ = self.dispatch(Request::Select { id: Some(selected.clone()) });
                if self.is_double_click(selected.clone()) {
                    self.begin_edit(selected);
                }
            }
            Did::Click(target) => {
                let selected = Selected::Node(target.node_id().expect("only Reattach lacks a node").clone());
                let _ = self.dispatch(Request::Select { id: Some(selected.clone()) });
                if self.is_double_click(selected.clone()) {
                    self.begin_edit(selected);
                }
            }
            Did::Lift(HitTarget::Resize(id, corner)) => {
                if let Some(node) = self.canvas.node(&id) {
                    self.resize_origin = Some((id, corner, node.rect));
                }
            }
            Did::Drop {
                key: HitTarget::Move(id),
                x,
                y,
            } => {
                if let Some(node) = self.canvas.node(&id) {
                    let (w, h) = (node.rect.width, node.rect.height);
                    let max_x = canvas_area.right().saturating_sub(w).max(canvas_area.x);
                    let max_y = canvas_area.bottom().saturating_sub(h).max(canvas_area.y);
                    let nx = x.saturating_sub(self.grab_offset.0).clamp(canvas_area.x, max_x);
                    let ny = y.saturating_sub(self.grab_offset.1).clamp(canvas_area.y, max_y);
                    let _ = self.dispatch(Request::SetRect { id, x: nx, y: ny, w, h });
                }
            }
            Did::Drop {
                key: HitTarget::Resize(id, corner),
                x,
                y,
            } => {
                if let Some((origin_id, origin_corner, origin_rect)) = self.resize_origin.take()
                    && origin_id == id
                    && origin_corner == corner
                {
                    let r = resized_rect(origin_rect, corner, x, y, canvas_area);
                    let _ = self.dispatch(Request::SetRect { id, x: r.x, y: r.y, w: r.width, h: r.height });
                }
            }
            Did::Drop {
                key: HitTarget::Connect(from),
                x,
                y,
            } => {
                let to = self.node_at(x, y).or_else(|| self.nearest_node(x, y));
                match to {
                    Some(to) if to != from => {
                        let _ = self.dispatch(Request::Connect { from, to });
                        self.status.clear();
                    }
                    Some(_) => self.status = "can't connect a box to itself".to_string(),
                    None => self.status = "nothing to connect to".to_string(),
                }
            }
            Did::Drop {
                key: HitTarget::Reattach(id, end),
                x,
                y,
            } => {
                let node = self.node_at(x, y).or_else(|| self.nearest_node(x, y));
                match node {
                    Some(node) => {
                        let end = match end {
                            Endpoint::From => "from",
                            Endpoint::To => "to",
                        };
                        match self.dispatch(Request::Reattach { id, end: end.to_string(), node }) {
                            Ok(_) => self.status.clear(),
                            Err(msg) => self.status = msg,
                        }
                    }
                    None => self.status = "nothing to attach to".to_string(),
                }
            }
            _ => {}
        }

        if let MouseEventKind::Up(MouseButton::Left) = ev.kind {
            if let Some((edge_id, px, py)) = self.press_on_edge.take()
                && px == ev.column
                && py == ev.row
            {
                let selected = Selected::Edge(edge_id);
                let _ = self.dispatch(Request::Select { id: Some(selected.clone()) });
                if self.is_double_click(selected.clone()) {
                    self.begin_edit(selected);
                }
            } else if let Some((start, end)) = self.drawing.take()
                && start != end
            {
                let x = start.0.min(end.0);
                let y = start.1.min(end.1);
                let w = (start.0.max(end.0) - x + 1).max(MIN_W);
                let h = (start.1.max(end.1) - y + 1).max(MIN_H);
                if let Ok(Response::Placed { id }) =
                    self.dispatch(Request::Place { x, y, w: Some(w), h: Some(h), path: None })
                {
                    let selected = Selected::Node(id);
                    self.selected = Some(selected.clone());
                    self.begin_edit(selected);
                }
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }

        match self.mode.clone() {
            Mode::Editing(target) => match key.code {
                KeyCode::Esc => self.commit_edit(),
                // A box holds a note, where a newline is normal text; a
                // connector's label is one line, so Enter there means
                // "done" the way it does everywhere else in Normal mode.
                KeyCode::Enter => match &target {
                    Selected::Node(id) => {
                        self.editing_text.push('\n');
                        self.grow_to_fit(id);
                    }
                    Selected::Edge(_) => self.commit_edit(),
                },
                KeyCode::Backspace => {
                    self.editing_text.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.editing_text.push(c);
                    if let Selected::Node(id) = &target {
                        self.grow_to_fit(id);
                    }
                }
                _ => {}
            },
            Mode::EditingPath(id) => match key.code {
                KeyCode::Esc | KeyCode::Enter => {
                    let path = std::mem::take(&mut self.editing_text);
                    let _ = self.dispatch(Request::SetFilePath { id, path });
                    self.mode = Mode::Normal;
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
                // Ctrl+Shift+Z can't be told apart from Ctrl+Z over most
                // terminals' legacy key encoding (no shift bit on a
                // control byte), so redo gets its own key rather than a
                // shift that will not arrive.
                KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.redo();
                }
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Char('s') => {
                    let _ = self.dispatch(Request::Save);
                }
                KeyCode::Enter => {
                    if let Some(target) = self.selected.clone() {
                        self.begin_edit(target);
                    }
                }
                KeyCode::Char('c') => {
                    if let Some(Selected::Node(id)) = self.selected.clone()
                        && let Some(node) = self.canvas.node(&id)
                    {
                        let next = Color::cycle(node.color.as_ref());
                        let _ = self.dispatch(Request::SetColor { id, color: next.map(|c| c.to_string()) });
                    }
                }
                KeyCode::Char('x') => {
                    if let Some(Selected::Node(id)) = self.selected.clone()
                        && let Some(node) = self.canvas.node(&id)
                    {
                        let next = node.shape.cycle();
                        let shape = next.as_str().unwrap_or("rectangle").to_string();
                        let _ = self.dispatch(Request::SetShape { id, shape });
                    }
                }
                // Sets (or edits) a file box's path. A plain text box
                // becomes one; a file box that already has a path
                // starts from it, so this doubles as "retarget".
                KeyCode::Char('f') => {
                    if let Some(Selected::Node(id)) = self.selected.clone() {
                        self.editing_text = match self.canvas.node(&id).map(|n| &n.kind) {
                            Some(NodeKind::File { path, .. }) => path.clone(),
                            _ => String::new(),
                        };
                        self.mode = Mode::EditingPath(id);
                    }
                }
                KeyCode::Char('d') | KeyCode::Delete => match self.selected.clone() {
                    Some(Selected::Node(id)) => {
                        let _ = self.dispatch(Request::Delete { id });
                    }
                    Some(Selected::Edge(id)) => {
                        let _ = self.dispatch(Request::DeleteEdge { id });
                    }
                    None => {}
                },
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
pub fn run_one(app: &mut App, kind: &str, value: serde_json::Value) -> serde_json::Value {
    match serde_json::from_value::<Request>(value) {
        Ok(req) => match app.dispatch(req) {
            Ok(resp) => serde_json::json!({"id": kind, "result": resp}),
            Err(message) => serde_json::json!({"id": kind, "error": {"message": message}}),
        },
        Err(e) => serde_json::json!({"id": kind, "error": {"message": e.to_string()}}),
    }
}

fn resized_rect(origin: Rect, corner: Corner, cx: u16, cy: u16, bounds: Rect) -> Rect {
    let cx = cx.clamp(bounds.x, bounds.right().saturating_sub(1));
    let cy = cy.clamp(bounds.y, bounds.bottom().saturating_sub(1));
    let (left, top, right, bottom) = (origin.x, origin.y, origin.right(), origin.bottom());

    let (new_left, new_right) = match corner {
        Corner::TopLeft | Corner::BottomLeft => (cx.min(right.saturating_sub(MIN_W)), right),
        Corner::TopRight | Corner::BottomRight => (left, cx.max(left + MIN_W)),
    };
    let (new_top, new_bottom) = match corner {
        Corner::TopLeft | Corner::TopRight => (cy.min(bottom.saturating_sub(MIN_H)), bottom),
        Corner::BottomLeft | Corner::BottomRight => (top, cy.max(top + MIN_H)),
    };

    Rect::new(new_left, new_top, new_right - new_left, new_bottom - new_top)
}

/// Cells from `(x, y)` to the nearest edge of `r`, 0 if inside it — the
/// same measure ratatui-dnd's own `sort::Sortable` uses to let a drop
/// just past a border still land.
fn rect_distance(r: Rect, x: u16, y: u16) -> u32 {
    let dx = if x < r.x {
        r.x - x
    } else {
        x.saturating_sub(r.x + r.width.saturating_sub(1))
    };
    let dy = if y < r.y {
        r.y - y
    } else {
        y.saturating_sub(r.y + r.height.saturating_sub(1))
    };
    dx as u32 + dy as u32
}

/// How tall a box needs to be to show `text` without clipping, wrapped
/// to `width` columns the same simple way `Paragraph`'s `Wrap` would,
/// plus the top and bottom border.
fn wrapped_height(text: &str, width: u16) -> u16 {
    let width = width.max(1) as usize;
    let content_lines: u16 = text
        .split('\n')
        .map(|line| {
            let len = line.chars().count();
            if len == 0 { 1 } else { len.div_ceil(width) as u16 }
        })
        .sum();
    content_lines.max(1) + 2
}

fn node_fields(node: &Node) -> NodeFields {
    let (kind, text) = match &node.kind {
        NodeKind::Text(t) => ("text", t.clone()),
        // `subpath` doesn't round-trip — no board so far has used it,
        // and it's not worth a ninth CRDT field until one does.
        NodeKind::File { path, .. } => ("file", path.clone()),
        NodeKind::Link(url) => ("link", url.clone()),
        NodeKind::Group { label, .. } => ("group", label.clone().unwrap_or_default()),
    };
    NodeFields {
        x: node.rect.x as i64,
        y: node.rect.y as i64,
        w: node.rect.width as i64,
        h: node.rect.height as i64,
        text,
        color: node.color.as_ref().map(|c| c.to_string()),
        shape: node.shape.as_str().unwrap_or("rectangle").to_string(),
        kind: kind.to_string(),
    }
}

fn node_from_fields(id: String, f: NodeFields) -> Node {
    let kind = match f.kind.as_str() {
        "file" => NodeKind::File { path: f.text, subpath: None },
        "link" => NodeKind::Link(f.text),
        "group" => NodeKind::Group { label: Some(f.text).filter(|s| !s.is_empty()), background: None, background_style: None },
        _ => NodeKind::Text(f.text),
    };
    Node {
        id,
        rect: Rect::new(
            f.x.clamp(0, u16::MAX as i64) as u16,
            f.y.clamp(0, u16::MAX as i64) as u16,
            f.w.clamp(1, u16::MAX as i64) as u16,
            f.h.clamp(1, u16::MAX as i64) as u16,
        ),
        color: f.color.as_deref().map(Color::parse),
        shape: Shape::parse(&f.shape),
        kind,
    }
}

fn edge_fields(edge: &Edge) -> EdgeFields {
    EdgeFields {
        from: edge.from.clone(),
        to: edge.to.clone(),
        from_side: edge.from_side.map(side_to_string).map(str::to_string),
        to_side: edge.to_side.map(side_to_string).map(str::to_string),
        from_end: edge_end_to_string(edge.from_end).to_string(),
        to_end: edge_end_to_string(edge.to_end).to_string(),
        color: edge.color.as_ref().map(|c| c.to_string()),
        label: edge.label.clone(),
    }
}

fn edge_from_fields(id: String, f: EdgeFields) -> Edge {
    Edge {
        id,
        from: f.from,
        from_side: f.from_side.as_deref().and_then(parse_side),
        from_end: parse_edge_end(&f.from_end),
        to: f.to,
        to_side: f.to_side.as_deref().and_then(parse_side),
        to_end: parse_edge_end(&f.to_end),
        color: f.color.as_deref().map(Color::parse),
        label: f.label,
    }
}

fn side_to_string(s: Side) -> &'static str {
    match s {
        Side::Top => "top",
        Side::Right => "right",
        Side::Bottom => "bottom",
        Side::Left => "left",
    }
}

fn parse_side(s: &str) -> Option<Side> {
    match s {
        "top" => Some(Side::Top),
        "right" => Some(Side::Right),
        "bottom" => Some(Side::Bottom),
        "left" => Some(Side::Left),
        _ => None,
    }
}

fn edge_end_to_string(e: EdgeEnd) -> &'static str {
    match e {
        EdgeEnd::None => "none",
        EdgeEnd::Arrow => "arrow",
    }
}

fn parse_edge_end(s: &str) -> EdgeEnd {
    match s {
        "arrow" => EdgeEnd::Arrow,
        _ => EdgeEnd::None,
    }
}
