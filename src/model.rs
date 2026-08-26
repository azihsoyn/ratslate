use ratatui::layout::Rect;

pub type ShapeId = String;

#[derive(Debug, Clone, PartialEq)]
pub enum Color {
    /// One of the six preset colors from the JSON Canvas spec, 1..=6.
    Preset(u8),
    Hex(String),
}

impl Color {
    /// None -> 1 -> 2 -> ... -> 6 -> None.
    pub fn cycle(current: Option<&Color>) -> Option<Color> {
        match current {
            None => Some(Color::Preset(1)),
            Some(Color::Preset(n)) if *n < 6 => Some(Color::Preset(n + 1)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum NodeKind {
    Text(String),
    File {
        path: String,
        subpath: Option<String>,
    },
    Link(String),
    Group {
        label: Option<String>,
        background: Option<String>,
        background_style: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: ShapeId,
    pub rect: Rect,
    pub color: Option<Color>,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeEnd {
    None,
    Arrow,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub id: String,
    pub from: ShapeId,
    pub from_side: Option<Side>,
    pub from_end: EdgeEnd,
    pub to: ShapeId,
    pub to_side: Option<Side>,
    pub to_end: EdgeEnd,
    pub color: Option<Color>,
    pub label: Option<String>,
}

/// Nodes are drawn (and hit-tested) in this order, so the last one is on
/// top — the same z-index rule JSON Canvas uses for its `nodes` array.
#[derive(Debug, Default)]
pub struct Canvas {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    next_id: u64,
}

impl Canvas {
    fn fresh_id(&mut self) -> String {
        self.next_id += 1;
        format!("n{}", self.next_id)
    }

    /// Bumps the id counter past whatever an imported file already used,
    /// so newly placed shapes never collide with it.
    pub fn set_next_id(&mut self, next: u64) {
        if next > self.next_id {
            self.next_id = next;
        }
    }

    pub fn place_text(&mut self, x: u16, y: u16) -> ShapeId {
        let id = self.fresh_id();
        self.nodes.push(Node {
            id: id.clone(),
            rect: Rect::new(x, y, 16, 3),
            color: None,
            kind: NodeKind::Text(String::new()),
        });
        id
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn connect(&mut self, from: ShapeId, to: ShapeId) {
        let id = self.fresh_id();
        self.edges.push(Edge {
            id,
            from,
            from_side: None,
            from_end: EdgeEnd::None,
            to,
            to_side: None,
            to_end: EdgeEnd::Arrow,
            color: None,
            label: None,
        });
    }

    pub fn delete(&mut self, id: &str) {
        self.nodes.retain(|n| n.id != id);
        self.edges.retain(|e| e.from != id && e.to != id);
    }

    pub fn edit_text(&mut self, id: &str, f: impl FnOnce(&mut String)) {
        if let Some(node) = self.node_mut(id)
            && let NodeKind::Text(text) = &mut node.kind
        {
            f(text);
        }
    }
}
