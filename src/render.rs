use ratatui::{
    Frame,
    layout::Rect,
    style::{Color as RColor, Modifier, Style},
    widgets::{Block, Paragraph, Wrap},
};

use crate::app::{App, Corner, HitTarget, Mode};
use crate::model::{Canvas, Color, EdgeEnd, Node, NodeKind};

pub fn render(frame: &mut Frame, app: &mut App, canvas_area: Rect, status_area: Rect) {
    app.hits.clear();
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

    draw_edges(frame, &app.canvas, live_rect.as_ref());

    for node in &app.canvas.nodes {
        if hidden_id.as_deref() == Some(node.id.as_str()) {
            continue;
        }
        app.hits.put(node.rect, HitTarget::Move(node.id.clone()));
        for (corner, rect) in corner_rects(node.rect) {
            app.hits.put(rect, HitTarget::Resize(node.id.clone(), corner));
        }
        let selected = app.selected.as_deref() == Some(node.id.as_str());
        let editing = matches!(&app.mode, Mode::Editing(id) if id == &node.id);
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

fn draw_edges(frame: &mut Frame, canvas: &Canvas, live: Option<&(String, Rect)>) {
    let rect_of = |id: &str| -> Option<Rect> {
        live.filter(|(live_id, _)| live_id == id)
            .map(|(_, r)| *r)
            .or_else(|| canvas.node(id).map(|n| n.rect))
    };
    for edge in &canvas.edges {
        let (Some(from_rect), Some(to_rect)) = (rect_of(&edge.from), rect_of(&edge.to)) else {
            continue;
        };
        let style = edge
            .color
            .as_ref()
            .map(ratatui_color)
            .map(|c| Style::default().fg(c))
            .unwrap_or_default();
        let (fc, tc) = (center(from_rect), center(to_rect));
        let path = clipped_path(from_rect, to_rect, fc, tc);
        draw_path(frame, &path, style);
        if edge.to_end == EdgeEnd::Arrow
            && let Some(&(x, y)) = path.last()
        {
            put_char(frame, x, y, arrow_char(tc.0 - fc.0, tc.1 - fc.1), style);
        }
        if edge.from_end == EdgeEnd::Arrow
            && let Some(&(x, y)) = path.first()
        {
            put_char(frame, x, y, arrow_char(fc.0 - tc.0, fc.1 - tc.1), style);
        }
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
    let hint = "click to place/edit · drag move · shift+drag connect · corner resize · esc then c color / d delete · s save · q quit";
    let line = format!("{mode} — {} — {hint}", app.status);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().fg(RColor::DarkGray)),
        area,
    );
}
