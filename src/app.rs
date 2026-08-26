use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui_dnd::{Did, Drag, Hits};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::canvas_io::{self, FileRoot};
use crate::model::{Canvas, Color, ShapeId};

const MIN_W: u16 = 5;
const MIN_H: u16 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitTarget {
    Move(ShapeId),
    Resize(ShapeId, Corner),
    Connect(ShapeId),
}

impl HitTarget {
    fn shape_id(&self) -> &ShapeId {
        match self {
            HitTarget::Move(id) | HitTarget::Connect(id) | HitTarget::Resize(id, _) => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Editing(ShapeId),
}

/// Every way the board can change. The TUI's mouse and key handlers
/// build one of these and hand it to [`App::dispatch`] just like
/// `--api` does — one path, so a script and a drag can never disagree
/// about what a move means.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Place a new text box, top-left at (x, y).
    Place { x: u16, y: u16 },
    /// Replace a box's text outright.
    SetText { id: ShapeId, text: String },
    /// Move and/or resize a box to an exact rectangle.
    SetRect { id: ShapeId, x: u16, y: u16, w: u16, h: u16 },
    /// A JSON Canvas preset "1".."6", a hex string like "#ff8800", or
    /// null to clear it.
    SetColor { id: ShapeId, color: Option<String> },
    /// Draw an arrow from one box to another.
    Connect { from: ShapeId, to: ShapeId },
    /// Remove a box and any connectors touching it.
    Delete { id: ShapeId },
    /// Select a box, or clear the selection with `null`.
    Select { id: Option<ShapeId> },
    /// The whole board, as JSON Canvas.
    State,
    /// Write the board to the path given on the command line.
    Save,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Placed { id: ShapeId },
    Ok,
    State { board: FileRoot },
    Saved { path: String },
}

pub struct App {
    pub canvas: Canvas,
    pub drag: Drag<HitTarget>,
    pub hits: Hits<HitTarget>,
    pub selected: Option<ShapeId>,
    pub mode: Mode,
    pub editing_text: String,
    pub should_quit: bool,
    pub save_path: Option<PathBuf>,
    pub status: String,
    /// Refreshed by `render` every frame, so a height grown while typing
    /// has something to clamp against without threading it through
    /// `on_key`.
    pub canvas_area: Rect,
    grab_offset: (u16, u16),
    resize_origin: Option<(ShapeId, Corner, Rect)>,
    press_on_empty: Option<(u16, u16)>,
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

        Self {
            canvas,
            drag: Drag::new(),
            hits: Hits::new(),
            selected: None,
            mode: Mode::Normal,
            editing_text: String::new(),
            should_quit: false,
            save_path,
            status,
            canvas_area: Rect::default(),
            grab_offset: (0, 0),
            resize_origin: None,
            press_on_empty: None,
        }
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

