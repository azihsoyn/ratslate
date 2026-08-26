use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Clear, Paragraph},
};

use crate::mind_app::{MindApp, Mode};
use crate::mindmap::{Kind, MNode, NodeId};

const INDENT: u16 = 2;

pub fn render(frame: &mut Frame, app: &mut MindApp, canvas_area: Rect, status_area: Rect) {
    let held = app.sort.held().copied();
    let flat = app.tree.flat(held);
    let target_slot = app.sort.over().map(|(_, s)| s);

    let mut rows: Vec<(NodeId, Rect)> = Vec::new();
    let mut gap: Option<Rect> = None;
    let mut y = canvas_area.y;

    for (i, &id) in flat.iter().enumerate() {
        if target_slot == Some(i) {
            gap = Some(Rect::new(canvas_area.x, y, canvas_area.width, 1).intersection(canvas_area));
            y += 1;
        }
        let node = app.tree.node(id).expect("came from this tree's own flat()");
        let indent = node.depth as u16 * INDENT;
        let row = Rect::new(
            canvas_area.x + indent,
            y,
            canvas_area.width.saturating_sub(indent),
            1,
        )
        .intersection(canvas_area);
        let selected = app.selected == Some(id);
        let editing = matches!(app.mode, Mode::Editing(eid) if eid == id);
        draw_row(frame, node, row, selected, editing, &app.editing_text);
        rows.push((id, row));
        y += 1;
    }
    if target_slot == Some(flat.len()) {
        gap = Some(Rect::new(canvas_area.x, y, canvas_area.width, 1).intersection(canvas_area));
    }

    app.sort.container((), canvas_area, &rows);

    if let Some(g) = gap {
        frame.render_widget(
            Paragraph::new("·".repeat(g.width as usize)).style(Style::default().fg(Color::DarkGray)),
            g,
        );
    }

    if let Some(g) = app.sort.ghost(canvas_area)
        && let Some(id) = held
        && let Some(node) = app.tree.node(id)
    {
        frame.render_widget(Clear, g);
        draw_row(frame, node, g, true, false, "");
    }

    draw_status(frame, app, status_area);
}

fn draw_row(frame: &mut Frame, node: &MNode, rect: Rect, selected: bool, editing: bool, editing_text: &str) {
    let marker = match node.kind {
        Kind::Heading(level) => "#".repeat(level as usize),
        Kind::ListItem(b) => b.to_string(),
    };
    let text = if editing { editing_text } else { &node.text };
    let mut line = format!("{marker} {text}");
    if editing {
        line.push('▏');
    }
    let mut style = match node.kind {
        Kind::Heading(_) => Style::default().add_modifier(Modifier::BOLD),
        Kind::ListItem(_) => Style::default(),
    };
    if selected {
        style = style.fg(Color::Cyan);
    }
    frame.render_widget(Paragraph::new(line).style(style), rect);
}

fn draw_status(frame: &mut Frame, app: &MindApp, area: Rect) {
    let mode = match app.mode {
        Mode::Normal => "NORMAL",
        Mode::Editing(_) => "EDIT (Esc/Enter to leave)",
    };
    let hint = "click select · drag reorder/re-parent · enter edit · ctrl+z undo · s save · q/esc quit";
    let line = format!("{mode} — {} — {hint}", app.status);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}
