//! Turn a text graph description — a Mermaid flowchart or a Graphviz
//! DOT digraph — into a board. The point is the round trip: an agent
//! or a doc already has the graph as text, so paste it in, nudge it
//! with the mouse, and write it back out as `--render` ASCII or a JSON
//! Canvas. Positions come from the same `layout::layered` pass the `l`
//! key runs, so an import opens already untangled.

use std::collections::HashMap;

use crate::model::{Canvas, EdgeEnd, LineStyle, Shape, ShapeId};

/// A node as the parser finds it, before it becomes a real box — id is
/// the graph's own (`A`, `start`), label is what shows in the box.
struct PNode {
    label: String,
    shape: Shape,
}

/// The two halves of a parse: the ordered node list (graph id + its
/// box) and the edges between them.
type Parsed = (Vec<(String, PNode)>, Vec<PEdge>);

/// A parsed edge: the two graph ids, an optional label, whether it
/// carries an arrowhead, and its line weight.
struct PEdge {
    from: String,
    to: String,
    label: Option<String>,
    arrow: bool,
    style: LineStyle,
}

/// Auto-detects Mermaid vs DOT and parses whichever it is. `Err` with a
/// human-readable reason when the text is neither — the caller prints
/// it and exits, rather than opening an empty board that silently ate
/// the input.
pub fn parse(text: &str) -> Result<Canvas, String> {
    let trimmed = text.trim_start();
    // `graph` starts both a DOT graph and a Mermaid flowchart; the
    // brace on DOT's opening line is what tells them apart.
    let first_has_brace = trimmed.lines().next().map(|l| l.contains('{')).unwrap_or(false);
    let looks_dot = trimmed.starts_with("digraph")
        || trimmed.starts_with("strict")
        || (trimmed.starts_with("graph") && first_has_brace);
    let looks_mermaid = (trimmed.starts_with("graph") || trimmed.starts_with("flowchart")) && !first_has_brace;

    let (nodes, edges) = if looks_dot {
        parse_dot(text)?
    } else if looks_mermaid {
        parse_mermaid(text)?
    } else if text.contains("->") || text.contains("-->") {
        // No header, but it has edges — try DOT's `->` first, then
        // Mermaid's `-->`, so a bare `a -> b` list still imports.
        if text.contains("-->") { parse_mermaid(text)? } else { parse_dot(text)? }
    } else {
        return Err("not a recognizable Mermaid flowchart or DOT graph (no edges found)".to_string());
    };

    if nodes.is_empty() {
        return Err("no nodes found in the graph".to_string());
    }
    Ok(build(nodes, edges))
}

/// Builds the real `Canvas`: a box per node sized to its label, an edge
/// per connection, then one layered-layout pass so it opens tidy. Graph
/// ids map to fresh ratslate ids so two imports never collide.
fn build(pnodes: Vec<(String, PNode)>, pedges: Vec<PEdge>) -> Canvas {
    let mut canvas = Canvas::default();
    let mut id_of: HashMap<String, ShapeId> = HashMap::new();
    for (gid, pn) in pnodes {
        // Box wide enough for the longest line of the label plus a
        // little air, tall enough for its lines — the layout pass only
        // reads these sizes, it doesn't change them.
        let w = pn.label.lines().map(crate::table::display_width).max().unwrap_or(0);
        let width = (w + 4).clamp(8, 60) as u16;
        let height = (pn.label.lines().count().max(1) + 2) as u16;
        let id = canvas.place_text(0, 0, Some(width), Some(height));
        canvas.edit_text(&id, |t| *t = pn.label.clone());
        if let Some(node) = canvas.node_mut(&id) {
            node.shape = pn.shape;
        }
        id_of.insert(gid, id);
    }
    // An edge can name a node the node section never declared (common
    // in Mermaid, where `A --> B` introduces both); make a plain box
    // for any such id, labeled with the id itself.
    let ensure = |canvas: &mut Canvas, id_of: &mut HashMap<String, ShapeId>, gid: &str| -> ShapeId {
        if let Some(id) = id_of.get(gid) {
            return id.clone();
        }
        let width = (crate::table::display_width(gid) + 4).clamp(8, 60) as u16;
        let id = canvas.place_text(0, 0, Some(width), Some(3));
        canvas.edit_text(&id, |t| *t = gid.to_string());
        id_of.insert(gid.to_string(), id.clone());
        id
    };
    for pe in pedges {
        let from = ensure(&mut canvas, &mut id_of, &pe.from);
        let to = ensure(&mut canvas, &mut id_of, &pe.to);
        let eid = canvas.connect(from, to);
        if let Some(edge) = canvas.edge_mut(&eid) {
            edge.to_end = if pe.arrow { EdgeEnd::Arrow } else { EdgeEnd::None };
            edge.style = pe.style;
            edge.label = pe.label;
        }
    }
    for (id, x, y) in crate::layout::layered(&canvas) {
        if let Some(node) = canvas.node_mut(&id) {
            node.rect.x = x;
            node.rect.y = y;
        }
    }
    canvas
}

