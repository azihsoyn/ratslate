//! JSON Canvas (https://jsoncanvas.org/spec/1.0/) on disk, converted to
//! and from the live [`Canvas`] model.

use std::path::Path;

use anyhow::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{ArrowStyle, Canvas, CellAnchor, Color, Edge, EdgeEnd, LineStyle, Node, NodeKind, NodeStatus, Shape, Side, WorldRect};

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
        /// Not part of the JSON Canvas spec: the border style, omitted
        /// for a plain rectangle. Written as `ratslateShape` like every
        /// other extension; still read under the bare `shape` name
        /// earlier releases wrote. An unrecognized reader just sees an
        /// unknown field and renders a normal text node.
        #[serde(rename = "ratslateShape", alias = "shape", skip_serializing_if = "Option::is_none")]
        shape: Option<String>,
        #[serde(rename = "ratslateStatus", skip_serializing_if = "Option::is_none")]
        status: Option<String>,
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
        #[serde(rename = "ratslateStatus", skip_serializing_if = "Option::is_none")]
        status: Option<String>,
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
        #[serde(rename = "ratslateStatus", skip_serializing_if = "Option::is_none")]
        status: Option<String>,
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

/// Which row and/or column of a table box a connector end lines up
/// with. Not part of the JSON Canvas spec — a `ratslate`-prefixed field
/// so it can never collide with a future official one, and so an
/// unrecognized reader's "ignore what I don't know" behavior is
/// unambiguously the right call rather than an accident of naming.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
pub struct FileAnchor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col: Option<usize>,
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
    #[serde(rename = "ratslateFromAnchor", skip_serializing_if = "Option::is_none")]
    pub from_anchor: Option<FileAnchor>,
    #[serde(rename = "toNode")]
    pub to_node: String,
    #[serde(rename = "toSide", skip_serializing_if = "Option::is_none")]
    pub to_side: Option<String>,
    #[serde(rename = "toEnd", skip_serializing_if = "Option::is_none")]
    pub to_end: Option<String>,
    #[serde(rename = "ratslateToAnchor", skip_serializing_if = "Option::is_none")]
    pub to_anchor: Option<FileAnchor>,
    #[serde(rename = "ratslateStyle", skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(rename = "ratslateArrow", skip_serializing_if = "Option::is_none")]
    pub arrow: Option<String>,
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

