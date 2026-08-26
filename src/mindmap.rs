//! Markdown as a tree: ATX headings and list items become nodes, nested
//! by heading level / list indentation. Headings always break out to
//! heading-level context (a list item can never be a heading's parent),
//! so the only valid ancestor chain is: some headings, then some list
//! items.
//!
//! A node's line range spans from its own line up to (but not
//! including) the next node at the same depth or shallower — so blank
//! lines, prose, and code fences between nodes travel with whichever
//! node they visually belong to, without needing to be parsed at all.
//! A move cuts that whole range out and splices it back in elsewhere;
//! only the lines that actually change depth get rewritten, so an
//! untouched document round-trips byte for byte and a reorder is
//! nothing but a block of lines changing position.

use std::collections::HashMap;
use std::ops::Range;

pub type NodeId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Heading(u8),
    ListItem(char),
}

#[derive(Debug, Clone)]
pub struct MNode {
    pub kind: Kind,
    /// Leading column count; meaningful for list items, always 0 for headings.
    pub indent: u16,
    pub text: String,
    pub line: usize,
    /// Exclusive end of this node's whole subtree, in source lines.
    pub end: usize,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub depth: usize,
}

#[derive(Debug, Default)]
pub struct MindTree {
    pub nodes: HashMap<NodeId, MNode>,
    pub roots: Vec<NodeId>,
}

/// Where a node landed after `resolve_target`: a parent (`None` = the
/// document root) and a position among that parent's children, counted
/// with the moved node already excluded.
pub struct Target {
    pub parent: Option<NodeId>,
    pub index: usize,
}

pub struct Move {
    old_range: Range<usize>,
    insert_at: usize,
    kind: Kind,
    /// Heading: `#`-count delta. ListItem: leading-space delta.
    delta: i32,
}