/// `A[Label]` / `A(Label)` / `A{Label}` — splits a node reference into
/// its id and, if present, its bracketed label and the shape the
/// bracket style implies. A bare `A` is a rectangle labeled `A`.
fn node_ref(s: &str) -> Option<(String, Option<String>, Shape)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // The id runs up to the first bracket.
    let bracket = s.find(['[', '(', '{']);
    let Some(bi) = bracket else {
        // No label: a bare id, as long as it's a plausible identifier.
        if s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.') {
            return Some((s.to_string(), None, Shape::Rectangle));
        }
        return None;
    };
    let id = s[..bi].trim().to_string();
    if id.is_empty() {
        return None;
    }
    let open = &s[bi..bi + 1];
    let (close, shape) = match open {
        "(" => (")", Shape::Rounded),
        "{" => ("}", Shape::Rectangle), // rhombus has no box equivalent
        _ => ("]", Shape::Rectangle),
    };
    // Label is everything between the first open bracket and the last
    // matching close — tolerant of the doubled `[[ ]]`, `([ ])` forms.
    let rest = &s[bi..];
    let inner = rest
        .trim_start_matches(['[', '(', '{'])
        .trim_end_matches([']', ')', '}'])
        .trim_matches('"')
        .trim();
    let _ = close;
    let label = if inner.is_empty() { None } else { Some(inner.to_string()) };
    Some((id, label, shape))
}

/// Mermaid flowchart: a header line (`graph TD`, `flowchart LR`) then
/// edge statements. Handles `-->`, `---`, `==>` and `-.->`, inline edge
/// labels as `-->|text|` or `-- text -->`, and node labels in brackets
/// anywhere a node is first named.
fn parse_mermaid(text: &str) -> Result<Parsed, String> {
    let mut nodes: Vec<(String, PNode)> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut edges: Vec<PEdge> = Vec::new();

    let note = |nodes: &mut Vec<(String, PNode)>, seen: &mut HashMap<String, usize>, id: &str, label: Option<String>, shape: Shape| {
        match seen.get(id) {
            Some(&i) => {
                // A later mention with a label fills one in for a node
                // first seen bare.
                if let Some(l) = label {
                    let pn = &mut nodes[i].1;
                    if pn.label == id {
                        pn.label = l;
                        pn.shape = shape;
                    }
                }
            }
            None => {
                seen.insert(id.to_string(), nodes.len());
                nodes.push((id.to_string(), PNode { label: label.unwrap_or_else(|| id.to_string()), shape }));
            }
        }
    };

    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        // Drop the header and any directive/subgraph lines — a subgraph
        // is flattened, its nodes kept, its grouping dropped.
        let lower = line.to_ascii_lowercase();
        if n == 0 && (lower.starts_with("graph") || lower.starts_with("flowchart")) {
            continue;
        }
        if lower.starts_with("subgraph") || lower == "end" || lower.starts_with("direction") || lower.starts_with("classdef") || lower.starts_with("class ") || lower.starts_with("style ") || lower.starts_with("click ") {
            continue;
        }

        // Find the edge operator. Longest first so `-->` wins over `--`.
        let ops = ["-.->", "==>", "-->", "---", "--"];
        let mut split = None;
        for op in ops {
            if let Some(pos) = line.find(op) {
                split = Some((pos, op));
                break;
            }
        }
        let Some((pos, op)) = split else {
            // A standalone node declaration, e.g. `A[Label]`.
            if let Some((id, label, shape)) = node_ref(line) {
                note(&mut nodes, &mut seen, &id, label, shape);
            }
            continue;
        };

        let left = &line[..pos];
        let mut right = &line[pos + op.len()..];
        let arrow = op != "---" && op != "--";
        let style = if op == "==>" { LineStyle::Thick } else if op == "-.->" { LineStyle::Dashed } else { LineStyle::Solid };

        // Inline edge label: `-->|text|B` or `A -- text --> B`. The
        // pipe form carries the label right after the operator.
        let mut label = None;
        if let Some(stripped) = right.trim_start().strip_prefix('|')
            && let Some(close) = stripped.find('|')
        {
            label = Some(stripped[..close].trim().to_string());
            right = &stripped[close + 1..];
        }

        let Some((lid, llabel, lshape)) = node_ref(left) else { continue };
        let Some((rid, rlabel, rshape)) = node_ref(right) else { continue };
        note(&mut nodes, &mut seen, &lid, llabel, lshape);
        note(&mut nodes, &mut seen, &rid, rlabel, rshape);
        edges.push(PEdge { from: lid, to: rid, label: label.filter(|l| !l.is_empty()), arrow, style });
    }
    Ok((nodes, edges))
}

