use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui_dnd::{Did, Drag, Hits};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::canvas_io::{self, FileRoot};
use crate::collab::{Collab, EdgeFields, NodeFields};
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
    /// The small button next to a selected box or connector that opens
    /// its color picker — a `c` key with no mnemonic beyond "press it a
    /// few times and see", so this is somewhere to actually look at the
    /// choices (hex ones included, which cycling `c` can't reach).
    ColorMenu(Selected),
    /// One swatch in an open color picker. `None` clears the color; a
    /// preset is `Some("1")`..`Some("6")`; anything else is a literal
    /// hex string.
    ColorSwatch(Selected, Option<String>),
    /// A row/column button on an open table editor — `Ctrl`+arrow does
    /// the same thing, but plenty of terminals never forward that
    /// combination at all, so this is the reliable way to reach it.
    TableMenu(ShapeId, TableOp),
    /// One rendered cell of a table box. A click here jumps straight to
    /// that cell instead of always landing on `(0, 0)` — direct if the
    /// table is already open for editing, a double-click (matching any
    /// other box's own open gesture) if it isn't yet.
    TableCell(ShapeId, usize, usize),
}

/// What one of a table's row/column buttons does, always relative to
/// whichever cell is currently open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableOp {
    AddCol,
    DelCol,
    AddRow,
    DelRow,
}

