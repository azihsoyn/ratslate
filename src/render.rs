use ratatui::{
    Frame,
    layout::Rect,
    style::{Color as RColor, Modifier, Style},
    widgets::{Block, Paragraph, Wrap},
};

use crate::app::{App, Corner, HitTarget, Mode, Selected};
use crate::model::{Color, EdgeEnd, Node, NodeKind};

pub fn render(frame: &mut Frame, app: &mut App, canvas_area: Rect, status_area: Rect) {
    app.hits.clear();
    app.edge_hits.clear();
    app.canvas_area = canvas_area;

    let hidden_id: Option<String> = match app.drag.moving() {
        Some(HitTarget::Move(id)) | Some(HitTarget::Resize(id, _)) => Some(id.clone()),
        _ => None,
    };

    // Where the box being dragged actually is right now, so a
    // connector follows the ghost instead of staying put until the
    // drop lands.
    let live_rect: Option<(String, Rect)> = if let Some((id, rect)) = app.resize_preview(canvas_area) {
        Some((id, rect))
    } else if let Some(HitTarget::Move(id)) = app.drag.moving() {
        app.drag.ghost(canvas_area).map(|g| (id.clone(), g))
    } else {
        None
    };

    draw_edges(frame, app, live_rect.as_ref());

    for node in &app.canvas.nodes {
        if hidden_id.as_deref() == Some(node.id.as_str()) {
            continue;
        }
        app.hits.put(node.rect, HitTarget::Move(node.id.clone()));
        for (corner, rect) in corner_rects(node.rect) {
            app.hits.put(rect, HitTarget::Resize(node.id.clone(), corner));
        }
        let selected = matches!(&app.selected, Some(Selected::Node(id)) if id == &node.id);
        let editing = matches!(&app.mode, Mode::Editing(Selected::Node(id)) if id == &node.id);
        draw_node(frame, node, selected, editing, &app.editing_text);
    }

    if let Some((id, rect)) = &live_rect {
        let color = app.canvas.node(id).and_then(|n| n.color.as_ref());
        draw_ghost(frame, *rect, color);
    }

    if let Some(HitTarget::Connect(from)) = app.drag.moving()
        && let Some((cx, cy)) = app.drag.cursor()
        && let Some(from_node) = app.canvas.node(from)
    {
        let start = center(from_node.rect);
        let end = (cx as i32, cy as i32);
        let path = clipped_path(from_node.rect, from_node.rect, start, end);
        let style = Style::default().fg(RColor::DarkGray);
        draw_path(frame, &path, style);
        if let Some(&(x, y)) = path.last() {
            put_char(frame, x, y, arrow_char(end.0 - start.0, end.1 - start.1), style);
        }
    }

    draw_status(frame, app, status_area);
}

fn draw_node(frame: &mut Frame, node: &Node, selected: bool, editing: bool, editing_text: &str) {
    let base = node
        .color
        .as_ref()
        .map(ratatui_color)
        .map(|c| Style::default().fg(c))
        .unwrap_or_default();
    let border_style = if selected {
        base.add_modifier(Modifier::BOLD)
    } else {
        base
    };
    let block = Block::bordered().border_style(border_style);
    let inner = block.inner(node.rect);
    frame.render_widget(block, node.rect);

    let mut text = if editing { editing_text.to_string() } else { display_text(node) };
    if editing {
        text.push('▏');
    }
    if !text.is_empty() {
        frame.render_widget(
            Paragraph::new(text).style(base).wrap(Wrap { trim: false }),
            inner,
        );
    }
}

fn display_text(node: &Node) -> String {
    match &node.kind {
        NodeKind::Text(t) => t.clone(),
        NodeKind::File { path, .. } => format!("[file] {path}"),
        NodeKind::Link(url) => format!("[link] {url}"),
        NodeKind::Group { label, .. } => match label {
            Some(l) => format!("[group] {l}"),
            None => "[group]".to_string(),
        },
    }
}

fn ratatui_color(color: &Color) -> RColor {
    match color {
        Color::Preset(1) => RColor::Red,
        Color::Preset(2) => RColor::Rgb(255, 140, 0),
        Color::Preset(3) => RColor::Yellow,
        Color::Preset(4) => RColor::Green,
        Color::Preset(5) => RColor::Cyan,
        Color::Preset(6) => RColor::Rgb(160, 32, 240),
        Color::Preset(_) => RColor::Reset,
        Color::Hex(hex) => parse_hex(hex).unwrap_or(RColor::Reset),
    }
}

fn parse_hex(hex: &str) -> Option<RColor> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(RColor::Rgb(r, g, b))
}

fn corner_rects(rect: Rect) -> [(Corner, Rect); 4] {
    [
        (Corner::TopLeft, Rect::new(rect.x, rect.y, 1, 1)),
        (Corner::TopRight, Rect::new(rect.right() - 1, rect.y, 1, 1)),
        (Corner::BottomLeft, Rect::new(rect.x, rect.bottom() - 1, 1, 1)),
        (
            Corner::BottomRight,
            Rect::new(rect.right() - 1, rect.bottom() - 1, 1, 1),
        ),
    ]
}

