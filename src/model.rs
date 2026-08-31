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

    /// A JSON Canvas preset "1".."6", or any other string as a literal hex color.
    pub fn parse(s: &str) -> Color {
        match s {
            "1" | "2" | "3" | "4" | "5" | "6" => Color::Preset(s.parse().unwrap()),
            hex => Color::Hex(hex.to_string()),
        }
    }
}

impl std::fmt::Display for Color {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Color::Preset(n) => write!(f, "{n}"),
            Color::Hex(h) => write!(f, "{h}"),
        }
    }
}

/// A box's outline. Not part of JSON Canvas — carried as an extra
/// `shape` field on text nodes that other readers (Obsidian included)
/// just ignore, rather than a property any importer needs to know
/// about to render the node at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
    #[default]
    Rectangle,
    Rounded,
}

impl Shape {
    pub fn cycle(self) -> Shape {
        match self {
            Shape::Rectangle => Shape::Rounded,
            Shape::Rounded => Shape::Rectangle,
        }
    }

    pub fn parse(s: &str) -> Shape {
        match s {
            "rounded" => Shape::Rounded,
            _ => Shape::Rectangle,
        }
    }

    pub fn as_str(self) -> Option<&'static str> {
        match self {
            Shape::Rectangle => None,
            Shape::Rounded => Some("rounded"),
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
    pub shape: Shape,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// Which row and/or column of a table box a connector end lines up
/// with — not part of JSON Canvas at all (it has no notion of a point
/// inside a node), carried as an extra field an unrecognized reader
/// just ignores, landing on that node's ordinary side/midpoint instead.
/// Either field alone anchors to a whole row or column; both together
/// anchor to one cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellAnchor {
    pub row: Option<usize>,
    pub col: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub id: String,
    pub from: ShapeId,
    pub from_side: Option<Side>,
    pub from_end: EdgeEnd,
    pub from_anchor: Option<CellAnchor>,
    pub to: ShapeId,
    pub to_side: Option<Side>,
    pub to_end: EdgeEnd,
    pub to_anchor: Option<CellAnchor>,
    pub color: Option<Color>,
    pub label: Option<String>,
}

/// Nodes are drawn (and hit-tested) in this order, so the last one is on
/// top — the same z-index rule JSON Canvas uses for its `nodes` array.
#[derive(Debug, Default, Clone)]
pub struct Canvas {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// A short id with enough randomness that two boards never mint the same
/// one without having to coordinate. A plain incrementing counter looked
/// fine single-process, but nodes and edges share this same id space and
/// only nodes flow through the CRDT — two independent `--api` calls (or
/// an `--api` call racing the TUI's own local counter) can each pick
/// "the next number" without ever having seen the other's pick, and a
/// collision there quietly merges two unrelated shapes into one. Eight
/// hex digits is short enough to still type by hand and long enough
/// (32 bits) that a collision within any one board's realistic lifetime
/// isn't worth guarding against further; it's also long enough that it
/// can never collide with the old sequential "n12"-style ids an older
/// save file might still contain.
fn fresh_id() -> String {
    use std::io::Read;
    let mut buf = [0u8; 4];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).is_err() {
        buf = (std::process::id() ^ (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()))
            .to_le_bytes();
    }
    format!("n{:08x}", u32::from_le_bytes(buf))
}

impl Canvas {
    pub fn place_text(&mut self, x: u16, y: u16, w: Option<u16>, h: Option<u16>) -> ShapeId {
        let id = fresh_id();
        self.nodes.push(Node {
            id: id.clone(),
            rect: Rect::new(x, y, w.unwrap_or(16), h.unwrap_or(3)),
            color: None,
            shape: Shape::default(),
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

    pub fn connect(&mut self, from: ShapeId, to: ShapeId) -> String {
        let id = fresh_id();
        self.edges.push(Edge {
            id: id.clone(),
            from,
            from_side: None,
            from_end: EdgeEnd::None,
            from_anchor: None,
            to,
            to_side: None,
            to_end: EdgeEnd::Arrow,
            to_anchor: None,
            color: None,
            label: None,
        });
        id
    }

    pub fn delete(&mut self, id: &str) {
        self.nodes.retain(|n| n.id != id);
        self.edges.retain(|e| e.from != id && e.to != id);
    }

    pub fn edge(&self, id: &str) -> Option<&Edge> {
        self.edges.iter().find(|e| e.id == id)
    }

    pub fn edge_mut(&mut self, id: &str) -> Option<&mut Edge> {
        self.edges.iter_mut().find(|e| e.id == id)
    }

    pub fn delete_edge(&mut self, id: &str) {
        self.edges.retain(|e| e.id != id);
    }

    pub fn edit_text(&mut self, id: &str, f: impl FnOnce(&mut String)) {
        if let Some(node) = self.node_mut(id)
            && let NodeKind::Text(text) = &mut node.kind
        {
            f(text);
        }
    }
}
