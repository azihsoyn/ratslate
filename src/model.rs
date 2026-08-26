use ratatui::layout::Rect;

pub type ShapeId = u64;

#[derive(Debug, Clone)]
pub struct BoxShape {
    pub id: ShapeId,
    pub rect: Rect,
}

#[derive(Debug, Default)]
pub struct Canvas {
    pub boxes: Vec<BoxShape>,
    next_id: ShapeId,
}

impl Canvas {
    pub fn place(&mut self, x: u16, y: u16) -> ShapeId {
        let id = self.next_id;
        self.next_id += 1;
        self.boxes.push(BoxShape {
            id,
            rect: Rect::new(x, y, 12, 3),
        });
        id
    }

    pub fn get_mut(&mut self, id: ShapeId) -> Option<&mut BoxShape> {
        self.boxes.iter_mut().find(|b| b.id == id)
    }
}