fn draw_ghost(frame: &mut Frame, rect: Rect, color: Option<&Color>) {
    let style = color
        .map(ratatui_color)
        .map(|c| Style::default().fg(c))
        .unwrap_or(Style::default().fg(RColor::DarkGray));
    frame.render_widget(Block::bordered().border_style(style), rect);
}

fn draw_edges(frame: &mut Frame, app: &mut App, live: Option<&(String, Rect)>) {
    for i in 0..app.canvas.edges.len() {
        let (from_id, to_id, color, to_end, from_end, label, edge_id) = {
            let edge = &app.canvas.edges[i];
            (
                edge.from.clone(),
                edge.to.clone(),
                edge.color.clone(),
                edge.to_end,
                edge.from_end,
                edge.label.clone(),
                edge.id.clone(),
            )
        };
        let rect_of = |id: &str| -> Option<Rect> {
            live.filter(|(live_id, _)| live_id == id)
                .map(|(_, r)| *r)
                .or_else(|| app.canvas.node(id).map(|n| n.rect))
        };
        let (Some(from_rect), Some(to_rect)) = (rect_of(&from_id), rect_of(&to_id)) else {
            continue;
        };
        let selected = app.selected == Some(Selected::Edge(edge_id.clone()));
        let editing = matches!(&app.mode, Mode::Editing(Selected::Edge(id)) if id == &edge_id);
        let mut style = color
            .as_ref()
            .map(ratatui_color)
            .map(|c| Style::default().fg(c))
            .unwrap_or_default();
        if selected {
            style = style.fg(RColor::Cyan).add_modifier(Modifier::BOLD);
        }
        let waypoints = route(from_rect, to_rect);
        let glyphs: Vec<(i32, i32, char)> = route_glyphs(&waypoints)
            .into_iter()
            .filter(|&(x, y, _)| !inside(from_rect, x, y) && !inside(to_rect, x, y))
            .collect();

        for &(x, y, ch) in &glyphs {
            put_char(frame, x, y, ch, style);
            app.edge_hits.put(rect_at(x, y), edge_id.clone());
        }

        if to_end == EdgeEnd::Arrow {
            let last = waypoints.len() - 1;
            let (dx, dy) = (waypoints[last].0 - waypoints[last - 1].0, waypoints[last].1 - waypoints[last - 1].1);
            put_char(frame, waypoints[last].0, waypoints[last].1, arrow_char(dx, dy), style);
        }
        if from_end == EdgeEnd::Arrow {
            let (dx, dy) = (waypoints[0].0 - waypoints[1].0, waypoints[0].1 - waypoints[1].1);
            put_char(frame, waypoints[0].0, waypoints[0].1, arrow_char(dx, dy), style);
        }

        let shown = if editing {
            Some(format!("{}▏", app.editing_text))
        } else {
            label.filter(|l| !l.is_empty())
        };
        if let Some(shown) = shown {
            let (mx, my) = glyphs.get(glyphs.len() / 2).map(|&(x, y, _)| (x, y)).unwrap_or(waypoints[0]);
            if mx >= 0 && my >= 0 {
                let width = shown.chars().count().min(u16::MAX as usize) as u16;
                frame.render_widget(Paragraph::new(shown).style(style), Rect::new(mx as u16, my as u16, width, 1));
            }
        }
    }
}

fn rect_at(x: i32, y: i32) -> Rect {
    if x < 0 || y < 0 || x > u16::MAX as i32 || y > u16::MAX as i32 {
        return Rect::default();
    }
    Rect::new(x as u16, y as u16, 1, 1)
}

/// The corner-to-corner path a connector takes between two boxes: a
/// straight line when they share a row or column, one right-angle bend
/// otherwise — never the diagonal a straight cursor-to-cursor line would
/// draw, which reads as noise rather than a wire between two boxes.
fn route(from: Rect, to: Rect) -> Vec<(i32, i32)> {
    let (fx0, fy0, fx1, fy1) = (from.x as i32, from.y as i32, from.right() as i32, from.bottom() as i32);
    let (tx0, ty0, tx1, ty1) = (to.x as i32, to.y as i32, to.right() as i32, to.bottom() as i32);

    let (oy0, oy1) = (fy0.max(ty0), fy1.min(ty1));
    if oy0 < oy1 {
        let y = (oy0 + oy1 - 1) / 2;
        return if fx0 <= tx0 { vec![(fx1, y), (tx0 - 1, y)] } else { vec![(fx0 - 1, y), (tx1, y)] };
    }

    let (ox0, ox1) = (fx0.max(tx0), fx1.min(tx1));
    if ox0 < ox1 {
        let x = (ox0 + ox1 - 1) / 2;
        return if fy0 <= ty0 { vec![(x, fy1), (x, ty0 - 1)] } else { vec![(x, fy0 - 1), (x, ty1)] };
    }

    let (fcx, fcy) = center(from);
    let (tcx, tcy) = center(to);
    let exit_x = if tcx > fcx { fx1 } else { fx0 - 1 };
    // One row past `to`'s border, not the border row itself — landing on
    // the border let a box's own bordered widget, drawn right after
    // edges, silently paint over the arrowhead.
    let enter_y = if tcy > fcy { ty0 - 1 } else { ty1 };
    vec![(exit_x, fcy), (tcx, fcy), (tcx, enter_y)]
}