fn parse_heading(line: &str) -> Option<(u8, String)> {
    let s = line.trim_start();
    let hashes = s.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &s[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some((hashes as u8, rest.trim().to_string()))
}

fn parse_list_item(line: &str) -> Option<(u16, char, String)> {
    let indent = (line.len() - line.trim_start().len()) as u16;
    let s = line.trim_start();
    let bullet = s.chars().next()?;
    if !matches!(bullet, '-' | '*' | '+') {
        return None;
    }
    let rest = &s[1..];
    if !rest.starts_with(' ') {
        return None;
    }
    Some((indent, bullet, rest.trim_start().to_string()))
}

/// Headings sort by level; any list item is deeper than any heading;
/// list items among themselves sort by indent. One tuple order captures
/// all of it.
fn depth_key(kind: Kind, indent: u16) -> (u8, u16) {
    match kind {
        Kind::Heading(level) => (0, level as u16),
        Kind::ListItem(_) => (1, indent),
    }
}

pub fn parse(lines: &[String]) -> MindTree {
    let mut tree = MindTree::default();
    let mut stack: Vec<(NodeId, (u8, u16))> = Vec::new();
    let mut flat: Vec<NodeId> = Vec::new();
    let mut next_id: NodeId = 1;

    for (i, raw) in lines.iter().enumerate() {
        let (kind, indent, text) = if let Some((level, text)) = parse_heading(raw) {
            (Kind::Heading(level), 0u16, text)
        } else if let Some((indent, bullet, text)) = parse_list_item(raw) {
            (Kind::ListItem(bullet), indent, text)
        } else {
            continue;
        };
        let key = depth_key(kind, indent);
        while stack.last().is_some_and(|&(_, k)| k >= key) {
            stack.pop();
        }
        let parent = stack.last().map(|&(id, _)| id);
        let depth = stack.len();
        let id = next_id;
        next_id += 1;
        tree.nodes.insert(
            id,
            MNode {
                kind,
                indent,
                text,
                line: i,
                end: i + 1,
                parent,
                children: Vec::new(),
                depth,
            },
        );
        match parent {
            Some(pid) => tree.nodes.get_mut(&pid).unwrap().children.push(id),
            None => tree.roots.push(id),
        }
        stack.push((id, key));
        flat.push(id);
    }

    for (i, &id) in flat.iter().enumerate() {
        let my_key = depth_key(tree.nodes[&id].kind, tree.nodes[&id].indent);
        let mut end = flat[i + 1..]
            .iter()
            .find(|&&other| depth_key(tree.nodes[&other].kind, tree.nodes[&other].indent) <= my_key)
            .map(|&other| tree.nodes[&other].line)
            .unwrap_or(lines.len());
        // Trailing blank lines are spacing between sections, not part of
        // this node's own content — leave them behind so a move doesn't
        // carry (or strand) the separator before the next node.
        let start = tree.nodes[&id].line;
        while end > start + 1 && lines[end - 1].trim().is_empty() {
            end -= 1;
        }
        tree.nodes.get_mut(&id).unwrap().end = end;
    }

    tree
}

impl MindTree {
    pub fn node(&self, id: NodeId) -> Option<&MNode> {
        self.nodes.get(&id)
    }

    /// Pre-order, skipping `exclude`'s whole subtree — what a drag
    /// leaves in the flow while it's held.
    pub fn flat(&self, exclude: Option<NodeId>) -> Vec<NodeId> {
        fn walk(tree: &MindTree, id: NodeId, exclude: Option<NodeId>, out: &mut Vec<NodeId>) {
            if Some(id) == exclude {
                return;
            }
            out.push(id);
            for &c in &tree.nodes[&id].children {
                walk(tree, c, exclude, out);
            }
        }
        let mut out = Vec::new();
        for &r in &self.roots {
            walk(self, r, exclude, &mut out);
        }
        out
    }

    fn siblings_of(&self, id: NodeId) -> &[NodeId] {
        match self.nodes[&id].parent {
            Some(p) => &self.nodes[&p].children,
            None => &self.roots,
        }
    }

    fn index_in_parent(&self, id: NodeId) -> usize {
        self.siblings_of(id).iter().position(|&x| x == id).unwrap_or(0)
    }

    fn ancestor_at_depth(&self, of: NodeId, depth: usize) -> NodeId {
        let mut cur = of;
        while self.nodes[&cur].depth > depth {
            cur = self.nodes[&cur].parent.expect("depth says an ancestor exists");
        }
        cur
    }

    /// `slot` is a flat-list insertion index (as `sort::slot` measures
    /// it) into the list `flat` came from; this turns it back into a
    /// tree position. Dropping right where a node's first child would
    /// begin makes it that node's new first child; dropping anywhere
    /// else snaps to the depth of whichever neighbor it rejoins.
    pub fn resolve_target(&self, flat: &[NodeId], slot: usize) -> Target {
        let prev = slot.checked_sub(1).and_then(|i| flat.get(i)).copied();
        let next = flat.get(slot).copied();

        let Some(p) = prev else {
            return Target { parent: None, index: 0 };
        };
        let p_depth = self.nodes[&p].depth;

        if let Some(n) = next
            && self.nodes[&n].depth > p_depth
        {
            return Target { parent: Some(p), index: 0 };
        }

        let anchor_depth = next.map(|n| self.nodes[&n].depth).unwrap_or(p_depth);
        let anchor = self.ancestor_at_depth(p, anchor_depth);
        Target {
            parent: self.nodes[&anchor].parent,
            index: self.index_in_parent(anchor) + 1,
        }
    }

    /// A heading can never become a list item's child — that isn't
    /// representable as nested markdown.
    fn valid(&self, moved: NodeId, target: &Target) -> bool {
        if let Kind::Heading(_) = self.nodes[&moved].kind
            && let Some(pid) = target.parent
            && matches!(self.nodes[&pid].kind, Kind::ListItem(_))
        {
            return false;
        }
        true
    }

    fn detect_indent_step(&self) -> u16 {
        self.nodes
            .values()
            .filter_map(|n| {
                let Kind::ListItem(_) = n.kind else { return None };
                let p = &self.nodes[&n.parent?];
                matches!(p.kind, Kind::ListItem(_)).then(|| n.indent.saturating_sub(p.indent))
            })
            .find(|&d| d > 0)
            .unwrap_or(2)
    }

    pub fn plan_move(&self, moved: NodeId, target: &Target) -> Option<Move> {
        if !self.valid(moved, target) {
            return None;
        }
        let node = &self.nodes[&moved];
        let old_range = node.line..node.end;

        let (kind, delta) = match node.kind {
            Kind::Heading(old_level) => {
                let new_level = match target.parent {
                    None => 1,
                    Some(pid) => match self.nodes[&pid].kind {
                        Kind::Heading(l) => (l + 1).min(6),
                        Kind::ListItem(_) => return None,
                    },
                };
                (node.kind, new_level as i32 - old_level as i32)
            }
            Kind::ListItem(_) => {
                let step = self.detect_indent_step() as i32;
                let new_indent = match target.parent {
                    None => 0,
                    Some(pid) => match self.nodes[&pid].kind {
                        Kind::Heading(_) => 0,
                        Kind::ListItem(_) => self.nodes[&pid].indent as i32 + step,
                    },
                };
                (node.kind, new_indent - node.indent as i32)
            }
        };

        let siblings: Vec<NodeId> = match target.parent {
            None => self.roots.iter().copied().filter(|&s| s != moved).collect(),
            Some(pid) => self.nodes[&pid]
                .children
                .iter()
                .copied()
                .filter(|&s| s != moved)
                .collect(),
        };
        // Markdown has no closing tag: a heading's scope ends only where
        // the next same-or-shallower heading begins, so among one
        // parent's children the list items are necessarily a leading
        // run and the sub-headings a trailing one — never interleaved.
        // Landing a moved heading inside that leading run would silently
        // adopt the list items after it; landing a moved list item past
        // it would silently get adopted by whichever heading precedes
        // it. Clamp to the boundary between the two runs instead.
        let list_prefix_len = siblings
            .iter()
            .take_while(|&&s| matches!(self.nodes[&s].kind, Kind::ListItem(_)))
            .count();
        let index = match kind {
            Kind::Heading(_) => target.index.max(list_prefix_len),
            Kind::ListItem(_) => target.index.min(list_prefix_len),
        };
        let insert_at = if let Some(&sibling) = siblings.get(index) {
            self.nodes[&sibling].line
        } else if let Some(pid) = target.parent {
            self.nodes[&pid].end
        } else {
            siblings.last().map(|&s| self.nodes[&s].end).unwrap_or(0)
        };

        Some(Move {
            old_range,
            insert_at,
            kind,
            delta,
        })
    }
}

pub fn apply_move(lines: &[String], mv: &Move) -> Vec<String> {
    let mut out: Vec<String> = lines.to_vec();
    let mut block: Vec<String> = out.splice(mv.old_range.clone(), std::iter::empty()).collect();
    let insert_at = if mv.insert_at > mv.old_range.start {
        mv.insert_at - block.len()
    } else {
        mv.insert_at
    };
    if mv.delta != 0 {
        for line in &mut block {
            match mv.kind {
                Kind::Heading(_) => {
                    if let Some((level, text)) = parse_heading(line) {
                        let new_level = (level as i32 + mv.delta).clamp(1, 6) as usize;
                        *line = format!("{} {}", "#".repeat(new_level), text);
                    }
                }
                Kind::ListItem(_) => {
                    if let Some((indent, bullet, text)) = parse_list_item(line) {
                        let new_indent = (indent as i32 + mv.delta).max(0) as usize;
                        *line = format!("{}{} {}", " ".repeat(new_indent), bullet, text);
                    }
                }
            }
        }
    }
    out.splice(insert_at..insert_at, block);
    out
}

pub fn set_text(lines: &mut [String], tree: &MindTree, id: NodeId, new_text: &str) {
    let Some(node) = tree.node(id) else { return };
    lines[node.line] = match node.kind {
        Kind::Heading(level) => format!("{} {}", "#".repeat(level as usize), new_text),
        Kind::ListItem(bullet) => format!("{}{} {}", " ".repeat(node.indent as usize), bullet, new_text),
    };
}
