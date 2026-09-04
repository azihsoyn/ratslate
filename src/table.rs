//! A "table box" is nothing but a text box whose content happens to be
//! a GFM markdown table — no new node kind, no schema change, so it
//! opens in any other JSON Canvas reader as the same plain text/table
//! it always would. Parsing and formatting live here; the grid editor
//! in `app.rs` and the grid renderer in `render.rs` both work on the
//! plain `Vec<Vec<String>>` this produces, never the raw markdown.

/// A row is a header if it's `rows[0]`; everything else is data. The
/// separator row (`| --- | --- |`) isn't part of this at all — it's
/// reconstructed on `format`, and never carries data of its own.
pub type Table = Vec<Vec<String>>;

fn split_row(line: &str) -> Option<Vec<String>> {
    let line = line.trim();
    let inner = line.strip_prefix('|').unwrap_or(line);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    if inner.trim().is_empty() {
        return None;
    }
    Some(inner.split('|').map(|cell| cell.trim().to_string()).collect())
}

/// Only `-`, `:`, spaces and `|` — a GFM header-separator row, the one
/// thing that actually marks this as a table rather than any other
/// line that happens to contain a few `|` characters.
fn is_separator_row(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty() && line.chars().all(|c| matches!(c, '-' | ':' | ' ' | '|'))
}

/// Parses `text` as a GFM table: a header row, a separator row, then
/// zero or more data rows, every row padded/truncated to the header's
/// own column count. `None` for anything else — plain prose keeps
/// rendering and editing exactly as a text box always has.
pub fn parse(text: &str) -> Option<Table> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 2 {
        return None;
    }
    let header = split_row(lines[0])?;
    if !is_separator_row(lines[1]) {
        return None;
    }
    let cols = header.len();
    let mut rows = vec![header];
    for line in &lines[2..] {
        let mut row = split_row(line).unwrap_or_default();
        row.resize(cols, String::new());
        rows.push(row);
    }
    Some(rows)
}

/// The inverse of `parse` — always emits a plain, unaligned `---`
/// separator (no per-column alignment markers), since nothing here
/// tracks alignment yet.
pub fn format(table: &Table) -> String {
    let mut out = String::new();
    for (i, row) in table.iter().enumerate() {
        out.push('|');
        for cell in row {
            out.push(' ');
            out.push_str(cell);
            out.push_str(" |");
        }
        out.push('\n');
        if i == 0 {
            out.push('|');
            for _ in row {
                out.push_str(" --- |");
            }
            out.push('\n');
        }
    }
    out.pop();
    out
}

/// A blank starting point for `t` on a box with no table of its own
/// yet — one header cell, one data row underneath it, both empty and
/// ready to type into.
pub fn blank() -> Table {
    vec![vec![String::new()], vec![String::new()]]
}

/// The narrowest a column is ever allowed to render — a freshly
/// inserted column has every cell empty, and a width of 0 would render
/// with no click target at all, making it uneditable by mouse.
const MIN_COL_WIDTH: usize = 3;

/// A GFM table row can't hold a literal newline — rows are split on
/// `.lines()`, so one would silently start a whole new row instead of
/// breaking a cell. A line break within a cell is spelled the same way
/// GitHub and other GFM renderers already read one: an HTML `<br>`
/// right in the cell text.
const CELL_BREAK: &str = "<br>";

/// A cell's own display lines — everything downstream that measures or
/// draws a cell (width, row height, the grid renderer) works off this
/// instead of the raw string, so they can't disagree on where a line
/// actually breaks.
pub fn cell_lines(s: &str) -> Vec<&str> {
    s.split(CELL_BREAK).collect()
}

/// A string's width in terminal cells — CJK and other wide glyphs
/// count as two. Everything that lays out the grid (column widths, box
/// sizing, padding) measures with this; counting chars instead put
/// every divider next to a Japanese cell in the wrong column.
pub fn display_width(s: &str) -> usize {
    use ratatui::buffer::CellWidth;
    s.cell_width() as usize
}