    /// The single place every board mutation goes through, whether it
    /// came from a mouse drag, a keystroke, or `--api`.
    pub fn dispatch(&mut self, req: Request) -> Result<Response, String> {
        match req {
            Request::Place { x, y } => {
                let id = self.canvas.place_text(x, y);
                Ok(Response::Placed { id })
            }
            Request::SetText { id, text } => {
                self.canvas.node(&id).ok_or_else(|| format!("no such node: {id}"))?;
                self.canvas.edit_text(&id, |t| {
                    t.clear();
                    t.push_str(&text);
                });
                Ok(Response::Ok)
            }
            Request::SetRect { id, x, y, w, h } => {
                let node = self.canvas.node_mut(&id).ok_or_else(|| format!("no such node: {id}"))?;
                node.rect = Rect::new(x, y, w.max(1), h.max(1));
                Ok(Response::Ok)
            }
            Request::SetColor { id, color } => {
                let node = self.canvas.node_mut(&id).ok_or_else(|| format!("no such node: {id}"))?;
                node.color = color.as_deref().map(Color::parse);
                Ok(Response::Ok)
            }
            Request::Connect { from, to } => {
                self.canvas.node(&from).ok_or_else(|| format!("no such node: {from}"))?;
                self.canvas.node(&to).ok_or_else(|| format!("no such node: {to}"))?;
                self.canvas.connect(from, to);
                Ok(Response::Ok)
            }
            Request::Delete { id } => {
                self.canvas.delete(&id);
                if self.selected.as_deref() == Some(id.as_str()) {
                    self.selected = None;
                }
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
        }
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

    /// Write whatever is in the scratch buffer back to the node being
    /// edited and leave edit mode. Called before any new press is acted
    /// on, so clicking away from an edit-in-progress can never lose it
    /// the way jumping straight to a different mode used to.
    fn commit_edit(&mut self) {
        if let Mode::Editing(id) = self.mode.clone() {
            let text = std::mem::take(&mut self.editing_text);
            let _ = self.dispatch(Request::SetText { id, text });
            self.mode = Mode::Normal;
        }
    }

    fn begin_edit(&mut self, id: ShapeId) {
        self.editing_text = match self.canvas.node(&id).map(|n| &n.kind) {
            Some(crate::model::NodeKind::Text(t)) => t.clone(),
            _ => String::new(),
        };
        self.mode = Mode::Editing(id);
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
                    self.press_on_empty = None;
                }
                Some(_) => self.press_on_empty = None,
                None => {
                    self.press_on_empty = canvas_area
                        .contains(Position::new(ev.column, ev.row))
                        .then_some((ev.column, ev.row));
                }
            }
        }

        match self.drag.on_mouse(ev, hit) {
            Did::Click(target) => {
                let id = target.shape_id().clone();
                let _ = self.dispatch(Request::Select { id: Some(id.clone()) });
                self.begin_edit(id);
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
                let to = self.hits.at(x, y).map(|(t, _)| t.shape_id().clone()).or_else(|| self.nearest_node(x, y));
                match to {
                    Some(to) if to != from => {
                        let _ = self.dispatch(Request::Connect { from, to });
                        self.status.clear();
                    }
                    Some(_) => self.status = "can't connect a box to itself".to_string(),
                    None => self.status = "nothing to connect to".to_string(),
                }
            }
            _ => {}
        }

        if let MouseEventKind::Up(MouseButton::Left) = ev.kind
            && let Some((px, py)) = self.press_on_empty.take()
            && px == ev.column
            && py == ev.row
            && let Ok(Response::Placed { id }) = self.dispatch(Request::Place { x: px, y: py })
        {
            self.selected = Some(id.clone());
            self.begin_edit(id);
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }

        match self.mode.clone() {
            Mode::Editing(id) => match key.code {
                KeyCode::Esc => self.commit_edit(),
                KeyCode::Enter => {
                    self.editing_text.push('\n');
                    self.grow_to_fit(&id);
                }
                KeyCode::Backspace => {
                    self.editing_text.pop();
                }
                KeyCode::Char(c) => {
                    self.editing_text.push(c);
                    self.grow_to_fit(&id);
                }
                _ => {}
            },
            Mode::Normal => match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Char('s') => {
                    let _ = self.dispatch(Request::Save);
                }
                KeyCode::Enter => {
                    if let Some(id) = self.selected.clone() {
                        self.begin_edit(id);
                    }
                }
                KeyCode::Char('c') => {
                    if let Some(id) = self.selected.clone()
                        && let Some(node) = self.canvas.node(&id)
                    {
                        let next = Color::cycle(node.color.as_ref());
                        let _ = self.dispatch(Request::SetColor { id, color: next.map(|c| c.to_string()) });
                    }
                }
                KeyCode::Char('d') | KeyCode::Delete => {
                    if let Some(id) = self.selected.clone() {
                        let _ = self.dispatch(Request::Delete { id });
                    }
                }
                KeyCode::Esc => {
                    let _ = self.dispatch(Request::Select { id: None });
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
