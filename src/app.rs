use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui_dnd::{Did, Drag, Hits};

use crate::model::{Canvas, ShapeId};

pub struct App {
    pub canvas: Canvas,
    pub drag: Drag<ShapeId>,
    pub hits: Hits<ShapeId>,
    pub should_quit: bool,
    // Where inside the grabbed box the press landed, so a drop keeps the
    // box under the cursor rather than snapping its corner there.
    grab_offset: (u16, u16),
    // A press that missed every box, in case it turns out to be a plain
    // click on empty canvas rather than the start of a drag.
    press_on_empty: Option<(u16, u16)>,
}

impl App {
    pub fn new() -> Self {
        Self {
            canvas: Canvas::default(),
            drag: Drag::new(),
            hits: Hits::new(),
            should_quit: false,
            grab_offset: (0, 0),
            press_on_empty: None,
        }
    }

    pub fn on_mouse(&mut self, ev: MouseEvent, canvas_area: Rect) {
        let hit = self.hits.at(ev.column, ev.row);

        if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
            match &hit {
                Some((_, rect)) => {
                    self.grab_offset = (
                        ev.column.saturating_sub(rect.x),
                        ev.row.saturating_sub(rect.y),
                    );
                    self.press_on_empty = None;
                }
                None => {
                    self.press_on_empty = canvas_area
                        .contains(Position::new(ev.column, ev.row))
                        .then_some((ev.column, ev.row));
                }
            }
        }

        if let Did::Drop { key, x, y } = self.drag.on_mouse(ev, hit)
            && let Some(b) = self.canvas.get_mut(key)
        {
            let max_x = canvas_area.right().saturating_sub(b.rect.width).max(canvas_area.x);
            let max_y = canvas_area.bottom().saturating_sub(b.rect.height).max(canvas_area.y);
            b.rect.x = x.saturating_sub(self.grab_offset.0).clamp(canvas_area.x, max_x);
            b.rect.y = y.saturating_sub(self.grab_offset.1).clamp(canvas_area.y, max_y);
        }

        if let MouseEventKind::Up(MouseButton::Left) = ev.kind
            && let Some((px, py)) = self.press_on_empty.take()
            && px == ev.column
            && py == ev.row
        {
            self.canvas.place(px, py);
        }
    }
}
