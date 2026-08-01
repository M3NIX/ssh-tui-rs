use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::Result;
use arboard::Clipboard;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use clap::Parser;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use ssh_tui_rs::{
    App, ConnectionFailure, EmbeddedMouseAction, InputMode, SSH_PROGRAM, SshConfig,
    is_ssh_error_exit_code, ssh_arguments, ui,
};

const MAX_CAPTURED_STDERR: usize = 64 * 1024;
const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);

struct SshOutcome {
    failure: Option<ConnectionFailure>,
}

#[derive(Debug, Parser)]
#[command(author, version, about = "Keyboard-first SSH config browser")]
struct Args {
    #[arg(
        short,
        long,
        value_name = "PATH",
        help = "Read OpenSSH configuration from PATH"
    )]
    config: Option<PathBuf>,

    #[arg(long, help = "Disable host reachability checks")]
    no_network_check: bool,

    #[arg(long, help = "Run SSH sessions inside the details pane")]
    embedded_ssh: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = SshConfig::load(args.config.as_deref())?;
    let mut app = App::with_features(config, !args.no_network_check, args.embedded_ssh);
    run(&mut app)
}

fn run(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    let keyboard_enhancement_enabled = try_push_keyboard_enhancement_flags(&mut stdout);
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut clipboard = Clipboard::new().ok();

    let result = run_loop(&mut terminal, app, &mut clipboard);

    if keyboard_enhancement_enabled {
        try_pop_keyboard_enhancement_flags(terminal.backend_mut());
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

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    clipboard: &mut Option<Clipboard>,
) -> Result<()> {
    let mut last_host_click = None;
    loop {
        app.poll_reachability();
        app.poll_embedded_session();
        terminal.draw(|frame| ui::draw(frame, app))?;
        if let Err(error) = app.sync_embedded_terminal_size() {
            let alias = app
                .embedded_sessions
                .get(app.active_tab)
                .map(|session| session.alias.clone())
                .unwrap_or_else(|| "SSH".to_string());
            app.close_embedded_session();
            app.show_connection_failure(ConnectionFailure {
                alias,
                message: format!("Could not resize the embedded terminal: {error}"),
                exit_status: None,
            });
        }

        let poll_interval = if app.embedded_session_running() {
            Duration::from_millis(33)
        } else {
            Duration::from_millis(250)
        };
        if event::poll(poll_interval)? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if app.connection_failure.is_some() {
                        match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Enter | KeyCode::Esc => app.dismiss_connection_failure(),
                            _ => {}
                        }
                        continue;
                    }

                    if app.embedded_session_failed() {
                        match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Enter | KeyCode::Esc => app.close_embedded_session(),
                            _ => {}
                        }
                        continue;
                    }

                    if key.kind == KeyEventKind::Press
                        && key.code == KeyCode::F(5)
                        && app.has_embedded_sessions()
                    {
                        app.toggle_embedded_focus();
                        continue;
                    }

                    if app.embedded_terminal_focused() {
                        app.send_embedded_key(key)?;
                        continue;
                    }

                    // Tab switching: Alt+←/→ or Alt+h/l when tree is focused.
                    if app.tab_count() > 1
                        && key.kind == KeyEventKind::Press
                        && key.modifiers.contains(KeyModifiers::ALT)
                    {
                        match key.code {
                            KeyCode::Right | KeyCode::Char('l') => {
                                app.next_tab();
                                continue;
                            }
                            KeyCode::Left | KeyCode::Char('h') => {
                                app.prev_tab();
                                continue;
                            }
                            _ => {}
                        }
                    }

                    if app.input_mode == InputMode::Search {
                        match key.code {
                            KeyCode::Esc => app.clear_search(),
                            KeyCode::Enter | KeyCode::Char('j')
                                if is_inline_shortcut(&key) && app.selected_host().is_some() =>
                            {
                                app.reveal_search_selection();
                                activate_selected_host(terminal, app, true)?;
                            }
                            KeyCode::Enter => app.reveal_search_selection(),
                            KeyCode::Backspace => app.pop_search(),
                            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.pop_search();
                            }
                            KeyCode::Char(c) => app.push_search(c),
                            KeyCode::Up => app.select_previous(),
                            KeyCode::Down => app.select_next(),
                            _ => {}
                        }
                        continue;
                    }

                    if is_inline_shortcut(&key) && app.selected_host().is_some() {
                        activate_selected_host(terminal, app, true)?;
                        continue;
                    }

                    if app.has_embedded_sessions() && key.code == KeyCode::Char('x') {
                        app.close_embedded_session();
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('r') => {
                            if let Err(error) = app.reload_config() {
                                app.status = format!("Reload failed: {error}");
                            }
                        }
                        KeyCode::Char('/') => app.start_search(),
                        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
                        KeyCode::Char('k') | KeyCode::Up => app.select_previous(),
                        KeyCode::Char('h') | KeyCode::Left => app.collapse_selected(),
                        KeyCode::Char('l') | KeyCode::Right => app.expand_or_enter_selected(),
                        KeyCode::Enter => {
                            if app.selected_host().is_some() {
                                activate_selected_host(terminal, app, false)?;
                            } else {
                                app.toggle_selected_group();
                            }
                        }
                        KeyCode::Char(' ') => app.toggle_selected_group(),
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

                    if app.embedded_session_failed() {
                        if app.details_contains(mouse.column, mouse.row) {
                            handle_embedded_mouse(terminal, app, clipboard, mouse)?;
                        } else if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                            app.close_embedded_session();
                        }
                        continue;
                    }

                    if app.has_embedded_sessions()
                        && app.details_contains(mouse.column, mouse.row)
                    {
                        app.focus_embedded_terminal();
                        handle_embedded_mouse(terminal, app, clipboard, mouse)?;
                        last_host_click = None;
                        continue;
                    }
                    if app.has_embedded_sessions() {
                        app.focus_tree();
                    }

                    match mouse.kind {
                        MouseEventKind::ScrollDown => app.select_next(),
                        MouseEventKind::ScrollUp => app.select_previous(),
                        MouseEventKind::Down(MouseButton::Left) => {
                            if app.search_contains(mouse.column, mouse.row) {
                                app.start_search();
                                last_host_click = None;
                            } else if app.click_at(mouse.column, mouse.row)
                                && app.selected_host().is_some()
                            {
                                let node_id = app
                                    .selected_node_id()
                                    .expect("clicked host has a selected node");
                                let now = Instant::now();
                                if register_host_click(&mut last_host_click, now, node_id) {
                                    if app.input_mode == InputMode::Search {
                                        app.reveal_search_selection();
                                    }
                                    activate_selected_host(terminal, app, false)?;
                                }
                            } else {
                                last_host_click = None;
                            }
                        }
                        _ => {}
                    }
                }
                Event::Paste(text) if app.embedded_terminal_focused() => {
                    app.send_embedded_paste(&text)?;
                }
                Event::FocusGained if app.embedded_terminal_focused() => {
                    app.send_embedded_focus(true)?;
                }
                Event::FocusLost if app.embedded_terminal_focused() => {
                    app.send_embedded_focus(false)?;
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }

    Ok(())
}

