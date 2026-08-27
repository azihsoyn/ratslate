mod app;
mod canvas_io;
mod collab;
mod mind_app;
mod mind_render;
mod mindmap;
mod model;
mod render;

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
use mind_app::MindApp;

type Backend = CrosstermBackend<io::Stdout>;

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().cloned().map(PathBuf::from);
    let is_markdown = path.as_ref().is_some_and(|p| {
        matches!(
            p.extension().and_then(|e| e.to_str()),
            Some("md") | Some("markdown")
        )
    });

    if args.iter().any(|a| a == "--schema") {
        print_schema(is_markdown);
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--api") {
        let Some(json) = args.get(i + 1) else {
            eprintln!("--api needs a JSON argument");
            std::process::exit(2);
        };
        return run_api(path, is_markdown, json);
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = if is_markdown {
        let mut app = MindApp::new(path.expect("is_markdown implies a path"));
        let result = run_mind(&mut terminal, &mut app);
        app.save();
        result
    } else {
        let mut app = App::new(path);
        let result = run_whiteboard(&mut terminal, &mut app);
        if app.save_path.is_some() {
            app.save();
        }
        result
    };

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
    loop {
        app.pull_collab();
        if let Some(mind) = &mut app.fullscreen {
            mind.reload_if_changed();
        }

        let mut canvas_area = Rect::default();
        terminal.draw(|frame| {
            let full = frame.area();
            let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(full);
            canvas_area = chunks[0];
            match &mut app.fullscreen {
                // A file box opened for editing covers the whole screen
                // rather than living in its own separate top-level mode.
                Some(mind) => mind_render::render(frame, mind, chunks[0], chunks[1]),
                None => render::render(frame, app, chunks[0], chunks[1]),
            }
        })?;

        if event::poll(Duration::from_millis(100))? {
            let event = event::read()?;
            if let Some(mind) = &mut app.fullscreen {
                match event {
                    Event::Key(key) => mind.on_key(key),
                    Event::Mouse(mouse) => mind.on_mouse(mouse),
                    _ => {}
                }
                if mind.should_quit {
                    mind.save();
                    app.fullscreen = None;
                }
            } else {
                match event {
                    Event::Key(key) => app.on_key(key),
                    Event::Mouse(mouse) => app.on_mouse(mouse, canvas_area),
                    _ => {}
                }
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

fn run_mind(terminal: &mut Terminal<Backend>, app: &mut MindApp) -> io::Result<()> {
    loop {
        app.reload_if_changed();

        terminal.draw(|frame| {
            let full = frame.area();
            let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(full);
            mind_render::render(frame, app, chunks[0], chunks[1]);
        })?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => app.on_key(key),
                Event::Mouse(mouse) => app.on_mouse(mouse),
                _ => {}
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
fn run_api(path: Option<PathBuf>, is_markdown: bool, json: &str) -> io::Result<()> {
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

    let mut results: Vec<Value> = Vec::with_capacity(items.len());
    if is_markdown {
        let mut app = MindApp::new(path.expect("mindmap mode needs a file"));
        for item in items {
            let kind = kind_of(&item);
            results.push(mind_app::run_one(&mut app, &kind, item));
        }
    } else {
        let mut app = App::new(path);
        for item in items {
            let kind = kind_of(&item);
            results.push(app::run_one(&mut app, &kind, item));
        }
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

fn print_schema(is_markdown: bool) {
    let doc = if is_markdown {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "ratslate --api (mindmap)",
            "request": schema_for!(mind_app::Request),
            "response": schema_for!(mind_app::Response),
        })
    } else {
        serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "ratslate --api (whiteboard)",
            "request": schema_for!(app::Request),
            "response": schema_for!(app::Response),
        })
    };
    println!("{}", serde_json::to_string_pretty(&doc).unwrap());
}
