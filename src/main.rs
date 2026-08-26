mod app;
mod canvas_io;
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

type Backend = CrosstermBackend<io::Stdout>;

fn main() -> io::Result<()> {
    let save_path = std::env::args().nth(1).map(PathBuf::from);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut app = App::new(save_path);
    let result = run(&mut terminal, &mut app);

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

fn run(terminal: &mut Terminal<Backend>, app: &mut App) -> io::Result<()> {
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
