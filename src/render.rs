use ratatui::{
    Frame,
    layout::Rect,
    style::{Color as RColor, Modifier, Style},
    widgets::{Block, BorderType, Paragraph, Wrap},
};

use crate::app::{App, Corner, Endpoint, HitTarget, Mode, Selected};
use crate::model::{Color, EdgeEnd, Node, NodeKind, Shape, Side};

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

    let reattaching: Option<(String, Endpoint)> = match app.drag.moving() {
        Some(HitTarget::Reattach(id, end)) => Some((id.clone(), *end)),
        _ => None,
    };

    draw_edges(frame, app, live_rect.as_ref(), reattaching.as_ref(), canvas_area);

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
        let target = Selected::Node(node.id.clone());
        let preview = app
            .hover_swatch
            .as_ref()
            .filter(|(t, _)| t == &target)
            .map(|(_, color)| color.as_ref().map(ratatui_color));
        draw_node(frame, node, selected, editing, &app.editing_text, preview);
    }

    if let Some(Selected::Node(id)) = app.selected.clone()
        && let Some(node) = app.canvas.node(&id)
    {
        let target = Selected::Node(id.clone());
        let (bx, by) = (node.rect.right(), node.rect.y);
        let button = Rect::new(bx, by, 1, 1).intersection(canvas_area);
        if !button.is_empty() {
            app.hits.put(button, HitTarget::ColorMenu(target.clone()));
            // Filled with the box's own color, so the button doubles as
            // a preview of what's currently set — not just a dropdown
            // that happens to sit there.
            let dot_color = node.color.as_ref().map(ratatui_color).unwrap_or(RColor::White);
            frame.render_widget(Paragraph::new("●").style(Style::default().fg(dot_color)), button);
        }
        if app.color_picker.as_ref() == Some(&target) {
            draw_color_picker(frame, app, target, bx, by + 1, canvas_area);
        }
    }

    if let Some((id, rect)) = &live_rect {
        let color = app.canvas.node(id).and_then(|n| n.color.as_ref());
        draw_ghost(frame, *rect, color);
    }

    if let Some(rect) = app.drawing_preview() {
        draw_ghost(frame, rect, None);
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

    // Dragging one end of an existing connector: a line from whichever
    // end stayed put to the cursor, same look as drawing a brand new one.
    if let Some((edge_id, end)) = &reattaching
        && let Some((cx, cy)) = app.drag.cursor()
        && let Some(edge) = app.canvas.edge(edge_id)
        && let Some(anchor_id) = match end {
            Endpoint::From => Some(&edge.to),
            Endpoint::To => Some(&edge.from),
        }
        && let Some(anchor) = app.canvas.node(anchor_id)
    {
        let start = center(anchor.rect);
        let end = (cx as i32, cy as i32);
        let path = clipped_path(anchor.rect, anchor.rect, start, end);
        let style = Style::default().fg(RColor::DarkGray);
        draw_path(frame, &path, style);
        if let Some(&(x, y)) = path.last() {
            put_char(frame, x, y, arrow_char(end.0 - start.0, end.1 - start.1), style);
        }
    }

    draw_status(frame, app, status_area);
}

