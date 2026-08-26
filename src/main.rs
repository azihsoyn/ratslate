mod app;
mod model;

use std::io;
use std::time::Duration;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Style},
    widgets::Block,
};

use app::App;

type Backend = CrosstermBackend<io::Stdout>;

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut app = App::new();
    let result = run(&mut terminal, &mut app);

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
            canvas_area = frame.area();
            render(frame, app, canvas_area);
        })?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.code == KeyCode::Char('q') => break,
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

fn render(frame: &mut Frame, app: &mut App, canvas_area: Rect) {
    app.hits.clear();

    for b in &app.canvas.boxes {
        // Drawn as the ghost instead, while it is being carried.
        if app.drag.moving() == Some(&b.id) {
            continue;
        }
        frame.render_widget(Block::bordered(), b.rect);
        app.hits.put(b.rect, b.id);
    }

    if let Some(ghost) = app.drag.ghost(canvas_area) {
        frame.render_widget(
            Block::bordered().border_style(Style::default().fg(Color::DarkGray)),
            ghost,
        );
    }
}
