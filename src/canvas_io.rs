//! JSON Canvas (https://jsoncanvas.org/spec/1.0/) on disk, converted to
//! and from the live [`Canvas`] model.

use std::path::Path;

use anyhow::Result;
use ratatui::layout::Rect;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{Canvas, Color, Edge, EdgeEnd, Node, NodeKind, Shape, Side};

/// A board, in the shape https://jsoncanvas.org/spec/1.0/ describes on
/// disk — also what `Request::State` hands back over `--api`.
#[derive(Debug, Serialize, Deserialize, JsonSchema, Default)]
pub struct FileRoot {
    #[serde(default)]
    pub nodes: Vec<FileNode>,
    #[serde(default)]
    pub edges: Vec<FileEdge>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum FileNode {
    Text {
        id: String,
        x: i64,
        y: i64,
        width: i64,
        height: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        /// Not part of the JSON Canvas spec: "rounded" or "diamond",
        /// omitted for a plain rectangle. An unrecognized reader just
        /// sees an unknown field and renders a normal text node.
        #[serde(skip_serializing_if = "Option::is_none")]
        shape: Option<String>,
        text: String,
    },
    File {
        id: String,
        x: i64,
        y: i64,
        width: i64,
        height: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        file: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        subpath: Option<String>,
    },
    Link {
        id: String,
        x: i64,
        y: i64,
        width: i64,
        height: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        url: String,
    },
    Group {
        id: String,
        x: i64,
        y: i64,
        width: i64,
        height: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        color: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        background: Option<String>,
        #[serde(
            rename = "backgroundStyle",
            skip_serializing_if = "Option::is_none"
        )]
        background_style: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct FileEdge {
    pub id: String,
    #[serde(rename = "fromNode")]
    pub from_node: String,
    #[serde(rename = "fromSide", skip_serializing_if = "Option::is_none")]
    pub from_side: Option<String>,
    #[serde(rename = "fromEnd", skip_serializing_if = "Option::is_none")]
    pub from_end: Option<String>,
    #[serde(rename = "toNode")]
    pub to_node: String,
    #[serde(rename = "toSide", skip_serializing_if = "Option::is_none")]
    pub to_side: Option<String>,
    #[serde(rename = "toEnd", skip_serializing_if = "Option::is_none")]
    pub to_end: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

pub fn load(path: &Path) -> Result<Canvas> {
    let data = std::fs::read_to_string(path)?;
    let root: FileRoot = serde_json::from_str(&data)?;
    Ok(from_file(root))
}

pub fn save(canvas: &Canvas, path: &Path) -> Result<()> {
    let root = to_file(canvas);
    let data = serde_json::to_string_pretty(&root)?;
    std::fs::write(path, data)?;
    Ok(())
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

fn side_to_string(s: Side) -> &'static str {
    match s {
        Side::Top => "top",
        Side::Right => "right",
        Side::Bottom => "bottom",
        Side::Left => "left",
    }
}

fn clamp_u16(v: i64) -> u16 {
    v.clamp(0, u16::MAX as i64) as u16
}

fn file_node_pos(node: &FileNode) -> (i64, i64) {
    match node {
        FileNode::Text { x, y, .. }
        | FileNode::File { x, y, .. }
        | FileNode::Link { x, y, .. }
        | FileNode::Group { x, y, .. } => (*x, *y),
    }
}

fn from_file(root: FileRoot) -> Canvas {
    let mut canvas = Canvas::default();

    // JSON Canvas allows negative coordinates (there is no pan yet on our
    // side), so shift everything into non-negative screen space instead
    // of just clamping each node independently and distorting the layout.
    let mut min_x = 0i64;
    let mut min_y = 0i64;
    for n in &root.nodes {
        let (x, y) = file_node_pos(n);
        min_x = min_x.min(x);
        min_y = min_y.min(y);
    }
    let off_x = -min_x;
    let off_y = -min_y;

    for fnode in root.nodes {
        let (id, x, y, w, h, color, shape, kind) = match fnode {
            FileNode::Text {
                id,
                x,
                y,
                width,
                height,
                color,
                shape,
                text,
            } => (id, x, y, width, height, color, shape, NodeKind::Text(text)),
            FileNode::File {
                id,
                x,
                y,
                width,
                height,
                color,
                file,
                subpath,
            } => (
                id,
                x,
                y,
                width,
                height,
                color,
                None,
                NodeKind::File { path: file, subpath },
            ),
            FileNode::Link {
                id,
                x,
                y,
                width,
                height,
                color,
                url,
            } => (id, x, y, width, height, color, None, NodeKind::Link(url)),
            FileNode::Group {
                id,
                x,
                y,
                width,
                height,
                color,
                label,
                background,
                background_style,
            } => (
                id,
                x,
                y,
                width,
                height,
                color,
                None,
                NodeKind::Group {
                    label,
                    background,
                    background_style,
                },
            ),
        };
        let rect = Rect::new(
            clamp_u16(x + off_x),
            clamp_u16(y + off_y),
            clamp_u16(w).max(1),
            clamp_u16(h).max(1),
        );
        canvas.nodes.push(Node {
            id,
            rect,
            shape: shape.as_deref().map(Shape::parse).unwrap_or_default(),
            color: color.as_deref().map(Color::parse),
            kind,
        });
    }

    for fedge in root.edges {
        canvas.edges.push(Edge {
            id: fedge.id,
            from: fedge.from_node,
            from_side: fedge.from_side.as_deref().and_then(parse_side),
            from_end: if fedge.from_end.as_deref() == Some("arrow") {
                EdgeEnd::Arrow
            } else {
                EdgeEnd::None
            },
            to: fedge.to_node,
            to_side: fedge.to_side.as_deref().and_then(parse_side),
            to_end: if fedge.to_end.as_deref() == Some("none") {
                EdgeEnd::None
            } else {
                EdgeEnd::Arrow
            },
            color: fedge.color.as_deref().map(Color::parse),
            label: fedge.label,
        });
    }

    canvas
}

pub fn to_file(canvas: &Canvas) -> FileRoot {
    let nodes = canvas
        .nodes
        .iter()
        .map(|n| {
            let color = n.color.as_ref().map(Color::to_string);
            let (x, y, width, height) = (
                n.rect.x as i64,
                n.rect.y as i64,
                n.rect.width as i64,
                n.rect.height as i64,
            );
            match &n.kind {
                NodeKind::Text(text) => FileNode::Text {
                    id: n.id.clone(),
                    x,
                    y,
                    width,
                    height,
                    color,
                    shape: n.shape.as_str().map(str::to_string),
                    text: text.clone(),
                },
                NodeKind::File { path, subpath } => FileNode::File {
                    id: n.id.clone(),
                    x,
                    y,
                    width,
                    height,
                    color,
                    file: path.clone(),
                    subpath: subpath.clone(),
                },
                NodeKind::Link(url) => FileNode::Link {
                    id: n.id.clone(),
                    x,
                    y,
                    width,
                    height,
                    color,
                    url: url.clone(),
                },
                NodeKind::Group {
                    label,
                    background,
                    background_style,
                } => FileNode::Group {
                    id: n.id.clone(),
                    x,
                    y,
                    width,
                    height,
                    color,
                    label: label.clone(),
                    background: background.clone(),
                    background_style: background_style.clone(),
                },
            }
        })
        .collect();

    let edges = canvas
        .edges
        .iter()
        .map(|e| FileEdge {
            id: e.id.clone(),
            from_node: e.from.clone(),
            from_side: e.from_side.map(side_to_string).map(str::to_string),
            from_end: (e.from_end == EdgeEnd::Arrow).then(|| "arrow".to_string()),
            to_node: e.to.clone(),
            to_side: e.to_side.map(side_to_string).map(str::to_string),
            to_end: (e.to_end == EdgeEnd::None).then(|| "none".to_string()),
            color: e.color.as_ref().map(Color::to_string),
            label: e.label.clone(),
        })
        .collect();

    FileRoot { nodes, edges }
}
