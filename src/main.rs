mod app;
mod canvas_io;
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

use app::App;
use mind_app::MindApp;

type Backend = CrosstermBackend<io::Stdout>;

fn main() -> io::Result<()> {
    let path = std::env::args().nth(1).map(PathBuf::from);
    let is_markdown = path.as_ref().is_some_and(|p| {
        matches!(
            p.extension().and_then(|e| e.to_str()),
            Some("md") | Some("markdown")
        )
    });

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
        let mut canvas_area = Rect::default();
        terminal.draw(|frame| {
            let full = frame.area();
            let chunks = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(full);
            canvas_area = chunks[0];
            render::render(frame, app, chunks[0], chunks[1]);
        })?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => app.on_key(key),
                Event::Mouse(mouse) => app.on_mouse(mouse, canvas_area),
                _ => {}
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
