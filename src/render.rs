use ratatui::{
    Frame,
    layout::Rect,
    style::{Color as RColor, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Paragraph, Wrap},
};

use ratatui_dnd::Hits;

use crate::app::{App, Corner, Endpoint, HitTarget, Mode, Selected, TableOp, cell_anchor_for};
use crate::model::{CellAnchor, Color, EdgeEnd, Node, NodeKind, Shape, Side, WorldRect};
use crate::table::Table;

/// World rect → screen space through the camera. Same `WorldRect` type
/// (screen positions can be negative too, for a box hanging off the
/// visible area's left or top), just a different origin.
fn to_screen(rect: WorldRect, camera: (i32, i32), area: Rect) -> WorldRect {
    WorldRect::new(
        rect.x - camera.0 + area.x as i32,
        rect.y - camera.1 + area.y as i32,
        rect.width,
        rect.height,
    )
}

/// The part of a screen-space rect that's actually on the canvas —
/// `None` when none of it is. This is where `i32` screen space becomes
/// the `u16` a widget or a hit target needs.
fn clip(rect: WorldRect, area: Rect) -> Option<Rect> {
    let x0 = rect.x.max(area.x as i32);
    let y0 = rect.y.max(area.y as i32);
    let x1 = rect.right().min(area.right() as i32);
    let y1 = rect.bottom().min(area.bottom() as i32);
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    Some(Rect::new(x0 as u16, y0 as u16, (x1 - x0) as u16, (y1 - y0) as u16))
}

/// Whether a screen-space rect sits entirely on the canvas — the
/// full-content drawing paths need the whole box, not a sliver.
fn fully_visible(rect: WorldRect, area: Rect) -> bool {
    rect.x >= area.x as i32
        && rect.y >= area.y as i32
        && rect.right() <= area.right() as i32
        && rect.bottom() <= area.bottom() as i32
}