/// Graphviz DOT: `digraph { A -> B [label="x"]; A [label="Box"]; }`.
/// Handles `->` (and `--`) edges, `[label="…"]` attributes on both
/// nodes and edges, statements split on `;` or newline, and C-style
/// `//` comments.
fn parse_dot(text: &str) -> Result<Parsed, String> {
    // Strip the outer `digraph X { … }` wrapper and comments.
    let mut body = String::new();
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("");
        body.push_str(line);
        body.push('\n');
    }
    let open = body.find('{');
    let inner = match open {
        Some(i) => {
            let rest = &body[i + 1..];
            rest.rfind('}').map(|j| &rest[..j]).unwrap_or(rest)
        }
        None => body.as_str(),
    };

    let mut nodes: Vec<(String, PNode)> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut edges: Vec<PEdge> = Vec::new();

    let note = |nodes: &mut Vec<(String, PNode)>, seen: &mut HashMap<String, usize>, id: &str, label: Option<String>| {
        match seen.get(id) {
            Some(&i) => {
                if let Some(l) = label {
                    nodes[i].1.label = l;
                }
            }
            None => {
                seen.insert(id.to_string(), nodes.len());
                nodes.push((id.to_string(), PNode { label: label.unwrap_or_else(|| id.to_string()), shape: Shape::Rectangle }));
            }
        }
    };

    for stmt in inner.split([';', '\n']) {
        let stmt = stmt.trim();
        if stmt.is_empty() || stmt.starts_with('#') {
            continue;
        }
        let lower = stmt.to_ascii_lowercase();
        if lower.starts_with("rankdir") || lower.starts_with("node ") || lower.starts_with("edge ") || lower.starts_with("graph ") || lower == "}" {
            continue;
        }
        // Pull a trailing `[ ... ]` attribute block off, and find
        // `label="..."` inside it.
        let (head, attr) = match stmt.split_once('[') {
            Some((h, a)) => (h.trim(), Some(a.trim_end_matches(']').trim())),
            None => (stmt, None),
        };
        let label = attr.and_then(dot_label);

        let op = if head.contains("->") { "->" } else if head.contains("--") { "--" } else { "" };
        if op.is_empty() {
            // Node statement.
            let id = unquote(head);
            if !id.is_empty() {
                note(&mut nodes, &mut seen, &id, label);
            }
            continue;
        }
        // Edge, possibly a chain `A -> B -> C`.
        let parts: Vec<String> = head.split(op).map(unquote).filter(|s| !s.is_empty()).collect();
        for pair in parts.windows(2) {
            note(&mut nodes, &mut seen, &pair[0], None);
            note(&mut nodes, &mut seen, &pair[1], None);
            edges.push(PEdge {
                from: pair[0].clone(),
                to: pair[1].clone(),
                label: label.clone().filter(|l| !l.is_empty()),
                arrow: op == "->",
                style: LineStyle::Solid,
            });
        }
    }
    Ok((nodes, edges))
}