/// `preview`, while a color picker swatch is hovered, is the color it
/// would apply — shown as if it already had, so picking one isn't a
/// guess. `Some(None)` previews clearing the color; `None` (outer)
/// means nothing's hovered, so the node's own color shows as normal.
fn draw_node(frame: &mut Frame, node: &Node, selected: bool, editing: bool, editing_text: &str, preview: Option<Option<RColor>>) {
    let shown = match preview {
        Some(p) => p,
        None => node.color.as_ref().map(ratatui_color),
    };
    let base = shown.map(|c| Style::default().fg(c)).unwrap_or_default();
    // Bold-on-whatever-color-it-already-has is easy to miss, especially
    // on a node with no color set at all — selection stays bold and
    // falls back to its own fixed color only when the node has none,
    // so picking a new color while still selected actually shows it
    // instead of selection's own color masking it.
    let border_style = if selected {
        Style::default().fg(shown.unwrap_or(RColor::Cyan)).add_modifier(Modifier::BOLD)
    } else {
        base
    };

    let mut block = Block::bordered().border_style(border_style);
    if node.shape == Shape::Rounded {
        block = block.border_type(BorderType::Rounded);
    }
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

/// A handful of common colors beyond the six JSON Canvas presets — the
/// whole reason for a picker instead of just cycling `c`, since a hex
/// color is otherwise only reachable from `--api`.
const HEX_SWATCHES: [&str; 6] = ["#ffffff", "#000000", "#808080", "#ff69b4", "#3b82f6", "#8b5a2b"];

/// Two rows below the color menu button: clear + the 6 presets, then 6
/// extra hex swatches. Each swatch is its own hit target, registered
/// fresh every frame like everything else `render` draws.
fn draw_color_picker(frame: &mut Frame, app: &mut App, target: Selected, x: u16, y: u16, canvas_area: Rect) {
    let mut put_swatch = |cx: u16, cy: u16, label: &str, style: Style, color: Option<String>| {
        let rect = Rect::new(cx, cy, 2, 1).intersection(canvas_area);
        if rect.is_empty() {
            return;
        }
        app.hits.put(rect, HitTarget::ColorSwatch(target.clone(), color));
        frame.render_widget(Paragraph::new(label).style(style), rect);
    };

    put_swatch(x, y, "╳ ", Style::default(), None);
    for preset in 1..=6u8 {
        let cx = x + 2 * preset as u16;
        let style = Style::default().bg(ratatui_color(&Color::Preset(preset)));
        put_swatch(cx, y, "  ", style, Some(preset.to_string()));
    }
    for (i, hex) in HEX_SWATCHES.iter().enumerate() {
        let cx = x + 2 * i as u16;
        let style = Style::default().bg(ratatui_color(&Color::Hex((*hex).to_string())));
        put_swatch(cx, y + 1, "  ", style, Some((*hex).to_string()));
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

fn draw_edges(
    frame: &mut Frame,
    app: &mut App,
    live: Option<&(String, Rect)>,
    reattaching: Option<&(String, Endpoint)>,
    canvas_area: Rect,
) {
    let rect_of = |id: &str| -> Option<Rect> {
        live.filter(|(live_id, _)| live_id == id)
            .map(|(_, r)| *r)
            .or_else(|| app.canvas.node(id).map(|n| n.rect))
    };

    // Which side of `from` and of `to` each edge leaves/arrives on, so
    // several edges leaving the same box on the same side can be spread
    // across it instead of all riding the exact midpoint and overlapping
    // for however far they travel together.
    let rects: Vec<Option<(Rect, Rect)>> = app
        .canvas
        .edges
        .iter()
        .map(|e| Some((rect_of(&e.from)?, rect_of(&e.to)?)))
        .collect();
    let sides: Vec<Option<(Side, Side)>> = rects
        .iter()
        .zip(&app.canvas.edges)
        .map(|(r, edge)| {
            let r = (*r)?;
            match (edge.from_side, edge.to_side) {
                (Some(fs), Some(ts)) => Some((fs, ts)),
                _ => Some(sides_for(r.0, r.1)),
            }
        })
        .collect();

    let mut from_frac = vec![0.5f32; app.canvas.edges.len()];
    let mut to_frac = vec![0.5f32; app.canvas.edges.len()];
    let mut from_groups: std::collections::HashMap<(String, Side), Vec<usize>> = std::collections::HashMap::new();
    let mut to_groups: std::collections::HashMap<(String, Side), Vec<usize>> = std::collections::HashMap::new();
    for (i, edge) in app.canvas.edges.iter().enumerate() {
        if let Some((fs, ts)) = sides[i] {
            from_groups.entry((edge.from.clone(), fs)).or_default().push(i);
            to_groups.entry((edge.to.clone(), ts)).or_default().push(i);
        }
    }
    for idxs in from_groups.into_values() {
        let n = idxs.len();
        for (k, i) in idxs.into_iter().enumerate() {
            from_frac[i] = (k + 1) as f32 / (n + 1) as f32;
        }
    }
    for idxs in to_groups.into_values() {
        let n = idxs.len();
        for (k, i) in idxs.into_iter().enumerate() {
            to_frac[i] = (k + 1) as f32 / (n + 1) as f32;
        }
    }

    for i in 0..app.canvas.edges.len() {
        let (color, to_end, from_end, label, edge_id, explicit_sides) = {
            let edge = &app.canvas.edges[i];
            (
                edge.color.clone(),
                edge.to_end,
                edge.from_end,
                edge.label.clone(),
                edge.id.clone(),
                edge.from_side.zip(edge.to_side),
            )
        };
        let Some((from_rect, to_rect)) = rects[i] else { continue };
        if reattaching.is_some_and(|(id, _)| id == &edge_id) {
            continue;
        }
        let target = Selected::Edge(edge_id.clone());
        let selected = app.selected == Some(target.clone());
        let editing = matches!(&app.mode, Mode::Editing(Selected::Edge(id)) if id == &edge_id);
        let preview = app
            .hover_swatch
            .as_ref()
            .filter(|(t, _)| t == &target)
            .map(|(_, c)| c.as_ref().map(ratatui_color));
        let shown_color = match preview {
            Some(p) => p,
            None => color.as_ref().map(ratatui_color),
        };
        let mut style = shown_color.map(|c| Style::default().fg(c)).unwrap_or_default();
        if selected {
            style = Style::default().fg(shown_color.unwrap_or(RColor::Cyan)).add_modifier(Modifier::BOLD);
        }
        let waypoints = route(from_rect, to_rect, from_frac[i], to_frac[i], explicit_sides);
        let glyphs: Vec<(i32, i32, char)> = route_glyphs(&waypoints)
            .into_iter()
            .filter(|&(x, y, _)| !inside(from_rect, x, y) && !inside(to_rect, x, y))
            .collect();

        for &(x, y, ch) in &glyphs {
            put_char(frame, x, y, ch, style);
            app.edge_hits.put(rect_at(x, y), edge_id.clone());
        }
        // Handles for dragging either end loose and re-pointing it —
        // registered after the plain path cells so they win the hit
        // test at their exact spot even though it's also on the line.
        let last = waypoints.len() - 1;
        app.hits.put(rect_at(waypoints[0].0, waypoints[0].1), HitTarget::Reattach(edge_id.clone(), Endpoint::From));
        app.hits.put(rect_at(waypoints[last].0, waypoints[last].1), HitTarget::Reattach(edge_id.clone(), Endpoint::To));

        if to_end == EdgeEnd::Arrow {
            let last = waypoints.len() - 1;
            let (dx, dy) = (waypoints[last].0 - waypoints[last - 1].0, waypoints[last].1 - waypoints[last - 1].1);
            put_char(frame, waypoints[last].0, waypoints[last].1, arrow_char(dx, dy), style);
        }
        if from_end == EdgeEnd::Arrow {
            let (dx, dy) = (waypoints[0].0 - waypoints[1].0, waypoints[0].1 - waypoints[1].1);
            put_char(frame, waypoints[0].0, waypoints[0].1, arrow_char(dx, dy), style);
        }

        let (mx, my) = glyphs.get(glyphs.len() / 2).map(|&(x, y, _)| (x, y)).unwrap_or(waypoints[0]);

        let shown = if editing {
            Some(format!("{}▏", app.editing_text))
        } else {
            label.filter(|l| !l.is_empty())
        };
        if let Some(shown) = shown
            && mx >= 0
            && my >= 0
        {
            let width = shown.chars().count().min(u16::MAX as usize) as u16;
            frame.render_widget(Paragraph::new(shown).style(style), Rect::new(mx as u16, my as u16, width, 1));
        }

        if selected && mx >= 0 && my > 0 {
            let (bx, by) = (mx as u16, my as u16 - 1);
            let button = Rect::new(bx, by, 1, 1).intersection(canvas_area);
            if !button.is_empty() {
                app.hits.put(button, HitTarget::ColorMenu(target.clone()));
                let dot_color = color.as_ref().map(ratatui_color).unwrap_or(RColor::White);
                frame.render_widget(Paragraph::new("●").style(Style::default().fg(dot_color)), button);
            }
            if app.color_picker.as_ref() == Some(&target) {
                // Below the label line (if any), not the button's own
                // row right above it, so a picker never covers either.
                draw_color_picker(frame, app, target.clone(), bx, my as u16 + 1, canvas_area);
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

/// Which side of `from` an edge leaves on and which side of `to` it
/// arrives on — the same three cases [`route`] draws, but usable before
/// any actual coordinate is picked, so several edges sharing a side can
/// be spread across it first.
fn sides_for(from: Rect, to: Rect) -> (Side, Side) {
    let (fx0, fy0, fx1, fy1) = (from.x as i32, from.y as i32, from.right() as i32, from.bottom() as i32);
    let (tx0, ty0, tx1, ty1) = (to.x as i32, to.y as i32, to.right() as i32, to.bottom() as i32);

    let (oy0, oy1) = (fy0.max(ty0), fy1.min(ty1));
    if oy0 < oy1 {
        return if fx0 <= tx0 { (Side::Right, Side::Left) } else { (Side::Left, Side::Right) };
    }
    let (ox0, ox1) = (fx0.max(tx0), fx1.min(tx1));
    if ox0 < ox1 {
        return if fy0 <= ty0 { (Side::Bottom, Side::Top) } else { (Side::Top, Side::Bottom) };
    }
    let (fcx, fcy) = center(from);
    let (tcx, tcy) = center(to);
    let from_side = if tcx > fcx { Side::Right } else { Side::Left };
    let to_side = if tcy > fcy { Side::Top } else { Side::Bottom };
    (from_side, to_side)
}

/// Where a connector attaches on a given side of `rect`, `frac` (0..1)
/// along it — one cell past the border, same convention `route`'s own
/// auto-picked sides already use, so an arrowhead never lands on the
/// border row/column a box's own widget redraws afterward.
fn side_point(rect: Rect, side: Side, frac: f32) -> (i32, i32) {
    let (x0, y0, x1, y1) = (rect.x as i32, rect.y as i32, rect.right() as i32, rect.bottom() as i32);
    match side {
        Side::Right => (x1, y0 + (frac * (y1 - y0 - 1).max(0) as f32).round() as i32),
        Side::Left => (x0 - 1, y0 + (frac * (y1 - y0 - 1).max(0) as f32).round() as i32),
        Side::Bottom => (x0 + (frac * (x1 - x0 - 1).max(0) as f32).round() as i32, y1),
        Side::Top => (x0 + (frac * (x1 - x0 - 1).max(0) as f32).round() as i32, y0 - 1),
    }
}

/// A connector's path when both ends name a specific side explicitly
/// (set through `--api`, or loaded from a file another app wrote) — a
/// straight bend between the two attachment points, on whichever axis
/// fits their exit directions. Simpler than [`route`]'s own geometry
/// (which picks the *shape* of the bend from box positions, not told
/// which sides to use), but every leg still lands orthogonally.
fn route_explicit(from: Rect, to: Rect, from_frac: f32, to_frac: f32, from_side: Side, to_side: Side) -> Vec<(i32, i32)> {
    let e = side_point(from, from_side, from_frac);
    let n = side_point(to, to_side, to_frac);
    let horizontal = |s: Side| matches!(s, Side::Left | Side::Right);
    match (horizontal(from_side), horizontal(to_side)) {
        (true, true) => {
            let mid_x = (e.0 + n.0) / 2;
            vec![e, (mid_x, e.1), (mid_x, n.1), n]
        }
        (false, false) => {
            let mid_y = (e.1 + n.1) / 2;
            vec![e, (e.0, mid_y), (n.0, mid_y), n]
        }
        (true, false) => vec![e, (n.0, e.1), n],
        (false, true) => vec![e, (e.0, n.1), n],
    }
}

/// The corner-to-corner path a connector takes between two boxes: a
/// straight line when they share a row or column, one right-angle bend
/// otherwise — never the diagonal a straight cursor-to-cursor line would
/// draw, which reads as noise rather than a wire between two boxes.
/// `from_frac`/`to_frac` (0..1) place the attachment point along
/// whichever side gets used, so edges sharing a box and a side don't
/// all leave from its exact midpoint and overlap. `sides`, when both
/// ends name one explicitly, routes between exactly those instead of
/// picking automatically from where the boxes happen to sit.
fn route(from: Rect, to: Rect, from_frac: f32, to_frac: f32, sides: Option<(Side, Side)>) -> Vec<(i32, i32)> {
    if let Some((fs, ts)) = sides {
        return route_explicit(from, to, from_frac, to_frac, fs, ts);
    }
    let (fx0, fy0, fx1, fy1) = (from.x as i32, from.y as i32, from.right() as i32, from.bottom() as i32);
    let (tx0, ty0, tx1, ty1) = (to.x as i32, to.y as i32, to.right() as i32, to.bottom() as i32);

    let (oy0, oy1) = (fy0.max(ty0), fy1.min(ty1));
    if oy0 < oy1 {
        let y = oy0 + (from_frac * (oy1 - oy0 - 1).max(0) as f32).round() as i32;
        return if fx0 <= tx0 { vec![(fx1, y), (tx0 - 1, y)] } else { vec![(fx0 - 1, y), (tx1, y)] };
    }

    let (ox0, ox1) = (fx0.max(tx0), fx1.min(tx1));
    if ox0 < ox1 {
        let x = ox0 + (from_frac * (ox1 - ox0 - 1).max(0) as f32).round() as i32;
        return if fy0 <= ty0 { vec![(x, fy1), (x, ty0 - 1)] } else { vec![(x, fy0 - 1), (x, ty1)] };
    }

    let (fcx, fcy) = center(from);
    let (tcx, tcy) = center(to);
    let exit_x = if tcx > fcx { fx1 } else { fx0 - 1 };
    let exit_y = fy0 + (from_frac * (fy1 - fy0 - 1).max(0) as f32).round() as i32;
    let enter_x = tx0 + (to_frac * (tx1 - tx0 - 1).max(0) as f32).round() as i32;
    // One row past `to`'s border, not the border row itself — landing on
    // the border let a box's own bordered widget, drawn right after
    // edges, silently paint over the arrowhead.
    let enter_y = if tcy > fcy { ty0 - 1 } else { ty1 };
    vec![(exit_x, exit_y), (enter_x, exit_y), (enter_x, enter_y)]
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
    let hint = "drag empty space to place · click to select · ● button color picker (box or connector) · dbl-click to edit · drag move · shift+drag connect · corner resize · esc then c color / x shape (or ends, on a connector) / d delete · ctrl+z undo · ctrl+y redo · s save · q/esc quit";
    let line = format!("{mode} — {} — {hint}", app.status);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().fg(RColor::DarkGray)),
        area,
    );
}
