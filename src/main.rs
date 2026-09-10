mod app;
mod canvas_io;
mod layout;
mod collab;
mod model;
mod render;
mod table;

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
};
use schemars::schema_for;
use serde_json::Value;

use app::App;

type Backend = CrosstermBackend<io::Stdout>;

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Every non-flag argument is a board. Several at once open as
    // tabs sharing one camera — flip between "before" and "after"
    // drawn at the same coordinates. Headless modes use the first.
    let paths: Vec<PathBuf> = {
        let mut ps = Vec::new();
        let mut skip = false;
        for a in &args {
            if skip {
                skip = false;
                continue;
            }
            if a == "--api" {
                skip = true;
            } else if !a.starts_with("--") {
                ps.push(PathBuf::from(a));
            }
        }
        ps
    };
    let path = paths.first().cloned();

    if args.iter().any(|a| a == "--schema") {
        print_schema();
        return Ok(());
    }
    // The board as plain text on stdout — pipe it into a doc, a PR, a
    // pager. The `--api` request `render` returns the same thing
    // wrapped in JSON.
    if args.iter().any(|a| a == "--render") {
        let mut app = App::new(path);
        println!("{}", render::to_ascii(&mut app));
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--api") {
        let Some(json) = args.get(i + 1) else {
            eprintln!("--api needs a JSON argument");
            std::process::exit(2);
        };
        return run_api(path, json);
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut apps: Vec<App> = if paths.is_empty() {
        vec![App::new(None)]
    } else {
        paths.iter().map(|p| App::new(Some(p.clone()))).collect()
    };
    let result = run_whiteboard(&mut terminal, &mut apps);
    for app in &mut apps {
        if app.save_path.is_some() {
            app.save();
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_whiteboard(terminal: &mut Terminal<Backend>, apps: &mut Vec<App>) -> io::Result<()> {
    let mut canvas_area = Rect::default();
    let mut active = 0usize;
    // Redraw only when something actually happened — an input event, a
    // change merged from another writer, or the first frame. Any-motion
    // mouse tracking delivers an event for every cell the cursor
    // crosses; drawing per event turned a flick of the mouse into
    // hundreds of full-frame renders, and an idle board still redrew
    // ten times a second for nothing.
    let mut dirty = true;
    loop {
        let names: Vec<String> = apps
            .iter()
            .map(|a| {
                a.save_path
                    .as_deref()
                    .and_then(|p| p.file_stem())
                    .map(|st| st.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "untitled".to_string())
            })
            .collect();
        let app = &mut apps[active];
        app.tab_names = names;
        app.active_tab = active;

        dirty |= app.pull_collab();

        if dirty {
            terminal.draw(|frame| {
                let full = frame.area();
                let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(full);
                canvas_area = chunks[0];
                render::render(frame, app, chunks[0], chunks[1]);
            })?;
            dirty = false;
        }

        if event::poll(Duration::from_millis(100))? {
            // Handle everything arriving within one frame's budget and
            // draw once at the end. A real mouse streams motion at
            // 60–120Hz, one event at a time — draining only what's
            // already queued still meant one full render per event, so
            // a drag redrew at pointer rate. Waiting out the remainder
            // of the budget for stragglers caps redraws near 60fps
            // however the events arrive, at the cost of ~15ms of
            // draw latency nothing can perceive.
            let deadline = std::time::Instant::now() + Duration::from_millis(15);
            loop {
                match event::read()? {
                    Event::Key(key) => {
                        app.on_key(key);
                        dirty = true;
                    }
                    Event::Mouse(mouse) => dirty |= app.on_mouse(mouse, canvas_area),
                    Event::Resize(..) => dirty = true,
                    _ => {}
                }
                let now = std::time::Instant::now();
                if now >= deadline || !event::poll(deadline - now)? {
                    break;
                }
            }
        }

        if app.should_quit {
            break;
        }

        if let Some(action) = app.tab_action.take() {
            dirty = true;
            handle_tab_action(apps, &mut active, action);
        }
    }
    Ok(())
}

/// The session-level half of the tab bar: switching carries the camera
/// (and the minimap toggle) so the view stays put while the board
/// underneath it changes — that's the whole point of stacking several
/// files on the same coordinates. `New` clones the active board into
/// the next free `<stem>-N.canvas` beside it, the natural way to start
/// an "after" from a finished "before".
fn handle_tab_action(apps: &mut Vec<App>, active: &mut usize, action: app::TabAction) {
    use app::TabAction;
    let n = apps.len();
    let target = match action {
        TabAction::Goto(i) if i < n => Some(i),
        TabAction::Goto(_) => None,
        TabAction::Next if n > 1 => Some((*active + 1) % n),
        TabAction::Prev if n > 1 => Some((*active + n - 1) % n),
        TabAction::Next | TabAction::Prev => {
            apps[*active].status = "only one board open — T clones this one as a new tab".to_string();
            None
        }
        TabAction::New => {
            let Some(src_path) = apps[*active].save_path.clone() else {
                apps[*active].status = "no file to clone — open a saved board first".to_string();
                return;
            };
            let stem = src_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "board".into());
            let ext = src_path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_else(|| "canvas".into());
            let dir = src_path.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
            let mut k = 2;
            let new_path = loop {
                let candidate = dir.join(format!("{stem}-{k}.{ext}"));
                if !candidate.exists() {
                    break candidate;
                }
                k += 1;
            };
            if let Err(e) = canvas_io::save(&apps[*active].canvas, &new_path) {
                apps[*active].status = format!("couldn't clone board: {e}");
                return;
            }
            apps.push(App::new(Some(new_path.clone())));
            let i = apps.len() - 1;
            apps[i].status = format!("cloned into {}", new_path.display());
            Some(i)
        }
    };
    if let Some(i) = target
        && i != *active
    {
        let camera = apps[*active].camera;
        let minimap = apps[*active].minimap;
        *active = i;
        apps[i].camera = camera;
        apps[i].minimap = minimap;
    }
}

/// Headless: apply one request, or a batch of them, without a
/// terminal. The exact same `dispatch` the TUI's mouse and key
/// handlers call — this is not a second implementation of what a move
/// or an edit means, just another way to name one.
fn run_api(path: Option<PathBuf>, json: &str) -> io::Result<()> {
    let value: Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(e) => {
            println!("{}", serde_json::json!({"id": "?", "error": {"message": e.to_string()}}));
            return Ok(());
        }
    };
    let is_batch = value.is_array();
    let items: Vec<Value> = match value {
        Value::Array(items) => items,
        other => vec![other],
    };

    let mut app = App::new(path);
    let mut results: Vec<Value> = Vec::with_capacity(items.len());
    for item in items {
        let kind = kind_of(&item);
        results.push(app::run_one(&mut app, &kind, item));
    }

    let out = if is_batch {
        serde_json::json!({"id": "batch", "result": results})
    } else {
        results.into_iter().next().unwrap_or_else(|| serde_json::json!({"id": "?", "error": {"message": "empty request"}}))
    };
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
    Ok(())
}

fn kind_of(v: &Value) -> String {
    v.get("type").and_then(Value::as_str).unwrap_or("?").to_string()
}

fn print_schema() {
    let doc = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "ratslate --api",
        "request": schema_for!(app::Request),
        "response": schema_for!(app::Response),
    });
    println!("{}", serde_json::to_string_pretty(&doc).unwrap());
}