impl HitTarget {
    /// The node this hit belongs to — every variant but `Reattach`
    /// (always an edge) and `ColorMenu`/`ColorSwatch` (either), which
    /// don't drive node-dragging logic and so don't need one.
    fn node_id(&self) -> Option<&ShapeId> {
        match self {
            HitTarget::Move(id) | HitTarget::Connect(id) | HitTarget::Resize(id, _) => Some(id),
            HitTarget::Reattach(..)
            | HitTarget::ColorMenu(_)
            | HitTarget::ColorSwatch(..)
            | HitTarget::TableMenu(..)
            | HitTarget::TableCell(..) => None,
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
    /// A table box's grid, open for editing one cell (row, col) at a
    /// time — `editing_table` holds the whole staged grid, committed
    /// back to the box's text as one GFM table on Esc.
    EditingCell(ShapeId, usize, usize),
}

/// Every way the board can change. The TUI's mouse and key handlers
/// build one of these and hand it to [`App::dispatch`] just like
/// `--api` does — one path, so a script and a drag can never disagree
/// about what a move means.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Place a new text box, top-left at (x, y). `w`/`h` default to the
    /// usual placed size if omitted.
    Place {
        x: u16,
        y: u16,
        #[serde(default)]
        w: Option<u16>,
        #[serde(default)]
        h: Option<u16>,
    },
    /// Replace a box's text outright.
    SetText { id: ShapeId, text: String },
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
    /// Same color grammar as a box's `SetColor`.
    SetEdgeColor { id: String, color: Option<String> },
    /// Which ends carry an arrowhead: "none" or "arrow", for each end.
    SetEdgeEnds { id: String, from_end: String, to_end: String },
    /// Which side of each box a connector leaves from/arrives at —
    /// "top" / "right" / "bottom" / "left", or `null` to go back to
    /// picking automatically based on where the boxes actually sit.
    SetEdgeSides {
        id: String,
        from_side: Option<String>,
        to_side: Option<String>,
    },
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
    /// Which box or connector's color picker is open, if any — always
    /// the selected one, since the button that opens it only renders
    /// next to that.
    pub color_picker: Option<Selected>,
    /// The swatch the cursor is over right now, if any — its color
    /// previews on the actual box or connector while hovered, so
    /// picking one isn't a guess. `Some((target, None))` previews
    /// clearing the color.
    pub hover_swatch: Option<(Selected, Option<Color>)>,
    pub mode: Mode,
    pub editing_text: String,
    /// The whole grid staged for `Mode::EditingCell`, `editing_text`
    /// mirroring whichever one cell is live right now. Empty outside
    /// that mode.
    pub editing_table: crate::table::Table,
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
            color_picker: None,
            hover_swatch: None,
            mode: Mode::Normal,
            editing_text: String::new(),
            editing_table: Vec::new(),
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
            Request::Place { x, y, w, h } => {
                let id = self.canvas.place_text(x, y, w, h);
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
            Request::SetEdgeColor { id, color } => {
                let edge = self.canvas.edge_mut(&id).ok_or_else(|| format!("no such connector: {id}"))?;
                edge.color = color.as_deref().map(Color::parse);
                touched_edge = Some(id);
                Ok(Response::Ok)
            }
            Request::SetEdgeEnds { id, from_end, to_end } => {
                let edge = self.canvas.edge_mut(&id).ok_or_else(|| format!("no such connector: {id}"))?;
                edge.from_end = parse_edge_end(&from_end);
                edge.to_end = parse_edge_end(&to_end);
                touched_edge = Some(id);
                Ok(Response::Ok)
            }
            Request::SetEdgeSides { id, from_side, to_side } => {
                let edge = self.canvas.edge_mut(&id).ok_or_else(|| format!("no such connector: {id}"))?;
                edge.from_side = from_side.as_deref().and_then(parse_side);
                edge.to_side = to_side.as_deref().and_then(parse_side);
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
            Mode::EditingCell(id, row, col) => {
                self.sync_cell(row, col);
                let text = crate::table::format(&self.editing_table);
                self.editing_table = Vec::new();
                let _ = self.dispatch(Request::SetText { id, text });
            }
            Mode::Normal => return,
        }
        self.mode = Mode::Normal;
    }

    fn begin_edit(&mut self, target: Selected) {
        self.editing_text = match &target {
            Selected::Node(id) => match self.canvas.node(id).map(|n| &n.kind) {
                Some(NodeKind::Text(t)) => t.clone(),
                _ => String::new(),
            },
            Selected::Edge(id) => self.canvas.edge(id).and_then(|e| e.label.clone()).unwrap_or_default(),
        };
        self.mode = Mode::Editing(target);
    }

    /// Opens a box's content as a grid — parsed as a GFM table if it
    /// already is one, or a fresh one-header-one-row blank if it isn't
    /// (or is empty), so `t` always lands somewhere editable.
    fn begin_table_edit(&mut self, id: ShapeId) {
        self.begin_table_edit_at(id, 0, 0);
    }

    /// Same as `begin_table_edit`, but opens straight on `(row, col)`
    /// instead of always `(0, 0)` — a double-click on a specific cell of
    /// a table that isn't open for editing yet should land right there.
    fn begin_table_edit_at(&mut self, id: ShapeId, row: usize, col: usize) {
        let current = match self.canvas.node(&id).map(|n| &n.kind) {
            Some(NodeKind::Text(t)) => t.clone(),
            _ => return,
        };
        self.editing_table = crate::table::parse(&current).unwrap_or_else(crate::table::blank);
        let (rows, cols) = self.table_dims();
        let r = row.min(rows.saturating_sub(1));
        let c = col.min(cols.saturating_sub(1));
        self.table_goto(id, r, c);
    }

    fn table_dims(&self) -> (usize, usize) {
        let rows = self.editing_table.len();
        let cols = self.editing_table.first().map(Vec::len).unwrap_or(0);
        (rows, cols)
    }

    fn sync_cell(&mut self, row: usize, col: usize) {
        if let Some(cell) = self.editing_table.get_mut(row).and_then(|r| r.get_mut(col)) {
            *cell = crate::table::encode_break(&std::mem::take(&mut self.editing_text));
        }
    }

    /// Loads `(row, col)`'s content into the scratch buffer and makes
    /// it the live cell — the tail end of every table navigation key.
    fn table_goto(&mut self, id: ShapeId, row: usize, col: usize) {
        let stored = self.editing_table.get(row).and_then(|r| r.get(col)).map(String::as_str).unwrap_or("");
        self.editing_text = crate::table::decode_break(stored);
        self.table_grow_to_fit(&id, row, col);
        self.mode = Mode::EditingCell(id, row, col);
    }

    /// Grows (never shrinks) the box being edited so the grid always
    /// has room for every column and row — checked on every table nav
    /// key, since that's when the row/column count can have changed,
    /// and on every line break typed into the live cell, since that
    /// changes row height without any navigation at all. `editing_text`
    /// isn't written back to `editing_table` until the cell is left, so
    /// this patches a scratch copy the same way the renderer does —
    /// measuring the stale table would leave a growing cell clipped
    /// until the next Tab/Enter synced it.
    fn table_grow_to_fit(&mut self, id: &ShapeId, row: usize, col: usize) {
        let Some(node) = self.canvas.node(id) else { return };
        let mut patched = self.editing_table.clone();
        if let Some(cell) = patched.get_mut(row).and_then(|r| r.get_mut(col)) {
            *cell = crate::table::encode_break(&self.editing_text);
        }
        let (needed_w, needed_h) = crate::table::render_size(&patched);
        let (w, h) = (needed_w.max(node.rect.width), needed_h.max(node.rect.height));
        if w == node.rect.width && h == node.rect.height {
            return;
        }
        let (x, y) = (node.rect.x, node.rect.y);
        let max_w = self.canvas_area.right().saturating_sub(x).max(w);
        let max_h = self.canvas_area.bottom().saturating_sub(y).max(h);
        let _ = self.dispatch(Request::SetRect { id: id.clone(), x, y, w: w.min(max_w), h: h.min(max_h) });
    }

    /// `Tab`/`Shift+Tab`: across, wrapping to the next or previous row.
    /// Tabbing past the last cell of the last row grows the table by
    /// one row — the usual way a spreadsheet keeps a table growing.
    fn table_tab(&mut self, id: ShapeId, row: usize, col: usize, backward: bool) {
        self.sync_cell(row, col);
        let (rows, cols) = self.table_dims();
        let (r, c) = if backward {
            if col == 0 {
                if row == 0 { (row, col) } else { (row - 1, cols.saturating_sub(1)) }
            } else {
                (row, col - 1)
            }
        } else if col + 1 >= cols {
            if row + 1 >= rows {
                self.editing_table.push(vec![String::new(); cols]);
            }
            (row + 1, 0)
        } else {
            (row, col + 1)
        };
        self.table_goto(id, r, c);
    }

    /// `Enter`: down within the same column, growing a new row at the
    /// end the same way `Tab` does — the fast way to fill one column
    /// down a whole table.
    fn table_enter(&mut self, id: ShapeId, row: usize, col: usize) {
        self.sync_cell(row, col);
        let (rows, cols) = self.table_dims();
        if row + 1 >= rows {
            self.editing_table.push(vec![String::new(); cols]);
        }
        self.table_goto(id, row + 1, col);
    }

    /// Arrow keys: plain navigation, clamped to the grid's own bounds —
    /// unlike `Tab`/`Enter`, never grows it.
    fn table_move(&mut self, id: ShapeId, row: usize, col: usize, dr: i32, dc: i32) {
        self.sync_cell(row, col);
        let (rows, cols) = self.table_dims();
        let r = (row as i32 + dr).clamp(0, rows as i32 - 1) as usize;
        let c = (col as i32 + dc).clamp(0, cols as i32 - 1) as usize;
        self.table_goto(id, r, c);
    }

    fn table_insert_col(&mut self, id: ShapeId, row: usize, col: usize) {
        self.sync_cell(row, col);
        for r in &mut self.editing_table {
            r.insert(col + 1, String::new());
        }
        self.table_goto(id, row, col + 1);
    }

    /// A no-op with the grid's last column, rather than emptying the
    /// table entirely — there's always at least one column to type
    /// into.
    fn table_delete_col(&mut self, id: ShapeId, row: usize, col: usize) {
        self.sync_cell(row, col);
        let (_, cols) = self.table_dims();
        if cols > 1 {
            for r in &mut self.editing_table {
                r.remove(col);
            }
        }
        let (_, cols) = self.table_dims();
        self.table_goto(id, row, col.min(cols - 1));
    }

    fn table_insert_row(&mut self, id: ShapeId, row: usize, col: usize) {
        self.sync_cell(row, col);
        let (_, cols) = self.table_dims();
        self.editing_table.insert(row + 1, vec![String::new(); cols]);
        self.table_goto(id, row + 1, col);
    }

    /// A no-op on the header row, or once only the header and one data
    /// row are left — a table always keeps both.
    fn table_delete_row(&mut self, id: ShapeId, row: usize, col: usize) {
        self.sync_cell(row, col);
        let (rows, _) = self.table_dims();
        if row != 0 && rows > 2 {
            self.editing_table.remove(row);
        }
        let (rows, _) = self.table_dims();
        self.table_goto(id, row.min(rows - 1), col);
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
        if let MouseEventKind::Moved = ev.kind {
            self.hover_swatch = match self.hits.at(ev.column, ev.row) {
                Some((HitTarget::ColorSwatch(target, color), _)) => Some((target, color.map(|c| Color::parse(&c)))),
                _ => None,
            };
        }

        let mut hit = self.hits.at(ev.column, ev.row);

        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            // A table's own +/- buttons, and a click on another cell of
            // the very table already open, both act on the edit in
            // progress — committing (and so leaving `EditingCell`)
            // before either click is even handled would make them
            // silently no-op instead of landing where they're aimed.
            let stays_in_same_table = match &hit {
                Some((HitTarget::TableMenu(id, _), _)) | Some((HitTarget::TableCell(id, _, _), _)) => {
                    matches!(&self.mode, Mode::EditingCell(mode_id, ..) if mode_id == id)
                }
                _ => false,
            };
            if !stays_in_same_table {
                self.commit_edit();
            }

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
                self.color_picker = None;
                self.hover_swatch = None;
                let selected = Selected::Edge(edge_id);
                let _ = self.dispatch(Request::Select { id: Some(selected.clone()) });
                if self.is_double_click(selected.clone()) {
                    self.begin_edit(selected);
                }
            }
            Did::Click(HitTarget::ColorMenu(target)) => {
                self.color_picker = if self.color_picker.as_ref() == Some(&target) { None } else { Some(target) };
                self.hover_swatch = None;
            }
            Did::Click(HitTarget::ColorSwatch(target, color)) => {
                let _ = match target {
                    Selected::Node(id) => self.dispatch(Request::SetColor { id, color }),
                    Selected::Edge(id) => self.dispatch(Request::SetEdgeColor { id, color }),
                };
                self.color_picker = None;
                self.hover_swatch = None;
            }
            Did::Click(HitTarget::TableCell(id, row, col)) => {
                self.color_picker = None;
                self.hover_swatch = None;
                if let Mode::EditingCell(mode_id, cur_row, cur_col) = self.mode.clone()
                    && mode_id == id
                {
                    // Already open for editing — a click on any other
                    // cell just jumps the cursor straight there.
                    self.sync_cell(cur_row, cur_col);
                    self.table_goto(id, row, col);
                } else {
                    let selected = Selected::Node(id.clone());
                    let _ = self.dispatch(Request::Select { id: Some(selected.clone()) });
                    if self.is_double_click(selected.clone()) {
                        self.begin_table_edit_at(id, row, col);
                    }
                }
            }
            Did::Click(HitTarget::TableMenu(id, op)) => {
                if let Mode::EditingCell(mode_id, row, col) = self.mode.clone()
                    && mode_id == id
                {
                    match op {
                        TableOp::AddCol => self.table_insert_col(id, row, col),
                        TableOp::DelCol => self.table_delete_col(id, row, col),
                        TableOp::AddRow => self.table_insert_row(id, row, col),
                        TableOp::DelRow => self.table_delete_row(id, row, col),
                    }
                }
            }
            Did::Click(target) => {
                self.color_picker = None;
                self.hover_swatch = None;
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
            } else if let Some((start, end)) = self.drawing.take() {
                if start != end {
                    let x = start.0.min(end.0);
                    let y = start.1.min(end.1);
                    let w = (start.0.max(end.0) - x + 1).max(MIN_W);
                    let h = (start.1.max(end.1) - y + 1).max(MIN_H);
                    if let Ok(Response::Placed { id }) = self.dispatch(Request::Place { x, y, w: Some(w), h: Some(h) }) {
                        let selected = Selected::Node(id);
                        self.selected = Some(selected.clone());
                        self.begin_edit(selected);
                    }
                } else if self.selected.is_some() {
                    // A plain click (no drag) on otherwise-empty canvas —
                    // clicking away to lose focus, the same as Esc does.
                    let _ = self.dispatch(Request::Select { id: None });
                    self.color_picker = None;
                    self.hover_swatch = None;
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
            Mode::EditingCell(id, row, col) => match key.code {
                KeyCode::Esc => self.commit_edit(),
                KeyCode::Tab => self.table_tab(id, row, col, false),
                KeyCode::BackTab => self.table_tab(id, row, col, true),
                // Plain Enter moves down a row, spreadsheet-style — a
                // line break within a cell needs its own key, same
                // reasoning as a connector's one-line label vs a box's
                // own free-form Enter.
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                    self.editing_text.push('\n');
                    self.table_grow_to_fit(&id, row, col);
                }
                KeyCode::Enter => self.table_enter(id, row, col),
                KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => self.table_insert_col(id, row, col),
                KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => self.table_delete_col(id, row, col),
                KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => self.table_insert_row(id, row, col),
                KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => self.table_delete_row(id, row, col),
                KeyCode::Left => self.table_move(id, row, col, 0, -1),
                KeyCode::Right => self.table_move(id, row, col, 0, 1),
                KeyCode::Up => self.table_move(id, row, col, -1, 0),
                KeyCode::Down => self.table_move(id, row, col, 1, 0),
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
                KeyCode::Char('c') => match self.selected.clone() {
                    Some(Selected::Node(id)) => {
                        if let Some(node) = self.canvas.node(&id) {
                            let next = Color::cycle(node.color.as_ref());
                            let _ = self.dispatch(Request::SetColor { id, color: next.map(|c| c.to_string()) });
                        }
                    }
                    Some(Selected::Edge(id)) => {
                        if let Some(edge) = self.canvas.edge(&id) {
                            let next = Color::cycle(edge.color.as_ref());
                            let _ = self.dispatch(Request::SetEdgeColor { id, color: next.map(|c| c.to_string()) });
                        }
                    }
                    None => {}
                },
                KeyCode::Char('x') => match self.selected.clone() {
                    Some(Selected::Node(id)) => {
                        if let Some(node) = self.canvas.node(&id) {
                            let next = node.shape.cycle();
                            let shape = next.as_str().unwrap_or("rectangle").to_string();
                            let _ = self.dispatch(Request::SetShape { id, shape });
                        }
                    }
                    // Cycles which end(s) carry an arrowhead: forward,
                    // both, neither, backward, then around again.
                    Some(Selected::Edge(id)) => {
                        if let Some(edge) = self.canvas.edge(&id) {
                            let (from_end, to_end) = match (edge.from_end, edge.to_end) {
                                (EdgeEnd::None, EdgeEnd::Arrow) => (EdgeEnd::Arrow, EdgeEnd::Arrow),
                                (EdgeEnd::Arrow, EdgeEnd::Arrow) => (EdgeEnd::None, EdgeEnd::None),
                                (EdgeEnd::None, EdgeEnd::None) => (EdgeEnd::Arrow, EdgeEnd::None),
                                _ => (EdgeEnd::None, EdgeEnd::Arrow),
                            };
                            let _ = self.dispatch(Request::SetEdgeEnds {
                                id,
                                from_end: edge_end_to_string(from_end).to_string(),
                                to_end: edge_end_to_string(to_end).to_string(),
                            });
                        }
                    }
                    None => {}
                },
                // Opens the selected box's content as a grid — a GFM
                // table already there, or a fresh blank one otherwise.
                // Still nothing but a text box's own text underneath;
                // any other JSON Canvas reader sees plain markdown.
                KeyCode::Char('t') => {
                    if let Some(Selected::Node(id)) = self.selected.clone() {
                        self.begin_table_edit(id);
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
                    if self.color_picker.take().is_some() {
                        self.hover_swatch = None;
                    } else if self.selected.is_some() {
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