fn keyboard_enhancement_flags() -> KeyboardEnhancementFlags {
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
}

fn try_push_keyboard_enhancement_flags(writer: &mut impl Write) -> bool {
    execute!(
        writer,
        PushKeyboardEnhancementFlags(keyboard_enhancement_flags())
    )
    .is_ok()
}

fn try_pop_keyboard_enhancement_flags(writer: &mut impl Write) {
    let _ = execute!(writer, PopKeyboardEnhancementFlags);
}

fn is_inline_shortcut(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Enter
}

fn handle_embedded_mouse(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    clipboard: &mut Option<Clipboard>,
    mouse: crossterm::event::MouseEvent,
) -> Result<()> {
    if let EmbeddedMouseAction::Copy(text) = app.handle_embedded_mouse(mouse)? {
        copy_to_clipboard(terminal, clipboard, &text)?;
    }
    Ok(())
}

fn copy_to_clipboard(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    clipboard: &mut Option<Clipboard>,
    text: &str,
) -> Result<()> {
    let copied_natively = clipboard
        .as_mut()
        .is_some_and(|clipboard| clipboard.set_text(text).is_ok());
    if !copied_natively {
        terminal
            .backend_mut()
            .write_all(osc52_clipboard_sequence(text).as_bytes())?;
        terminal.backend_mut().flush()?;
    }
    Ok(())
}

fn osc52_clipboard_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x1b\\", STANDARD.encode(text))
}