/// Walks a waypoint path (each leg strictly horizontal or vertical) and
/// draws it in box-drawing characters, replacing each interior waypoint
/// with the corner glyph its two legs meet at.
fn route_glyphs(waypoints: &[(i32, i32)]) -> Vec<(i32, i32, char)> {
    let seg_char = |a: (i32, i32), b: (i32, i32)| if a.1 == b.1 { '─' } else { '│' };
    let mut out = vec![(waypoints[0].0, waypoints[0].1, seg_char(waypoints[0], waypoints[1]))];
    for w in waypoints.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        let ch = seg_char(w[0], w[1]);
        if x0 == x1 {
            let step = if y1 > y0 { 1 } else { -1 };
            let mut y = y0;
            while y != y1 {
                y += step;
                out.push((x0, y, ch));
            }
        } else {
            let step = if x1 > x0 { 1 } else { -1 };
            let mut x = x0;
            while x != x1 {
                x += step;
                out.push((x, y0, ch));
            }
        }
    }
    for i in 1..waypoints.len() - 1 {
        let ch = corner_char(waypoints[i - 1], waypoints[i], waypoints[i + 1]);
        if let Some(entry) = out.iter_mut().rev().find(|(x, y, _)| (*x, *y) == waypoints[i]) {
            entry.2 = ch;
        }
    }
    out
}

fn corner_char(prev: (i32, i32), corner: (i32, i32), next: (i32, i32)) -> char {
    let dir = |a: (i32, i32), b: (i32, i32)| {
        if a.1 == b.1 {
            if b.0 > a.0 { 'R' } else { 'L' }
        } else if b.1 > a.1 {
            'D'
        } else {
            'U'
        }
    };
    match (dir(prev, corner), dir(corner, next)) {
        ('R', 'D') | ('U', 'L') => '┐',
        ('R', 'U') | ('D', 'L') => '┘',
        ('L', 'D') | ('U', 'R') => '┌',
        ('L', 'U') | ('D', 'R') => '└',
        _ => '+',
    }
}

fn center(rect: Rect) -> (i32, i32) {
    (
        rect.x as i32 + rect.width as i32 / 2,
        rect.y as i32 + rect.height as i32 / 2,
    )
}

fn inside(rect: Rect, x: i32, y: i32) -> bool {
    x >= rect.x as i32
        && x < rect.x as i32 + rect.width as i32
        && y >= rect.y as i32
        && y < rect.y as i32 + rect.height as i32
}

fn clipped_path(from: Rect, to: Rect, start: (i32, i32), end: (i32, i32)) -> Vec<(i32, i32)> {
    line_cells(start.0, start.1, end.0, end.1)
        .into_iter()
        .filter(|&(x, y)| !inside(from, x, y) && !inside(to, x, y))
        .collect()
}

/// Bresenham's line algorithm, cell by cell.
fn line_cells(mut x0: i32, mut y0: i32, x1: i32, y1: i32) -> Vec<(i32, i32)> {
    let mut cells = Vec::new();
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        cells.push((x0, y0));
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
    cells
}

fn arrow_char(dx: i32, dy: i32) -> char {
    if dx.abs() >= dy.abs() {
        if dx >= 0 { '>' } else { '<' }
    } else if dy >= 0 {
        'v'
    } else {
        '^'
    }
}

fn draw_path(frame: &mut Frame, path: &[(i32, i32)], style: Style) {
    for &(x, y) in path {
        put_char(frame, x, y, '·', style);
    }
}

fn put_char(frame: &mut Frame, x: i32, y: i32, ch: char, style: Style) {
    let area = frame.area();
    if x < area.x as i32 || y < area.y as i32 || x >= area.right() as i32 || y >= area.bottom() as i32 {
        return;
    }
    frame.render_widget(
        Paragraph::new(ch.to_string()).style(style),
        Rect::new(x as u16, y as u16, 1, 1),
    );
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let mode = match &app.mode {
        Mode::Normal => "NORMAL",
        Mode::Editing(_) => "EDIT (Esc to leave)",
    };
    let hint = "click box/connector to edit · drag move · shift+drag connect · corner resize · esc then c color / d delete · ctrl+z undo · ctrl+y redo · s save · q/esc quit";
    let line = format!("{mode} — {} — {hint}", app.status);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().fg(RColor::DarkGray)),
        area,
    );
}