fn clamp_i32(v: i64) -> i32 {
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

fn clamp_u16(v: i64) -> u16 {
    v.clamp(0, u16::MAX as i64) as u16
}

fn from_file(root: FileRoot) -> Canvas {
    let mut canvas = Canvas::default();

    for fnode in root.nodes {
        let (id, x, y, w, h, color, shape, status, kind) = match fnode {
            FileNode::Text {
                id,
                x,
                y,
                width,
                height,
                color,
                shape,
                status,
                text,
            } => (id, x, y, width, height, color, shape, status, NodeKind::Text(text)),
            FileNode::File {
                id,
                x,
                y,
                width,
                height,
                color,
                file,
                subpath,
                status,
            } => (
                id,
                x,
                y,
                width,
                height,
                color,
                None,
                status,
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
                status,
            } => (id, x, y, width, height, color, None, status, NodeKind::Link(url)),
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
                None,
                NodeKind::Group {
                    label,
                    background,
                    background_style,
                },
            ),
        };
        // World coordinates kept verbatim — negative and all. The
        // camera decides what's on screen; the model never shifts, so
        // a board authored elsewhere saves back with every coordinate
        // exactly where that tool put it.
        let rect = WorldRect::new(clamp_i32(x), clamp_i32(y), clamp_u16(w).max(1), clamp_u16(h).max(1));
        canvas.nodes.push(Node {
            id,
            rect,
            shape: shape.as_deref().map(Shape::parse).unwrap_or_default(),
            status: status.as_deref().and_then(NodeStatus::parse),
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
            from_anchor: fedge.from_anchor.map(|a| CellAnchor { row: a.row, col: a.col }),
            to: fedge.to_node,
            to_side: fedge.to_side.as_deref().and_then(parse_side),
            to_end: if fedge.to_end.as_deref() == Some("none") {
                EdgeEnd::None
            } else {
                EdgeEnd::Arrow
            },
            to_anchor: fedge.to_anchor.map(|a| CellAnchor { row: a.row, col: a.col }),
            style: fedge.style.as_deref().map(LineStyle::parse).unwrap_or_default(),
            arrow: fedge.arrow.as_deref().map(ArrowStyle::parse).unwrap_or_default(),
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
                    status: n.status.map(|s| s.as_str().to_string()),
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
                    status: n.status.map(|s| s.as_str().to_string()),
                },
                NodeKind::Link(url) => FileNode::Link {
                    id: n.id.clone(),
                    x,
                    y,
                    width,
                    height,
                    color,
                    url: url.clone(),
                    status: n.status.map(|s| s.as_str().to_string()),
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
            from_anchor: e.from_anchor.map(|a| FileAnchor { row: a.row, col: a.col }),
            to_node: e.to.clone(),
            to_side: e.to_side.map(side_to_string).map(str::to_string),
            to_end: (e.to_end == EdgeEnd::None).then(|| "none".to_string()),
            to_anchor: e.to_anchor.map(|a| FileAnchor { row: a.row, col: a.col }),
            style: e.style.as_str().map(str::to_string),
            arrow: e.arrow.as_str().map(str::to_string),
            color: e.color.as_ref().map(Color::to_string),
            label: e.label.clone(),
        })
        .collect();

    FileRoot { nodes, edges }
}

#[cfg(test)]
mod compat {
    //! JSON Canvas compatibility, kept honest by tests: a board another
    //! tool wrote survives a round trip untouched, and everything of
    //! ratslate's own that lands in a file is an extension key other
    //! readers ignore.
    use super::*;
    use serde_json::{Value, json};

    /// The fields JSON Canvas 1.0 defines. Anything else ratslate
    /// writes must carry the `ratslate` prefix.
    const SPEC_NODE: &[&str] =
        &["id", "type", "x", "y", "width", "height", "color", "text", "file", "subpath", "url", "label", "background", "backgroundStyle"];
    const SPEC_EDGE: &[&str] = &["id", "fromNode", "fromSide", "fromEnd", "toNode", "toSide", "toEnd", "color", "label"];

    fn round_trip(v: Value) -> Value {
        let root: FileRoot = serde_json::from_value(v).unwrap();
        serde_json::to_value(to_file(&from_file(root))).unwrap()
    }

    /// `fromEnd`'s default is "none" and `toEnd`'s is "arrow"; ratslate
    /// omits explicit defaults on save, which the spec reads the same.
    fn with_defaults(mut e: Value) -> Value {
        let o = e.as_object_mut().unwrap();
        o.entry("fromEnd").or_insert(json!("none"));
        o.entry("toEnd").or_insert(json!("arrow"));
        e
    }

    #[test]
    fn a_board_another_tool_wrote_round_trips_untouched() {
        let obsidian = json!({
            "nodes": [
                {"id":"a1","type":"text","text":"negative coords","x":-420,"y":-160,"width":200,"height":60},
                {"id":"b2","type":"text","text":"second","x":100,"y":80,"width":250,"height":60,"color":"4"},
                {"id":"g3","type":"group","label":"My Group","x":-440,"y":-180,"width":400,"height":200,"background":"img.png","backgroundStyle":"cover"},
                {"id":"f4","type":"file","file":"notes/spec.md","subpath":"#sec","x":200,"y":-200,"width":180,"height":80},
                {"id":"l5","type":"link","url":"https://jsoncanvas.org","x":-200,"y":200,"width":180,"height":60}
            ],
            "edges": [
                {"id":"e1","fromNode":"a1","fromSide":"right","fromEnd":"none","toNode":"b2","toSide":"left","toEnd":"arrow","color":"2","label":"flows"}
            ]
        });
        let out = round_trip(obsidian.clone());
        assert_eq!(out["nodes"], obsidian["nodes"], "every node field and coordinate comes back as written, in order");
        let edges_in: Vec<Value> = obsidian["edges"].as_array().unwrap().iter().cloned().map(with_defaults).collect();
        let edges_out: Vec<Value> = out["edges"].as_array().unwrap().iter().cloned().map(with_defaults).collect();
        assert_eq!(edges_out, edges_in, "every edge field comes back as written (explicit defaults may be omitted)");
        assert!(!serde_json::to_string(&out).unwrap().contains("ratslate"), "a board with none of ratslate's extras gets none written");
    }

    #[test]
    fn the_old_bare_shape_key_still_reads_and_is_rewritten_prefixed() {
        let old = json!({"nodes":[{"id":"n","type":"text","text":"t","x":0,"y":0,"width":10,"height":3,"shape":"rounded"}],"edges":[]});
        let out = round_trip(old);
        assert_eq!(out["nodes"][0]["ratslateShape"], json!("rounded"));
        assert!(out["nodes"][0].get("shape").is_none(), "the bare key is not written back");
    }

    #[test]
    fn everything_ratslate_adds_to_a_file_is_a_prefixed_extension_key() {
        // Every per-node and per-edge extra ratslate can store, all at once.
        let mut canvas = Canvas::default();
        let a = canvas.place_text(0, 0, Some(20), Some(5));
        let b = canvas.place_text(40, 0, Some(20), Some(5));
        canvas.edit_text(&a, |t| *t = "| h | |\n| --- | --- |\n| r | c |".into());
        {
            let n = canvas.node_mut(&a).unwrap();
            n.shape = Shape::Thick;
            n.status = Some(NodeStatus::Running);
            n.color = Some(Color::Hex("#ffd75f".into()));
        }
        let e = canvas.connect(a.clone(), b.clone());
        {
            let edge = canvas.edge_mut(&e).unwrap();
            edge.style = LineStyle::Double;
            edge.arrow = ArrowStyle::Triangle;
            edge.label = Some("x".into());
            edge.from_anchor = Some(CellAnchor { row: Some(1), col: Some(0) });
            edge.to_anchor = Some(CellAnchor { row: None, col: Some(0) });
            edge.from_side = Some(Side::Right);
            edge.to_side = Some(Side::Left);
        }
        let out = serde_json::to_value(to_file(&canvas)).unwrap();
        let mut offenders = Vec::new();
        for n in out["nodes"].as_array().unwrap() {
            for k in n.as_object().unwrap().keys() {
                if !SPEC_NODE.contains(&k.as_str()) && !k.starts_with("ratslate") {
                    offenders.push(format!("node key {k}"));
                }
            }
        }
        for e in out["edges"].as_array().unwrap() {
            for k in e.as_object().unwrap().keys() {
                if !SPEC_EDGE.contains(&k.as_str()) && !k.starts_with("ratslate") {
                    offenders.push(format!("edge key {k}"));
                }
            }
        }
        assert!(offenders.is_empty(), "non-spec keys must be ratslate-prefixed: {offenders:?}");
        // And the extras really were written (the test isn't vacuous).
        let s = serde_json::to_string(&out).unwrap();
        for k in ["ratslateShape", "ratslateStatus", "ratslateStyle", "ratslateArrow", "ratslateFromAnchor", "ratslateToAnchor"] {
            assert!(s.contains(k), "{k} should be in the file");
        }
    }
}