fn activate_selected_host(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    force_embedded: bool,
) -> Result<()> {
    if force_embedded || app.embedded_ssh_enabled() {
        if let Err(error) = app.start_embedded_session() {
            let alias = app
                .selected_host()
                .map(|host| host.alias.clone())
                .unwrap_or_else(|| "SSH".to_string());
            app.show_connection_failure(ConnectionFailure {
                alias,
                message: format!("Could not start the embedded SSH session: {error}"),
                exit_status: None,
            });
        }
    } else if !app.has_embedded_sessions() {
        connect_selected_host(terminal, app)?;
    } else {
        app.focus_embedded_terminal();
    }
    Ok(())
}

fn register_host_click(
    previous: &mut Option<(Instant, usize)>,
    clicked_at: Instant,
    node_id: usize,
) -> bool {
    let is_double_click = previous
        .map(|(previous_at, previous_node)| {
            previous_node == node_id
                && clicked_at.duration_since(previous_at) <= DOUBLE_CLICK_INTERVAL
        })
        .unwrap_or(false);
    *previous = (!is_double_click).then_some((clicked_at, node_id));
    is_double_click
}

fn connect_selected_host(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    let Some(alias) = app.selected_host().map(|host| host.alias.clone()) else {
        return Ok(());
    };
    let outcome = launch_ssh(terminal, &app.config.source, &alias)?;
    if let Some(failure) = outcome.failure {
        app.show_connection_failure(failure);
    }
    Ok(())
}

fn launch_ssh(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: &std::path::Path,
    alias: &str,
) -> Result<SshOutcome> {
    try_pop_keyboard_enhancement_flags(terminal.backend_mut());
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    let outcome = run_ssh(config, alias);

    enable_raw_mode()?;
    let _ = try_push_keyboard_enhancement_flags(terminal.backend_mut());
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

fn run_ssh(config: &std::path::Path, alias: &str) -> SshOutcome {
    let mut command = Command::new(SSH_PROGRAM);
    command
        .args(ssh_arguments(config, alias))
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return failed_outcome(
                alias,
                format!("Could not start the SSH process: {error}"),
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
        Ok(status)
            if status
                .code()
                .is_some_and(|code| !is_ssh_error_exit_code(i64::from(code))) =>
        {
            SshOutcome { failure: None }
        }
        Ok(status) => failed_outcome(alias, summarize_stderr(&captured), status.code()),
        Err(error) => failed_outcome(
            alias,
            format!("Could not wait for the SSH process: {error}"),
            None,
        ),
    }
}

fn failed_outcome(alias: &str, message: String, exit_status: Option<i32>) -> SshOutcome {
    SshOutcome {
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

    #[test]
    fn encodes_selected_text_for_the_terminal_clipboard() {
        assert_eq!(
            osc52_clipboard_sequence("copy"),
            "\u{1b}]52;c;Y29weQ==\u{1b}\\"
        );
    }

    #[test]
    fn recognizes_alt_enter_only() {
        assert!(is_inline_shortcut(&KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::ALT
        )));
        assert!(!is_inline_shortcut(&KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::CONTROL
        )));
        assert!(!is_inline_shortcut(&KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE
        )));
    }

    #[test]
    fn parses_no_network_check_and_rejects_removed_browse_only_flag() {
        let args = Args::try_parse_from(["ssh-tui-rs", "--no-network-check"]).unwrap();
        assert!(args.no_network_check);
        assert!(!args.embedded_ssh);

        let args = Args::try_parse_from(["ssh-tui-rs", "--embedded-ssh"]).unwrap();
        assert!(args.embedded_ssh);

        assert!(Args::try_parse_from(["ssh-tui-rs", "--browse-only"]).is_err());
    }

    #[test]
    fn double_click_requires_the_same_host_within_the_interval() {
        let start = Instant::now();
        let mut previous = None;

        assert!(!register_host_click(&mut previous, start, 1));
        assert!(!register_host_click(
            &mut previous,
            start + Duration::from_millis(100),
            2
        ));
        assert!(!register_host_click(
            &mut previous,
            start + DOUBLE_CLICK_INTERVAL + Duration::from_millis(101),
            2
        ));
        assert!(register_host_click(
            &mut previous,
            start + DOUBLE_CLICK_INTERVAL + Duration::from_millis(200),
            2
        ));
        assert!(previous.is_none());
    }
}
