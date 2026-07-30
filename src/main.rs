use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
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
use ssh_tui::{App, ConnectionFailure, InputMode, SshConfig, ui};

const MAX_CAPTURED_STDERR: usize = 64 * 1024;

struct SshOutcome {
    status: String,
    failure: Option<ConnectionFailure>,
}

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
        app.poll_reachability();
        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.connection_failure.is_some() {
                        match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Enter | KeyCode::Esc => app.dismiss_connection_failure(),
                            _ => {}
                        }
                        continue;
                    }

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
                                    let outcome = launch_ssh(terminal, &host.alias)?;
                                    app.set_status(outcome.status);
                                    if let Some(failure) = outcome.failure {
                                        app.show_connection_failure(failure);
                                    }
                                }
                            } else {
                                app.toggle_selected_folder();
                            }
                        }
                        KeyCode::Char(' ') => app.toggle_selected_folder(),
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => {
                    if app.connection_failure.is_some() {
                        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                            app.dismiss_connection_failure();
                        }
                        continue;
                    }
                    match mouse.kind {
                        MouseEventKind::ScrollDown => app.select_next(),
                        MouseEventKind::ScrollUp => app.select_previous(),
                        MouseEventKind::Down(MouseButton::Left) => app.click_at(mouse.row),
                        _ => {}
                    }
                }
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
) -> Result<SshOutcome> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    let outcome = run_ssh(alias);

    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    terminal.autoresize()?;
    terminal.clear()?;
    terminal.hide_cursor()?;
    Ok(outcome)
}

fn run_ssh(alias: &str) -> SshOutcome {
    let mut child = match Command::new("ssh")
        .arg("--")
        .arg(alias)
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return failed_outcome(
                alias,
                format!("Could not start the ssh process: {error}"),
                None,
            );
        }
    };

    let stderr_thread = child
        .stderr
        .take()
        .map(|stderr| std::thread::spawn(move || capture_and_relay_stderr(stderr, io::stderr())));
    let status = child.wait();
    let captured = stderr_thread
        .and_then(|thread| thread.join().ok())
        .unwrap_or_default();

    match status {
        Ok(status) if status.success() => SshOutcome {
            status: format!("Session with {alias} ended"),
            failure: None,
        },
        Ok(status) => failed_outcome(alias, summarize_stderr(&captured), status.code()),
        Err(error) => failed_outcome(
            alias,
            format!("Could not wait for the ssh process: {error}"),
            None,
        ),
    }
}

fn failed_outcome(alias: &str, message: String, exit_status: Option<i32>) -> SshOutcome {
    SshOutcome {
        status: format!("Connection to {alias} failed"),
        failure: Some(ConnectionFailure {
            alias: alias.to_string(),
            message,
            exit_status,
        }),
    }
}

fn capture_and_relay_stderr(mut reader: impl Read, mut relay: impl Write) -> Vec<u8> {
    let mut captured = VecDeque::with_capacity(MAX_CAPTURED_STDERR);
    let mut buffer = [0_u8; 4096];

    while let Ok(read) = reader.read(&mut buffer) {
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        let _ = relay.write_all(chunk);
        let _ = relay.flush();

        let overflow = captured
            .len()
            .saturating_add(chunk.len())
            .saturating_sub(MAX_CAPTURED_STDERR);
        if overflow > 0 {
            captured.drain(..overflow.min(captured.len()));
        }
        captured.extend(chunk);
    }

    captured.into()
}

fn summarize_stderr(captured: &[u8]) -> String {
    let stripped = strip_ansi_escapes::strip(captured);
    let text = String::from_utf8_lossy(&stripped);
    let mut lines = text
        .lines()
        .map(|line| {
            line.chars()
                .filter(|character| !character.is_control() || *character == '\t')
                .collect::<String>()
        })
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    if lines.is_empty() {
        return "SSH exited without an error message.".to_string();
    }
    if lines.len() > 5 {
        lines.drain(..lines.len() - 5);
    }
    lines
        .into_iter()
        .map(|line| {
            if line.chars().count() > 180 {
                format!("{}...", line.chars().take(177).collect::<String>())
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_and_limits_ssh_error_output() {
        let captured = b"\x1b[31mfirst\x1b[0m\nsecond\nthird\nfourth\nfifth\nssh: final error\n";

        let summary = summarize_stderr(captured);

        assert!(!summary.contains('\u{1b}'));
        assert!(!summary.contains("first"));
        assert!(summary.contains("ssh: final error"));
        assert_eq!(summary.lines().count(), 5);
    }

    #[test]
    fn captures_stderr_while_relaying_it() {
        let input = b"ssh: connection failed";
        let mut relayed = Vec::new();

        let captured = capture_and_relay_stderr(&input[..], &mut relayed);

        assert_eq!(captured, input);
        assert_eq!(relayed, input);
    }
}
