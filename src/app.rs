use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui_dnd::{Did, Drag, Hits};

use crate::canvas_io;
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

pub struct App {
    pub canvas: Canvas,
    pub drag: Drag<HitTarget>,
    pub hits: Hits<HitTarget>,
    pub selected: Option<ShapeId>,
    pub mode: Mode,
    pub should_quit: bool,
    pub save_path: Option<PathBuf>,
    pub status: String,
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
            should_quit: false,
            save_path,
            status,
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

    pub fn on_mouse(&mut self, ev: MouseEvent, canvas_area: Rect) {
        let mut hit = self.hits.at(ev.column, ev.row);

        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
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
                self.selected = Some(target.shape_id().clone());
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
                if let Some(node) = self.canvas.node_mut(&id) {
                    let max_x = canvas_area
                        .right()
                        .saturating_sub(node.rect.width)
                        .max(canvas_area.x);
                    let max_y = canvas_area
                        .bottom()
                        .saturating_sub(node.rect.height)
                        .max(canvas_area.y);
                    node.rect.x = x.saturating_sub(self.grab_offset.0).clamp(canvas_area.x, max_x);
                    node.rect.y = y.saturating_sub(self.grab_offset.1).clamp(canvas_area.y, max_y);
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
                    && let Some(node) = self.canvas.node_mut(&id)
                {
                    node.rect = resized_rect(origin_rect, corner, x, y, canvas_area);
                }
            }
            Did::Drop {
                key: HitTarget::Connect(from),
                x,
                y,
            } => {
                if let Some((target, _)) = self.hits.at(x, y) {
                    let to = target.shape_id().clone();
                    if to != from {
                        self.canvas.connect(from, to);
                    }
                }
            }
            _ => {}
        }

        if let MouseEventKind::Up(MouseButton::Left) = ev.kind
            && let Some((px, py)) = self.press_on_empty.take()
            && px == ev.column
            && py == ev.row
        {
            let id = self.canvas.place_text(px, py);
            self.selected = Some(id.clone());
            self.mode = Mode::Editing(id);
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }

        match self.mode.clone() {
            Mode::Editing(id) => match key.code {
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Enter => self.canvas.edit_text(&id, |t| t.push('\n')),
                KeyCode::Backspace => self.canvas.edit_text(&id, |t| {
                    t.pop();
                }),
                KeyCode::Char(c) => self.canvas.edit_text(&id, |t| t.push(c)),
                _ => {}
            },
            Mode::Normal => match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Char('s') => self.save(),
                KeyCode::Enter => {
                    if let Some(id) = self.selected.clone() {
                        self.mode = Mode::Editing(id);
                    }
                }
                KeyCode::Char('c') => {
                    if let Some(id) = self.selected.clone()
                        && let Some(node) = self.canvas.node_mut(&id)
                    {
                        node.color = Color::cycle(node.color.as_ref());
                    }
                }
                KeyCode::Char('d') | KeyCode::Delete => {
                    if let Some(id) = self.selected.take() {
                        self.canvas.delete(&id);
                    }
                }
                KeyCode::Esc => self.selected = None,
                _ => {}
            },
        }
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