fn cell_width(s: &str) -> usize {
    cell_lines(s).into_iter().map(display_width).max().unwrap_or(0)
}

/// How many display lines a row needs — the tallest of its own cells,
/// since every cell in a row shares the same grid lines above and
/// below it.
pub fn row_height(row: &[String]) -> usize {
    row.iter().map(|c| cell_lines(c).len()).max().unwrap_or(1).max(1)
}

/// `editing_text` works in real newlines while a cell is open — easiest
/// thing to push/pop a char at a time. This is the one place that turns
/// them into the `<br>` a cell is actually stored as.
pub fn encode_break(s: &str) -> String {
    s.replace('\n', CELL_BREAK)
}

/// The inverse — a stored cell's `<br>`s become real newlines the
/// moment its content is loaded into `editing_text` to type into.
pub fn decode_break(s: &str) -> String {
    s.replace(CELL_BREAK, "\n")
}

/// Each column's own width: the widest line of the widest cell in it,
/// header included — shared by the grid renderer and the box's own
/// auto-grow so the two always agree on how wide a table actually is.
pub fn col_widths(table: &Table) -> Vec<usize> {
    let cols = table.first().map(Vec::len).unwrap_or(0);
    (0..cols)
        .map(|c| table.iter().filter_map(|r| r.get(c)).map(|s| cell_width(s)).max().unwrap_or(0).max(MIN_COL_WIDTH))
        .collect()
}

/// The box size a table needs to show every cell in full: one padding
/// space on each side of every column, a border, and a single grid
/// line under the header — the renderer's own column dividers already
/// run the full height, so that's the only horizontal line the grid
/// draws. A row with a multi-line cell takes as many lines as that
/// cell needs, same as the grid renderer gives it.
pub fn render_size(table: &Table) -> (u16, u16) {
    let widths = col_widths(table);
    let cols = widths.len().max(1);
    let inner: usize = widths.iter().map(|w| w + 2).sum::<usize>() + cols.saturating_sub(1);
    let width = (inner + 2).max(3) as u16;
    let content_lines: usize = table.iter().map(|r| row_height(r)).sum::<usize>().max(1);
    let seps = if table.len() > 1 { 1 } else { 0 };
    let height = (content_lines + seps + 2).max(4) as u16;
    (width, height)
}

/// Where a column's own center sits, as a 0..1 fraction of a box this
/// wide — the same padding/border layout `render_size`/the grid
/// renderer already use, so a connector anchored to a column lines up
/// with it exactly rather than approximately.
pub fn col_center_frac(table: &Table, col: usize, box_width: u16) -> Option<f32> {
    let widths = col_widths(table);
    if col >= widths.len() {
        return None;
    }
    if box_width < 2 {
        return Some(0.5);
    }
    let mut x = 1u32; // the left border
    for (i, &cw) in widths.iter().enumerate() {
        let seg = cw as u32 + 2; // one padding space each side
        if i == col {
            let center = x as f32 + seg as f32 / 2.0;
            return Some((center / box_width as f32).clamp(0.0, 1.0));
        }
        x += seg + 1; // + the divider before the next column
    }
    None
}

/// Where a row's own center sits, as a 0..1 fraction of a box this
/// tall — mirrors `col_center_frac`, accounting for the one grid line
/// under the header and any row that's taller than one line.
pub fn row_center_frac(table: &Table, row: usize, box_height: u16) -> Option<f32> {
    if row >= table.len() {
        return None;
    }
    if box_height < 2 {
        return Some(0.5);
    }
    let mut y = 1u32; // the top border
    for (i, r) in table.iter().enumerate() {
        let rh = row_height(r) as u32;
        if i == row {
            let center = y as f32 + rh as f32 / 2.0;
            return Some((center / box_height as f32).clamp(0.0, 1.0));
        }
        y += rh;
        if i == 0 && table.len() > 1 {
            y += 1; // the header's own grid line
        }
    }
    None
}