/// `label="some text"` out of a DOT attribute list.
fn dot_label(attr: &str) -> Option<String> {
    let i = attr.find("label")?;
    let rest = attr[i + 5..].trim_start().strip_prefix('=')?.trim_start();
    if let Some(q) = rest.strip_prefix('"') {
        q.find('"').map(|end| q[..end].to_string())
    } else {
        // Unquoted label runs to the next comma or end.
        Some(rest.split([',', ']']).next().unwrap_or(rest).trim().to_string())
    }
}

/// Strips surrounding quotes and whitespace from a DOT identifier.
fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(c: &Canvas) -> Vec<String> {
        c.nodes
            .iter()
            .map(|n| match &n.kind {
                crate::model::NodeKind::Text(t) => t.clone(),
                _ => String::new(),
            })
            .collect()
    }

    #[test]
    fn mermaid_nodes_edges_and_shapes() {
        let c = parse("flowchart LR\n  A[parse] --> B[check]\n  B --> C(run)\n").unwrap();
        let mut ls = labels(&c);
        ls.sort();
        assert_eq!(ls, vec!["check", "parse", "run"]);
        assert_eq!(c.edges.len(), 2);
        // `(run)` is the one rounded box.
        assert_eq!(c.nodes.iter().filter(|n| n.shape == Shape::Rounded).count(), 1);
    }

    #[test]
    fn mermaid_edge_label_and_line_styles() {
        let c = parse("graph TD\n  A -->|yes| B\n  A -.-> C\n  A ==> D\n").unwrap();
        assert_eq!(c.edges.len(), 3);
        assert!(c.edges.iter().any(|e| e.label.as_deref() == Some("yes")));
        assert!(c.edges.iter().any(|e| e.style == LineStyle::Dashed));
        assert!(c.edges.iter().any(|e| e.style == LineStyle::Thick));
    }

    #[test]
    fn mermaid_bare_edge_introduces_both_nodes() {
        let c = parse("graph TD\n  start --> finish\n").unwrap();
        assert_eq!(c.nodes.len(), 2);
        assert_eq!(c.edges.len(), 1);
    }

    #[test]
    fn dot_digraph_chain_and_labels() {
        let c = parse("digraph g {\n  rankdir=LR;\n  A [label=\"Fetch\"];\n  A -> B -> C;\n  B -> D [label=\"errs\"];\n}").unwrap();
        let mut ls = labels(&c);
        ls.sort();
        // A is relabeled "Fetch"; B, C, D keep their ids.
        assert_eq!(ls, vec!["B", "C", "D", "Fetch"]);
        assert_eq!(c.edges.len(), 3); // A->B, B->C, B->D
        assert!(c.edges.iter().any(|e| e.label.as_deref() == Some("errs")));
    }

    #[test]
    fn dot_undirected_has_no_arrowheads() {
        let c = parse("graph { A -- B; }").unwrap();
        assert_eq!(c.edges.len(), 1);
        assert_eq!(c.edges[0].to_end, EdgeEnd::None);
    }

    #[test]
    fn rejects_non_graph_text() {
        assert!(parse("just some prose\nwith no edges").is_err());
    }

    #[test]
    fn layout_gives_everyone_a_distinct_spot() {
        let c = parse("graph LR\n A-->B\n A-->C\n B-->D\n C-->D\n").unwrap();
        let spots: std::collections::HashSet<(i32, i32)> = c.nodes.iter().map(|n| (n.rect.x, n.rect.y)).collect();
        assert_eq!(spots.len(), c.nodes.len(), "no two boxes share a cell");
    }
}
