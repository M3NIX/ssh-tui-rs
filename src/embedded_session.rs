use std::{ffi::OsString, path::Path};

use crossterm::event::{KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use tastty::{Builder, ExitStatus, ManagedTerminal, Position, Terminal, TerminalSize};

const SCROLLBACK_ROWS: u32 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedFocus {
    Terminal,
    Tree,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddedExit {
    Failed(ExitStatus),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddedPoll {
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbeddedMouseAction {
    Ignored,
    Handled,
    Copy(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalSelection {
    start: Position,
    end: Position,
    dragging: bool,
}

#[derive(Debug)]
pub struct EmbeddedSession {
    pub alias: String,
    pub terminal: ManagedTerminal,
    pub focus: EmbeddedFocus,
    pub exit: Option<EmbeddedExit>,
    size: TerminalSize,
    selection: Option<TerminalSelection>,
}

impl EmbeddedSession {
    pub fn spawn_ssh(alias: &str, config: &Path, rows: u16, cols: u16) -> tastty::Result<Self> {
        Self::spawn_command("ssh", ssh_arguments(config, alias), alias, rows, cols)
    }

    fn spawn_command(
        program: &str,
        arguments: impl IntoIterator<Item = OsString>,
        alias: &str,
        rows: u16,
        cols: u16,
    ) -> tastty::Result<Self> {
        let size = terminal_size(rows, cols);
        let terminal = Terminal::spawn(
            Builder::command(program)
                .args(arguments)
                .env("TERM", "xterm-256color")
                .env("COLORTERM", "truecolor")
                .size(size)
                .scrollback(SCROLLBACK_ROWS)
                .echo(true),
        )?;
        Ok(Self {
            alias: alias.to_string(),
            terminal,
            focus: EmbeddedFocus::Terminal,
            exit: None,
            size,
            selection: None,
        })
    }

    pub fn poll(&mut self) -> EmbeddedPoll {
        if self.exit.is_some() {
            return EmbeddedPoll::Failed;
        }
        match self.terminal.try_wait() {
            Ok(None) => EmbeddedPoll::Running,
            Ok(Some(status)) if status.success() => EmbeddedPoll::Succeeded,
            Ok(Some(status)) => {
                self.exit = Some(EmbeddedExit::Failed(status));
                self.focus = EmbeddedFocus::Tree;
                EmbeddedPoll::Failed
            }
            Err(error) => {
                self.exit = Some(EmbeddedExit::Error(error.to_string()));
                self.focus = EmbeddedFocus::Tree;
                EmbeddedPoll::Failed
            }
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> tastty::Result<()> {
        let size = terminal_size(rows, cols);
        if size == self.size {
            return Ok(());
        }
        self.terminal.resize(size)?;
        self.size = size;
        Ok(())
    }

    pub fn send_key(&self, key: KeyEvent) -> tastty::Result<()> {
        self.terminal.send_key(key)
    }

    pub fn send_paste(&self, text: &str) -> tastty::Result<()> {
        self.terminal.send_paste(text)
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> tastty::Result<EmbeddedMouseAction> {
        let position = self.clamp_position(mouse.row, mouse.column);
        let selecting = self.selection.is_some_and(|selection| selection.dragging);
        let override_guest = mouse.modifiers.contains(KeyModifiers::SHIFT) || !self.reports_mouse();

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if override_guest => {
                self.selection = Some(TerminalSelection {
                    start: position,
                    end: position,
                    dragging: true,
                });
                Ok(EmbeddedMouseAction::Handled)
            }
            MouseEventKind::Drag(MouseButton::Left) if selecting => {
                if let Some(selection) = &mut self.selection {
                    selection.end = position;
                }
                Ok(EmbeddedMouseAction::Handled)
            }
            MouseEventKind::Up(MouseButton::Left) if selecting => {
                if let Some(selection) = &mut self.selection {
                    selection.end = position;
                    selection.dragging = false;
                }
                Ok(self
                    .selected_text()
                    .filter(|text| !text.is_empty())
                    .map_or(EmbeddedMouseAction::Handled, EmbeddedMouseAction::Copy))
            }
            _ if self.is_running() => {
                self.selection = None;
                self.terminal.send_mouse(mouse)?;
                Ok(EmbeddedMouseAction::Handled)
            }
            _ => Ok(EmbeddedMouseAction::Ignored),
        }
    }

    pub fn send_focus(&self, gained: bool) -> tastty::Result<()> {
        self.terminal.send_focus(gained)
    }

    pub fn scroll_up(&self, rows: usize) {
        self.terminal.scroll_up(rows);
    }

    pub fn scroll_down(&self, rows: usize) {
        self.terminal.scroll_down(rows);
    }

    pub fn reports_mouse(&self) -> bool {
        self.terminal.any_mouse_mode()
    }

    pub fn is_running(&self) -> bool {
        self.exit.is_none()
    }

    pub fn exit_label(&self) -> Option<String> {
        match &self.exit {
            Some(EmbeddedExit::Failed(status)) => status
                .signal()
                .map(|signal| format!("signal {signal}"))
                .or_else(|| Some(format!("exit {}", status.exit_code()))),
            Some(EmbeddedExit::Error(error)) => Some(error.clone()),
            None => None,
        }
    }

    pub fn selection_range(&self) -> Option<(Position, Position)> {
        self.selection.map(|selection| {
            if (selection.start.row, selection.start.col) <= (selection.end.row, selection.end.col)
            {
                (selection.start, selection.end)
            } else {
                (selection.end, selection.start)
            }
        })
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection_range()?;
        let text = self
            .terminal
            .with_screen(|screen| screen.contents_between(start, end));
        Some(
            text.lines()
                .map(str::trim_end)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    fn clamp_position(&self, row: u16, col: u16) -> Position {
        Position {
            row: row.min(self.size.rows.saturating_sub(1)),
            col: col.min(self.size.cols.saturating_sub(1)),
        }
    }
}

pub fn ssh_arguments(config: &Path, alias: &str) -> Vec<OsString> {
    vec![
        OsString::from("-F"),
        config.as_os_str().to_owned(),
        OsString::from("--"),
        OsString::from(alias),
    ]
}

fn terminal_size(rows: u16, cols: u16) -> TerminalSize {
    TerminalSize {
        rows: rows.max(1),
        cols: cols.max(1),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;

    #[test]
    fn ssh_arguments_use_the_loaded_config() {
        assert_eq!(
            ssh_arguments(Path::new("/tmp/alternate-config"), "work-web"),
            [
                OsString::from("-F"),
                OsString::from("/tmp/alternate-config"),
                OsString::from("--"),
                OsString::from("work-web"),
            ]
        );
    }

    #[test]
    fn terminal_captures_output_and_reports_failure() {
        let mut session = EmbeddedSession::spawn_command(
            "/bin/sh",
            [
                OsString::from("-c"),
                OsString::from("printf 'embedded output\\n'; exit 7"),
            ],
            "test",
            10,
            40,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);

        while session.poll() == EmbeddedPoll::Running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(session.exit_label().as_deref(), Some("exit 7"));
        assert!(
            session
                .terminal
                .with_screen(|screen| screen.contents().contains("embedded output"))
        );
    }

    #[test]
    fn terminal_echoes_typed_input() {
        let session = EmbeddedSession::spawn_command(
            "/bin/sh",
            [OsString::from("-c"), OsString::from("read _line")],
            "test",
            10,
            40,
        )
        .unwrap();

        for character in "visible input".chars() {
            session
                .send_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        while !session
            .terminal
            .with_screen(|screen| screen.contents().contains("visible input"))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(
            session
                .terminal
                .with_screen(|screen| screen.contents().contains("visible input"))
        );
    }

    #[test]
    fn terminal_selection_extracts_text_in_both_directions() {
        let session = EmbeddedSession::spawn_command(
            "/bin/sh",
            [
                OsString::from("-c"),
                OsString::from("printf 'copy this'; read _line"),
            ],
            "test",
            10,
            40,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !session
            .terminal
            .with_screen(|screen| screen.contents().contains("copy this"))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut session = session;
        session.selection = Some(TerminalSelection {
            start: Position { row: 0, col: 8 },
            end: Position { row: 0, col: 5 },
            dragging: false,
        });

        assert_eq!(session.selected_text().as_deref(), Some("this"));
        assert_eq!(
            session.selection_range(),
            Some((Position { row: 0, col: 5 }, Position { row: 0, col: 8 }))
        );
    }

    #[test]
    fn mouse_drag_copies_selected_terminal_text() {
        let session = EmbeddedSession::spawn_command(
            "/bin/sh",
            [
                OsString::from("-c"),
                OsString::from("printf 'copy this'; read _line"),
            ],
            "test",
            10,
            40,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !session
            .terminal
            .with_screen(|screen| screen.contents().contains("copy this"))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut session = session;
        let mouse = |kind, column| MouseEvent {
            kind,
            column,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            session
                .handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0))
                .unwrap(),
            EmbeddedMouseAction::Handled
        );
        assert_eq!(
            session
                .handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 3))
                .unwrap(),
            EmbeddedMouseAction::Handled
        );
        assert_eq!(
            session
                .handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 3))
                .unwrap(),
            EmbeddedMouseAction::Copy("copy".to_string())
        );
    }
}
