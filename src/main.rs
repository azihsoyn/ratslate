mod app;
mod canvas_io;
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
    let path = args.first().cloned().map(PathBuf::from);

    if args.iter().any(|a| a == "--schema") {
        print_schema();
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

    let mut app = App::new(path);
    let result = run_whiteboard(&mut terminal, &mut app);
    if app.save_path.is_some() {
        app.save();
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

fn run_whiteboard(terminal: &mut Terminal<Backend>, app: &mut App) -> io::Result<()> {
    let mut canvas_area = Rect::default();
    // Redraw only when something actually happened — an input event, a
    // change merged from another writer, or the first frame. Any-motion
    // mouse tracking delivers an event for every cell the cursor
    // crosses; drawing per event turned a flick of the mouse into
    // hundreds of full-frame renders, and an idle board still redrew
    // ten times a second for nothing.
    let mut dirty = true;
    loop {
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
    }
    Ok(())
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