pub fn render(frame: &mut Frame, app: &mut App, canvas_area: Rect, status_area: Rect) {
    app.sync_table_cache();
    app.hits.clear();
    app.edge_hits.clear();
    app.canvas_area = canvas_area;

    let hidden_id: Option<String> = match app.drag.moving() {
        Some(HitTarget::Move(id)) | Some(HitTarget::Resize(id, _)) => Some(id.clone()),
        _ => None,
    };

    let camera = app.camera;

    // Where the box being dragged actually is right now (screen
    // space), so a connector follows the ghost instead of staying put
    // until the drop lands.
    let live_rect: Option<(String, WorldRect)> = if let Some((id, rect)) = app.resize_preview() {
        Some((id, to_screen(rect, camera, canvas_area)))
    } else if let Some(HitTarget::Move(id)) = app.drag.moving() {
        app.drag
            .ghost(canvas_area)
            .map(|g| (id.clone(), WorldRect::new(g.x as i32, g.y as i32, g.width, g.height)))
    } else {
        None
    };

    let reattaching: Option<(String, Endpoint)> = match app.drag.moving() {
        Some(HitTarget::Reattach(id, end)) => Some((id.clone(), *end)),
        _ => None,
    };

    // The hovered cell's own candidate anchor points, dim `○` — just
    // its column (top and bottom) and its row (left and right), not
    // every row and column of the table, so a big table doesn't ring
    // itself in dots. Visible while the cursor is on that cell,
    // hovering plainly or mid-drag aiming at it as the destination.
    // Drawn before `draw_edges` so an end actually anchored at one of
    // these spots shows its own `●` on top instead.
    if let Some((id, hovered)) = app.hover_cell.clone()
        && let Some(node) = app.canvas.node(&id)
        && let Some(table) = app.table_cache.get(&id).and_then(|(_, t)| t.clone())
    {
        let srect = to_screen(node.rect, camera, canvas_area);
        let dim = Style::default().fg(RColor::DarkGray);
        if let Some(c) = hovered.col
            && let Some(frac) = crate::table::col_center_frac(&table, c, srect.width)
        {
            for side in [Side::Top, Side::Bottom] {
                let (x, y) = side_point(srect, side, frac);
                put_char(frame, x, y, '○', dim);
            }
        }
        if let Some(r) = hovered.row
            && let Some(frac) = crate::table::row_center_frac(&table, r, srect.height)
        {
            for side in [Side::Left, Side::Right] {
                let (x, y) = side_point(srect, side, frac);
                put_char(frame, x, y, '○', dim);
            }
        }
    }

    draw_edges(frame, app, live_rect.as_ref(), reattaching.as_ref(), canvas_area);

    for node in &app.canvas.nodes {
        if hidden_id.as_deref() == Some(node.id.as_str()) {
            continue;
        }
        let srect = to_screen(node.rect, camera, canvas_area);
        let Some(clipped) = clip(srect, canvas_area) else { continue };
        app.hits.put(clipped, HitTarget::Move(node.id.clone()));
        // A box only partly on screen draws as its clipped frame — no
        // borders-at-the-wrong-place content layout to get subtly
        // wrong, and the `Move` hit above is still enough to grab it
        // and drag it back into view.
        if !fully_visible(srect, canvas_area) {
            frame.render_widget(Block::bordered(), clipped);
            continue;
        }
        for (corner, rect) in corner_rects(clipped) {
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

        let table_cursor = match &app.mode {
            Mode::EditingCell(id, r, c) if id == &node.id => Some((*r, *c)),
            _ => None,
        };
        if table_cursor.is_some() {
            let view = TableView { node, rect: clipped, selected, table: &app.editing_table, cursor: table_cursor, editing_text: &app.editing_text, preview };
            draw_table_node(frame, view, &mut app.hits);
        } else if let Some(table) = app.table_cache.get(&node.id).and_then(|(_, t)| t.clone()) {
            let view = TableView { node, rect: clipped, selected, table: &table, cursor: None, editing_text: "", preview };
            draw_table_node(frame, view, &mut app.hits);
        } else {
            draw_node(frame, node, clipped, selected, editing, &app.editing_text, preview);
        }
    }

    if let Mode::EditingCell(id, _, _) = app.mode.clone()
        && let Some(node) = app.canvas.node(&id)
        && let Some(clipped) = clip(to_screen(node.rect, camera, canvas_area), canvas_area)
    {
        draw_table_menu(frame, app, &id, clipped, canvas_area);
    }

    if let Some(Selected::Node(id)) = app.selected.clone()
        && let Some(node) = app.canvas.node(&id)
    {
        let srect = to_screen(node.rect, camera, canvas_area);
        let target = Selected::Node(id.clone());
        let (bx, by) = (srect.right(), srect.y);
        if bx >= 0
            && by >= 0
            && let button = Rect::new(bx as u16, by as u16, 1, 1).intersection(canvas_area)
            && !button.is_empty()
        {
            app.hits.put(button, HitTarget::ColorMenu(target.clone()));
            // Filled with the box's own color, so the button doubles as
            // a preview of what's currently set — not just a dropdown
            // that happens to sit there.
            let dot_color = node.color.as_ref().map(ratatui_color).unwrap_or(RColor::White);
            frame.render_widget(Paragraph::new("●").style(Style::default().fg(dot_color)), button);
            if app.color_picker.as_ref() == Some(&target) {
                draw_color_picker(frame, app, target, bx as u16, by as u16 + 1, canvas_area);
            }
        }
    }

    if let Some((id, rect)) = &live_rect
        && let Some(clipped) = clip(*rect, canvas_area)
    {
        let color = app.canvas.node(id).and_then(|n| n.color.as_ref());
        draw_ghost(frame, clipped, color);
    }

    if let Some(rect) = app.drawing_preview() {
        draw_ghost(frame, rect, None);
    }

    if let Some(HitTarget::Connect(from, anchor)) = app.drag.moving()
        && let Some((cx, cy)) = app.drag.cursor()
        && let Some(from_node) = app.canvas.node(from)
    {
        // The cursor hovering another box's own table cell or dot
        // previews the far end's anchor too — aiming at a destination
        // row/column should be visible before the drop that commits it.
        let hover_target = match app.hits.at(cx, cy) {
            Some((HitTarget::TableCell(id, row, col), _)) if &id != from => {
                app.canvas.node(&id).map(|n| (n, to_screen(n.rect, camera, canvas_area), Some(cell_anchor_for(row, col))))
            }
            Some((HitTarget::AnchorDot(id, dot_anchor), _)) if &id != from => {
                app.canvas.node(&id).map(|n| (n, to_screen(n.rect, camera, canvas_area), Some(dot_anchor)))
            }
            Some((target, _)) => target
                .node_id()
                .filter(|id| *id != from)
                .and_then(|id| app.canvas.node(id))
                .map(|n| (n, to_screen(n.rect, camera, canvas_area), None)),
            None => None,
        };
        let from_srect = to_screen(from_node.rect, camera, canvas_area);
        draw_drag_preview(frame, (from_node, from_srect), *anchor, (cx, cy), hover_target);
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
        && let Some(anchor_node) = app.canvas.node(anchor_id)
    {
        let srect = to_screen(anchor_node.rect, camera, canvas_area);
        draw_drag_preview(frame, (anchor_node, srect), None, (cx, cy), None);
    }

    draw_minimap(frame, app);

    draw_status(frame, app, status_area);
}

/// A small overlay in the canvas's bottom-right corner: every box as a
/// dot (in its own color) over the whole board's extent, with the
/// current viewport outlined — the way back to content that's panned
/// out of sight. Clicking (or dragging) anywhere on it centers the
/// view there; `m` toggles it. Drawn last so it sits over everything,
/// and its hit target registered last for the same reason: later wins,
/// so a click on the map is never a click on whatever it covers.
fn draw_minimap(frame: &mut Frame, app: &mut App) {
    // While a scrub-drag is in progress, draw against the same frozen
    // frame of reference the drag math uses — recomputing bounds every
    // frame would slide the map under the very cursor dragging it.
    let Some(layout) = app.minimap_drag.clone().or_else(|| app.minimap_layout()) else { return };

    frame.render_widget(Clear, layout.area);
    let block = Block::bordered()
        .border_style(Style::default().fg(RColor::DarkGray))
        .title("map");
    frame.render_widget(block, layout.area);

    // The viewport first, as a dim filled region, so the node dots
    // paint over it and stay legible inside it.
    let (vx0, vy0) = layout.to_map(app.camera.0, app.camera.1);
    let (vx1, vy1) = layout.to_map(
        app.camera.0 + app.canvas_area.width as i32,
        app.camera.1 + app.canvas_area.height as i32,
    );
    let view = Rect::new(vx0, vy0, (vx1 - vx0 + 1).max(1), (vy1 - vy0 + 1).max(1));
    frame.render_widget(Block::new().style(Style::default().bg(RColor::Rgb(60, 60, 70))), view);

    // The box being dragged or resized right now, at where its ghost
    // is rather than where the model still says it sits — the model
    // only updates on drop, and a map dot that sat still through the
    // whole drag then teleported read as the map not updating at all.
    let live: Option<(String, (i32, i32))> = match app.drag.moving() {
        Some(HitTarget::Move(id)) => app.drag.ghost(app.canvas_area).map(|g| {
            let (wx, wy) = app.to_world(g.x, g.y);
            (id.clone(), (wx + g.width as i32 / 2, wy + g.height as i32 / 2))
        }),
        Some(HitTarget::Resize(..)) => app
            .resize_preview()
            .map(|(id, r)| (id, (r.x + r.width as i32 / 2, r.y + r.height as i32 / 2))),
        _ => None,
    };

    // One dot per box, at its center — drawing each box's scaled
    // extent instead made a single wide box read as a row of separate
    // boxes, which is worse than not showing sizes at all.
    for node in &app.canvas.nodes {
        let (cx, cy) = match &live {
            Some((id, center)) if id == &node.id => *center,
            _ => (node.rect.x + node.rect.width as i32 / 2, node.rect.y + node.rect.height as i32 / 2),
        };
        let (mx, my) = layout.to_map(cx, cy);
        let color = node.color.as_ref().map(ratatui_color).unwrap_or(RColor::Gray);
        put_char(frame, mx as i32, my as i32, '▪', Style::default().fg(color));
    }

    app.hits.put(layout.area, HitTarget::Minimap);
}

/// `preview`, while a color picker swatch is hovered, is the color it
/// would apply — shown as if it already had, so picking one isn't a
/// guess. `Some(None)` previews clearing the color; `None` (outer)
/// means nothing's hovered, so the node's own color shows as normal.
fn node_style(color: Option<&Color>, selected: bool, preview: Option<Option<RColor>>) -> (Style, Style) {
    let shown = match preview {
        Some(p) => p,
        None => color.map(ratatui_color),
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
    (base, border_style)
}

/// `rect` is the node's place on screen, already translated through
/// the camera — the node's own `rect` is world coordinates and never
/// drawn from directly.
fn draw_node(frame: &mut Frame, node: &Node, rect: Rect, selected: bool, editing: bool, editing_text: &str, preview: Option<Option<RColor>>) {
    let (base, border_style) = node_style(node.color.as_ref(), selected, preview);

    let mut block = Block::bordered().border_style(border_style);
    if node.shape == Shape::Rounded {
        block = block.border_type(BorderType::Rounded);
    }
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

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

/// Everything `draw_table_node` needs about the box itself, bundled so
/// the function doesn't outgrow clippy's own argument-count lint —
/// `hits` stays a separate parameter since it's mutated, not read.
struct TableView<'a> {
    node: &'a Node,
    /// The node's place on screen, already translated through the
    /// camera and known to be fully visible.
    rect: Rect,
    selected: bool,
    table: &'a Table,
    cursor: Option<(usize, usize)>,
    editing_text: &'a str,
    preview: Option<Option<RColor>>,
}

/// A box whose content parses as a GFM table, drawn as an actual grid
/// instead of raw `| a | b |` text. `cursor`, while this box is the
/// one open in `Mode::EditingCell`, is the live cell — shown reversed,
/// with `editing_text` (not the table's own stale copy) as its content.
fn draw_table_node(frame: &mut Frame, view: TableView, hits: &mut Hits<HitTarget>) {
    let TableView { node, rect, selected, table, cursor, editing_text, preview } = view;
    let (base, border_style) = node_style(node.color.as_ref(), selected, preview);

    let mut block = Block::bordered().border_style(border_style);
    if node.shape == Shape::Rounded {
        block = block.border_type(BorderType::Rounded);
    }
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    // The cell actually being typed into lives in `editing_text`, not
    // yet written back to `table` (that only happens on navigating away
    // from it) — patch it in before measuring column widths, or the
    // column showing it renders one keystroke behind its own content.
    // Re-encoded to `<br>` on the way in so every cell in the patched
    // table agrees on how a line break is spelled, live multi-line
    // typing included.
    let mut patched;
    let table: &Table = if let Some((cr, cc)) = cursor {
        patched = table.clone();
        if let Some(cell) = patched.get_mut(cr).and_then(|r| r.get_mut(cc)) {
            *cell = crate::table::encode_break(editing_text);
        }
        &patched
    } else {
        table
    };

    let widths = crate::table::col_widths(table);

    // The x of each internal column divider — a plain `Block` has no
    // idea a divider is about to touch its border, so left alone the
    // join reads as a gap (the border's own `─`/`│` glyph doesn't
    // connect to the grid's) rather than a proper `┬`/`┴`/`├`/`┤`
    // junction. Patched into the buffer once the whole grid is drawn.
    // Bounded to `inner` — the box may not have grown to fit the full
    // table yet (auto-grow only runs from interactive navigation), and
    // an unclipped divider position would poke a junction glyph into
    // whatever the box's content is already clipped away from, well
    // outside its own border.
    let divider_xs: Vec<u16> = {
        let mut xs = Vec::new();
        let mut cx = inner.x;
        for (c, &w) in widths.iter().enumerate() {
            if c > 0 {
                if cx >= inner.right() {
                    break;
                }
                xs.push(cx);
                cx += 1;
            }
            cx += w as u16 + 2;
        }
        xs
    };

    let mut y = inner.y;
    let mut header_sep_y = None;
    for (r, row) in table.iter().enumerate() {
        if y >= inner.bottom() {
            break;
        }
        // A row is as tall as its tallest cell — most rows are one
        // line, but a cell with its own line break within it stretches
        // every other cell in the row to match.
        let cols_lines: Vec<Vec<&str>> = widths
            .iter()
            .enumerate()
            .map(|(c, _)| crate::table::cell_lines(row.get(c).map(String::as_str).unwrap_or("")))
            .collect();
        let row_h = cols_lines.iter().map(Vec::len).max().unwrap_or(1).max(1);

        let row_top = y;
        for line_idx in 0..row_h {
            if y >= inner.bottom() {
                break;
            }
            let mut spans = Vec::new();
            for (c, &w) in widths.iter().enumerate() {
                if c > 0 {
                    spans.push(Span::styled("│", base));
                }
                let is_cursor = cursor == Some((r, c));
                let lines = &cols_lines[c];
                let content = lines.get(line_idx).copied().unwrap_or("");
                // One space of breathing room on each side of the
                // content, not just a bare column butted up against the
                // divider — `col_widths`/`render_size` already size the
                // box assuming this padding is here.
                let mut text = format!(" {content:<w$}");
                if is_cursor && line_idx + 1 == lines.len().max(1) {
                    text.push('▏');
                } else {
                    text.push(' ');
                }
                let mut cell_style = base;
                if r == 0 {
                    cell_style = cell_style.add_modifier(Modifier::BOLD);
                }
                if is_cursor {
                    cell_style = cell_style.add_modifier(Modifier::REVERSED);
                }
                spans.push(Span::styled(text, cell_style));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), Rect::new(inner.x, y, inner.width, 1));
            y += 1;
        }
        // One hit rect per cell spanning its whole row height, not one
        // per display line — a click anywhere in a multi-line cell
        // should still land on it.
        let mut cx = inner.x;
        for (c, &w) in widths.iter().enumerate() {
            if c > 0 {
                cx += 1;
            }
            let cell_w = w as u16 + 2;
            let cell_rect = Rect::new(cx, row_top, cell_w.min(inner.right().saturating_sub(cx)), y.saturating_sub(row_top).min(inner.bottom().saturating_sub(row_top)));
            if !cell_rect.is_empty() {
                hits.put(cell_rect, HitTarget::TableCell(node.id.clone(), r, c));
            }
            cx += cell_w;
        }
        // One grid line, right under the header — the column dividers
        // already run the full height, so a line between every data
        // row just reads as noise rather than making anything clearer.
        // Padded to match the cells' own padded width, or the divider
        // `┼`s would land one column off from the `│`s above and below
        // them.
        if y < inner.bottom() && r == 0 && table.len() > 1 {
            let sep: String = widths
                .iter()
                .enumerate()
                .map(|(i, &w)| if i == 0 { "─".repeat(w + 2) } else { format!("┼{}", "─".repeat(w + 2)) })
                .collect();
            frame.render_widget(Paragraph::new(sep).style(base), Rect::new(inner.x, y, inner.width, 1));
            header_sep_y = Some(y);
            y += 1;
        }
    }

    let buf = frame.buffer_mut();
    for &dx in &divider_xs {
        if let Some(cell) = buf.cell_mut((dx, rect.y)) {
            cell.set_symbol("┬");
        }
    }
    // The bottom border only gets `┴` junctions when the grid actually
    // reaches it — `table_grow_to_fit` never shrinks the box, so a
    // taller-than-needed one leaves blank rows below the last divider,
    // and a junction there would float disconnected from any line.
    if y >= inner.bottom() {
        for &dx in &divider_xs {
            if rect.bottom() > rect.y
                && let Some(cell) = buf.cell_mut((dx, rect.bottom() - 1))
            {
                cell.set_symbol("┴");
            }
        }
    }
    if let Some(sep_y) = header_sep_y {
        if let Some(cell) = buf.cell_mut((rect.x, sep_y)) {
            cell.set_symbol("├");
        }
        if rect.right() > rect.x
            && let Some(cell) = buf.cell_mut((rect.right() - 1, sep_y))
        {
            cell.set_symbol("┤");
        }
    }

    // Every column/row anchor's own grab point gets a real hit target
    // here, registered unconditionally — nothing is drawn there unless
    // a connector is actually anchored to it (an unattached candidate
    // dot on every row/column of every table read as clutter, not a
    // hint), but the spot has to be reachable before that's true, or a
    // drag could never land on it to begin with. It sits one row above
    // the border, on the other side of a gap the border itself has no
    // hit at all across.
    // Wider than the single cell the dot is actually drawn on, and
    // stretched back in to cover the border cell right next to it too
    // — a dot is a real mouse target, not just a render position, and a
    // single terminal cell is a hard thing to land a real cursor on.
    let srect = WorldRect::new(rect.x as i32, rect.y as i32, rect.width, rect.height);
    for (c, _) in widths.iter().enumerate() {
        if let Some(frac) = crate::table::col_center_frac(table, c, rect.width) {
            let anchor = CellAnchor { row: None, col: Some(c) };
            for side in [Side::Top, Side::Bottom] {
                let (x, y) = side_point(srect, side, frac);
                let hit_y = if side == Side::Bottom { y - 1 } else { y };
                hits.put(rect_span(x - 1, hit_y, 3, 2), HitTarget::AnchorDot(node.id.clone(), anchor));
            }
        }
    }
    for r in 0..table.len() {
        if let Some(frac) = crate::table::row_center_frac(table, r, rect.height) {
            let anchor = CellAnchor { row: Some(r), col: None };
            for side in [Side::Left, Side::Right] {
                let (x, y) = side_point(srect, side, frac);
                let hit_x = if side == Side::Right { x - 1 } else { x };
                hits.put(rect_span(hit_x, y - 1, 2, 3), HitTarget::AnchorDot(node.id.clone(), anchor));
            }
        }
    }
}

/// Row/column buttons for an open table editor, one row below the box
/// — `Ctrl`+arrow does the same thing, but plenty of terminals never
/// forward that combination at all, so this is the reliable path to it.
fn draw_table_menu(frame: &mut Frame, app: &mut App, id: &str, node_rect: Rect, canvas_area: Rect) {
    let y = node_rect.bottom();
    let mut x = node_rect.x;
    for (label, op) in [
        ("+col", TableOp::AddCol),
        ("-col", TableOp::DelCol),
        ("+row", TableOp::AddRow),
        ("-row", TableOp::DelRow),
    ] {
        let rect = Rect::new(x, y, label.len() as u16, 1).intersection(canvas_area);
        if !rect.is_empty() {
            app.hits.put(rect, HitTarget::TableMenu(id.to_string(), op));
            frame.render_widget(Paragraph::new(label).style(Style::default().fg(RColor::DarkGray)), rect);
        }
        x += label.len() as u16 + 1;
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
    live: Option<&(String, WorldRect)>,
    reattaching: Option<&(String, Endpoint)>,
    canvas_area: Rect,
) {
    // Screen-space rects throughout — everything downstream (routing,
    // glyph placement, hit registration) is bounded per-cell, so a box
    // off the visible canvas simply routes to coordinates whose glyphs
    // `put_char` then drops.
    let camera = app.camera;
    let rect_of = |id: &str| -> Option<WorldRect> {
        live.filter(|(live_id, _)| live_id == id)
            .map(|(_, r)| *r)
            .or_else(|| app.canvas.node(id).map(|n| to_screen(n.rect, camera, canvas_area)))
    };

    // Which side of `from` and of `to` each edge leaves/arrives on, so
    // several edges leaving the same box on the same side can be spread
    // across it instead of all riding the exact midpoint and overlapping
    // for however far they travel together.
    let rects: Vec<Option<(WorldRect, WorldRect)>> = app
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
                _ => {
                    let (auto_fs, auto_ts) = sides_for(r.0, r.1);
                    let fs = forced_vertical(edge.from_anchor).map(|v| side_toward(r.0, r.1, v)).unwrap_or(auto_fs);
                    let ts = forced_vertical(edge.to_anchor).map(|v| side_toward(r.1, r.0, v)).unwrap_or(auto_ts);
                    Some((fs, ts))
                }
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

    // A row/column anchor overrides the fan-out spacing above with the
    // exact position that row or column sits at — only meaningful on
    // the axis the resolved exit side actually varies along (a column
    // anchor on a side that leaves top/bottom, a row anchor on one that
    // leaves left/right); the other axis's anchor, if any, is silently
    // unusable for this edge's current side and falls back to it.
    let table_of = |id: &str| app.table_cache.get(id).and_then(|(_, t)| t.clone());
    // `route`'s own automatic geometry draws one straight line through
    // wherever the two boxes' extents happen to overlap on the shared
    // axis — a real spot to put a line, but not necessarily anywhere
    // near the anchored row/column, and that overlap can be a single
    // row wide or none at all. An anchored edge routes like an
    // explicit-sides one instead: each end's own point, independently,
    // joined by a bend if they don't already line up.
    let mut anchored = vec![false; app.canvas.edges.len()];
    for (i, edge) in app.canvas.edges.iter().enumerate() {
        let Some((from_rect, to_rect)) = rects[i] else { continue };
        let Some((fs, ts)) = sides[i] else { continue };
        let mut from_anchored = None;
        let mut to_anchored = None;
        if let Some(anchor) = edge.from_anchor
            && let Some(table) = table_of(&edge.from)
        {
            from_anchored = match fs {
                Side::Top | Side::Bottom => anchor.col.and_then(|c| crate::table::col_center_frac(&table, c, from_rect.width)),
                Side::Left | Side::Right => anchor.row.and_then(|r| crate::table::row_center_frac(&table, r, from_rect.height)),
            };
        }
        if let Some(anchor) = edge.to_anchor
            && let Some(table) = table_of(&edge.to)
        {
            to_anchored = match ts {
                Side::Top | Side::Bottom => anchor.col.and_then(|c| crate::table::col_center_frac(&table, c, to_rect.width)),
                Side::Left | Side::Right => anchor.row.and_then(|r| crate::table::row_center_frac(&table, r, to_rect.height)),
            };
        }
        // When the two boxes line up on an axis, `route`'s own straight
        // line collapses both ends onto one shared coordinate — read
        // only from `from_frac`, `to_frac` silently unused. Mirroring a
        // lone anchor onto the other slot covers that case too, since
        // whichever slot `route` actually reads then already holds it.
        match (from_anchored, to_anchored) {
            (Some(f), None) => {
                from_frac[i] = f;
                to_frac[i] = f;
            }
            (None, Some(t)) => {
                from_frac[i] = t;
                to_frac[i] = t;
            }
            (Some(f), Some(t)) => {
                from_frac[i] = f;
                to_frac[i] = t;
            }
            (None, None) => {}
        }
        anchored[i] = from_anchored.is_some() || to_anchored.is_some();
    }

    for i in 0..app.canvas.edges.len() {
        let (color, to_end, from_end, label, edge_id, explicit_sides, has_from_anchor, has_to_anchor) = {
            let edge = &app.canvas.edges[i];
            (
                edge.color.clone(),
                edge.to_end,
                edge.from_end,
                edge.label.clone(),
                edge.id.clone(),
                edge.from_side.zip(edge.to_side).or(if anchored[i] { sides[i] } else { None }),
                edge.from_anchor.is_some(),
                edge.to_anchor.is_some(),
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
        // The direction an end is entered/left from — read against the
        // nearest waypoint that actually differs, not just the
        // adjacent one: a bend that degenerates onto the endpoint (the
        // straight-line case) leaves a zero-length final leg, and a
        // direction of (0, 0) both picks a junk arrow glyph and makes
        // the anchored-arrow step-back below a no-op, parking the
        // arrow under the `●` that then paints over it.
        let dir_into = |end: usize, inward: &mut dyn Iterator<Item = usize>| {
            inward
                .map(|j| waypoints[j])
                .find(|&w| w != waypoints[end])
                .map(|w| (waypoints[end].0 - w.0, waypoints[end].1 - w.1))
                .unwrap_or((0, 0))
        };
        // An anchored end shows both its `●` (on the attachment point
        // itself) and the arrowhead — one cell can't hold two glyphs,
        // so the arrow steps back one cell along its own final leg to
        // make room, rather than either one painting over the other.
        if to_end == EdgeEnd::Arrow {
            let (dx, dy) = dir_into(last, &mut (0..last).rev());
            let (mut ax, mut ay) = waypoints[last];
            if has_to_anchor {
                ax -= dx.signum();
                ay -= dy.signum();
            }
            put_char(frame, ax, ay, arrow_char(dx, dy), style);
        }
        if from_end == EdgeEnd::Arrow {
            let (dx, dy) = dir_into(0, &mut (1..waypoints.len()));
            let (mut ax, mut ay) = waypoints[0];
            if has_from_anchor {
                ax -= dx.signum();
                ay -= dy.signum();
            }
            put_char(frame, ax, ay, arrow_char(dx, dy), style);
        }
        if has_from_anchor {
            put_char(frame, waypoints[0].0, waypoints[0].1, '●', style);
        }
        if has_to_anchor {
            put_char(frame, waypoints[last].0, waypoints[last].1, '●', style);
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

/// Same as `rect_at`, but `w`x`h` instead of a single cell — for a hit
/// target that needs real room for an imprecise cursor, not just a
/// render position.
fn rect_span(x: i32, y: i32, w: u16, h: u16) -> Rect {
    if x < 0 || y < 0 || x > u16::MAX as i32 || y > u16::MAX as i32 {
        return Rect::default();
    }
    Rect::new(x as u16, y as u16, w, h)
}

/// Whether a table anchor pins a connector end to leave from the top
/// or bottom (a column, `true`) or the left or right (a row, `false`)
/// — `None` for a full cell (row and column both), which doesn't pin a
/// single axis on its own, or no anchor at all. Forcing the axis here,
/// not just the position along whichever side gets picked, is what
/// keeps "attached to this column" true after either box moves: the
/// ordinary automatic side pick answers a different question (which
/// side do these two boxes' *current positions* suggest), and moving
/// one can flip it right off the axis the anchor actually names.
fn forced_vertical(anchor: Option<CellAnchor>) -> Option<bool> {
    match anchor {
        Some(CellAnchor { col: Some(_), row: None }) => Some(true),
        Some(CellAnchor { row: Some(_), col: None }) => Some(false),
        _ => None,
    }
}

/// The specific top/bottom or left/right side of `this` that faces
/// `other` — `sides_for`'s own tie-break, reused so a forced axis still
/// picks whichever concrete side actually points toward the other box.
fn side_toward(this: WorldRect, other: WorldRect, vertical: bool) -> Side {
    let (tcx, tcy) = center(this);
    let (ocx, ocy) = center(other);
    if vertical {
        if ocy > tcy { Side::Bottom } else { Side::Top }
    } else if ocx > tcx {
        Side::Right
    } else {
        Side::Left
    }
}

/// Which side of `from` an edge leaves on and which side of `to` it
/// arrives on — the same three cases [`route`] draws, but usable before
/// any actual coordinate is picked, so several edges sharing a side can
/// be spread across it first.
fn sides_for(from: WorldRect, to: WorldRect) -> (Side, Side) {
    let (fx0, fy0, fx1, fy1) = (from.x, from.y, from.right(), from.bottom());
    let (tx0, ty0, tx1, ty1) = (to.x, to.y, to.right(), to.bottom());

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
fn side_point(rect: WorldRect, side: Side, frac: f32) -> (i32, i32) {
    let (x0, y0, x1, y1) = (rect.x, rect.y, rect.right(), rect.bottom());
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
fn route_explicit(from: WorldRect, to: WorldRect, from_frac: f32, to_frac: f32, from_side: Side, to_side: Side) -> Vec<(i32, i32)> {
    let e = side_point(from, from_side, from_frac);
    let n = side_point(to, to_side, to_frac);
    let horizontal = |s: Side| matches!(s, Side::Left | Side::Right);
    let clips = |x: i32, y: i32| inside(from, x, y) || inside(to, x, y);
    match (horizontal(from_side), horizontal(to_side)) {
        (true, true) => {
            let mid_x = (e.0 + n.0) / 2;
            vec![e, (mid_x, e.1), (mid_x, n.1), n]
        }
        (false, false) => {
            let mid_y = (e.1 + n.1) / 2;
            vec![e, (e.0, mid_y), (n.0, mid_y), n]
        }
        // Two ways to bend between a horizontal exit and a vertical
        // one — cut across at the source's own row/column first, or
        // travel out along the source's own exit line first. Forcing
        // a side against what the boxes' plain positions would've
        // picked (an anchor's whole point) can make the "cut across
        // first" order run straight underneath a box sitting in the
        // way — so it's only used when it actually doesn't.
        (true, false) => {
            if clips(n.0, e.1) { vec![e, (e.0, n.1), n] } else { vec![e, (n.0, e.1), n] }
        }
        (false, true) => {
            if clips(e.0, n.1) { vec![e, (n.0, e.1), n] } else { vec![e, (e.0, n.1), n] }
        }
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
fn route(from: WorldRect, to: WorldRect, from_frac: f32, to_frac: f32, sides: Option<(Side, Side)>) -> Vec<(i32, i32)> {
    if let Some((fs, ts)) = sides {
        return route_explicit(from, to, from_frac, to_frac, fs, ts);
    }
    let (fx0, fy0, fx1, fy1) = (from.x, from.y, from.right(), from.bottom());
    let (tx0, ty0, tx1, ty1) = (to.x, to.y, to.right(), to.bottom());

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

fn center(rect: WorldRect) -> (i32, i32) {
    (
        rect.x + rect.width as i32 / 2,
        rect.y + rect.height as i32 / 2,
    )
}

fn inside(rect: WorldRect, x: i32, y: i32) -> bool {
    x >= rect.x
        && x < rect.x + rect.width as i32
        && y >= rect.y
        && y < rect.y + rect.height as i32
}

/// A row/column anchor's own position along whichever side it forces —
/// `None` when there's no table to measure, no anchor on that axis, or
/// no anchor at all, in which case the caller's own default (a plain
/// box's own midpoint) stands.
fn anchor_frac(table: Option<&Table>, anchor: Option<CellAnchor>, vertical: bool, rect: WorldRect) -> Option<f32> {
    let table = table?;
    if vertical {
        crate::table::col_center_frac(table, anchor?.col?, rect.width)
    } else {
        crate::table::row_center_frac(table, anchor?.row?, rect.height)
    }
}

/// While a connector's still being dragged, previews it with the exact
/// same orthogonal-bend routing a landed one gets, rather than a raw
/// diagonal line toward the pointer that never looked like what
/// letting go would actually draw. `from_anchor`, when the drag
/// started on a row/column dot, forces the same exit axis and position
/// the final edge would end up with. `hover_target`, when the cursor
/// is currently over another box (and, if that box is a table, the
/// anchor a drop right there would carry), previews the far end the
/// same way — so aiming at a specific row/column doesn't just work at
/// the source, it can be seen and aimed at the destination too, before
/// ever letting go.
fn draw_drag_preview(
    frame: &mut Frame,
    from: (&Node, WorldRect),
    from_anchor: Option<CellAnchor>,
    cursor: (u16, u16),
    hover_target: Option<(&Node, WorldRect, Option<CellAnchor>)>,
) {
    let (from_node, from_rect) = from;
    let from_table = crate::table::parse(&display_text(from_node));
    let (to_rect, to_anchor, to_table) = match hover_target {
        Some((node, rect, anchor)) => (rect, anchor, crate::table::parse(&display_text(node))),
        None => (WorldRect::new(cursor.0 as i32, cursor.1 as i32, 1, 1), None, None),
    };

    let (auto_fs, auto_ts) = sides_for(from_rect, to_rect);
    let from_vertical = forced_vertical(from_anchor);
    let to_vertical = forced_vertical(to_anchor);
    let fs = from_vertical.map(|v| side_toward(from_rect, to_rect, v)).unwrap_or(auto_fs);
    let ts = to_vertical.map(|v| side_toward(to_rect, from_rect, v)).unwrap_or(auto_ts);
    let from_frac = from_vertical.and_then(|v| anchor_frac(from_table.as_ref(), from_anchor, v, from_rect)).unwrap_or(0.5);
    let to_frac = to_vertical.and_then(|v| anchor_frac(to_table.as_ref(), to_anchor, v, to_rect)).unwrap_or(0.5);
    let sides = (from_vertical.is_some() || to_vertical.is_some()).then_some((fs, ts));

    let waypoints = route(from_rect, to_rect, from_frac, to_frac, sides);
    let style = Style::default().fg(RColor::DarkGray);
    let glyphs: Vec<(i32, i32, char)> = route_glyphs(&waypoints)
        .into_iter()
        .filter(|&(x, y, _)| !inside(from_rect, x, y) && !inside(to_rect, x, y))
        .collect();
    for &(x, y, ch) in &glyphs {
        put_char(frame, x, y, ch, style);
    }
    if waypoints.len() >= 2 {
        let last = waypoints.len() - 1;
        let (dx, dy) = (waypoints[last].0 - waypoints[last - 1].0, waypoints[last].1 - waypoints[last - 1].1);
        put_char(frame, waypoints[last].0, waypoints[last].1, arrow_char(dx, dy), style);
    }
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
        Mode::EditingCell(..) => "TABLE (tab/enter/arrows move · alt+enter line break · ctrl+z undo · +col/-col/+row/-row buttons below · esc done)",
    };
    let hint = "drag empty space to place · click to select · ● button color picker (box or connector) · dbl-click to edit · t table · drag move · shift+drag connect · corner resize · arrows/wheel pan · m map · o open file/link · y copy · esc then c color / x shape (or ends, on a connector) / d delete · ctrl+z undo · ctrl+y redo · s save · q/esc quit";
    let line = format!("{mode} — {} — {hint}", app.status);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().fg(RColor::DarkGray)),
        area,
    );
}
