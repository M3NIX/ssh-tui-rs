use std::{
    io::{self, Write},
    path::PathBuf,
    process::Command,
    time::Duration,
};

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use ssh_tui::{App, InputMode, SshConfig, ui};

#[derive(Debug, Parser)]
#[command(author, version, about = "Keyboard-first SSH config browser")]
struct Args {
    #[arg(short, long, value_name = "PATH")]
    config: Option<PathBuf>,

    #[arg(long, help = "Disable launching ssh on Enter")]
    browse_only: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = SshConfig::load(args.config.as_deref())?;
    let mut app = App::new(config);
    run(&mut app, args.browse_only)
}

fn run(app: &mut App, browse_only: bool) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, app, browse_only);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    browse_only: bool,
) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.input_mode == InputMode::Search {
                        match key.code {
                            KeyCode::Esc => app.clear_search(),
                            KeyCode::Enter => app.finish_search(),
                            KeyCode::Backspace => app.pop_search(),
                            KeyCode::Char(c) => app.push_search(c),
                            KeyCode::Up => app.select_previous(),
                            KeyCode::Down => app.select_next(),
                            _ => {}
                        }
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('/') => app.start_search(),
                        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
                        KeyCode::Char('k') | KeyCode::Up => app.select_previous(),
                        KeyCode::Char('h') | KeyCode::Left => app.collapse_selected(),
                        KeyCode::Char('l') | KeyCode::Right => app.expand_or_enter_selected(),
                        KeyCode::Enter => {
                            if let Some(host) = app.selected_host() {
                                if browse_only {
                                    app.set_status(format!(
                                        "Browse-only mode: ssh {} was not launched",
                                        host.alias
                                    ));
                                } else {
                                    let status = launch_ssh(terminal, &host.alias)?;
                                    app.set_status(status);
                                }
                            } else {
                                app.toggle_selected_folder();
                            }
                        }
                        KeyCode::Char(' ') => app.toggle_selected_folder(),
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollDown => app.select_next(),
                    MouseEventKind::ScrollUp => app.select_previous(),
                    MouseEventKind::Down(MouseButton::Left) => app.click_at(mouse.row),
                    _ => {}
                },
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }

    Ok(())
}

fn launch_ssh(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    alias: &str,
) -> Result<String> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    let status = Command::new("ssh").arg(alias).status();
    println!();
    let message = match status {
        Ok(status) if status.success() => format!("ssh {alias} exited successfully"),
        Ok(status) => format!("ssh {alias} failed with {status}"),
        Err(error) => format!("Failed to launch ssh {alias}: {error}"),
    };
    println!("{message}");
    print!("Press Enter to return to ssh-tui...");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;

    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    terminal.autoresize()?;
    terminal.clear()?;
    terminal.hide_cursor()?;
    Ok(message)
}
