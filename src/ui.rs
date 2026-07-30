use std::collections::HashSet;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::{App, Node, NodeKind, VisibleRow};

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let root = frame.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(root);

    render_header(frame, app, layout[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(layout[1]);

    render_tree(frame, app, body[0]);
    render_details(frame, app, body[1]);
    render_footer(frame, app, layout[2]);
}

fn render_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let search = if app.search.is_empty() {
        "Search: /".to_string()
    } else {
        format!("Search: {}", app.search)
    };
    let title = Line::from(vec![
        Span::styled(
            "ssh-tui",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(search, Style::default().fg(Color::Yellow)),
        Span::raw("  "),
        Span::styled(
            format!("{} hosts", app.config.hosts.len()),
            Style::default().fg(Color::Gray),
        ),
    ]);
    let block = Block::default().borders(Borders::ALL).title("Connections");
    frame.render_widget(Paragraph::new(title).block(block), area);
}

fn render_tree(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title("Tree");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.set_tree_area(inner.y, inner.height);

    let rows = app
        .visible
        .iter()
        .skip(app.visible_offset)
        .take(usize::from(inner.height))
        .enumerate()
        .map(|(visible_position, row)| {
            let absolute_index = app.visible_offset + visible_position;
            ListItem::new(render_tree_row(app, row, absolute_index == app.selected))
        })
        .collect::<Vec<_>>();

    frame.render_widget(List::new(rows), inner);
}

fn render_tree_row(app: &App, row: &VisibleRow, selected: bool) -> Line<'static> {
    let node = &app.nodes[row.node_id];
    let mut spans = Vec::new();
    spans.push(Span::raw("  ".repeat(row.depth)));

    let marker = match node.kind {
        NodeKind::Root => "  ",
        NodeKind::Folder => {
            if app.expanded.contains(&node.id) || !app.search.is_empty() {
                "[-] "
            } else {
                "[+] "
            }
        }
        NodeKind::Host(_) => "> ",
    };
    spans.push(Span::styled(marker, Style::default().fg(Color::DarkGray)));
    spans.extend(highlighted_name(
        app.display_name(node),
        &row.matched_indices,
        selected,
    ));

    if selected {
        Line::from(spans).style(Style::default().bg(Color::Rgb(31, 47, 53)))
    } else {
        Line::from(spans)
    }
}

fn highlighted_name(name: &str, matched: &[usize], selected: bool) -> Vec<Span<'static>> {
    let matched = matched.iter().copied().collect::<HashSet<_>>();
    let mut spans = Vec::new();
    let base = if selected {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let highlight = Style::default()
        .fg(Color::Black)
        .bg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    for (index, character) in name.chars().enumerate() {
        let style = if matched.contains(&index) {
            highlight
        } else {
            base
        };
        spans.push(Span::styled(character.to_string(), style));
    }
    spans
}

fn render_details(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title("Details");
    let Some(node) = app.selected_node() else {
        frame.render_widget(Paragraph::new("No hosts found").block(block), area);
        return;
    };

    let lines = match node.kind {
        NodeKind::Root | NodeKind::Folder => folder_details(app, node),
        NodeKind::Host(host_index) => host_details(app, &app.config.hosts[host_index]),
    };

    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn folder_details(app: &App, node: &Node) -> Vec<Line<'static>> {
    let path = node_path(app, node.id);
    let (folders, hosts) = count_descendants(app, node.id);
    let mut lines = vec![
        Line::from(Span::styled(
            node.name.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        field_line("Path", if path.is_empty() { "/" } else { &path }),
        field_line("Folders", &folders.to_string()),
        field_line("Hosts", &hosts.to_string()),
    ];

    if let Some(description) = &node.description {
        lines.push(Line::from(""));
        lines.push(field_line("Description", description));
    }
    lines
}

fn host_details(app: &App, host: &crate::HostEntry) -> Vec<Line<'static>> {
    let group = if host.group_path.is_empty() {
        "/".to_string()
    } else {
        host.group_path.join("/")
    };
    let resolved = &host.resolved;
    let mut lines = vec![
        Line::from(Span::styled(
            host.alias.clone(),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        field_line("Host", &host.alias),
        field_line(
            "HostName",
            resolved.host_name.as_deref().unwrap_or("(not set)"),
        ),
        field_line("User", resolved.user.as_deref().unwrap_or("(not set)")),
        field_line(
            "Port",
            &resolved
                .port
                .map(|port| port.to_string())
                .unwrap_or_else(|| "(not set)".to_string()),
        ),
        field_line("ProxyJump", &empty_or_joined(&resolved.proxy_jump, ", ")),
        field_line(
            "IdentityFile",
            &empty_or_joined(
                &resolved
                    .identity_files
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>(),
                ", ",
            ),
        ),
        field_line("Group", &group),
        field_line(
            "Source",
            &format!("{}:{}", host.source.display(), host.line),
        ),
    ];

    if let Some(description) = &host.description {
        lines.push(Line::from(""));
        lines.push(field_line("Description", description));
    }

    if !host.options.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Block options",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        for (key, values) in &host.options {
            lines.push(field_line(key, &values.join(" ")));
        }
    }

    if app.search.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Enter connects with ssh. Space folds folders.",
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines
}

fn render_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    frame.render_widget(Clear, area);
    let mode = match app.input_mode {
        crate::InputMode::Normal => "q quit  / search  arrows/jk move  h/l fold  Enter connect",
        crate::InputMode::Search => "Esc clear  Enter keep search  Backspace edit",
    };
    let text = if app.status.is_empty() {
        mode.to_string()
    } else {
        format!("{}  |  {}", app.status, mode)
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn field_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<12}"),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_string(), Style::default().fg(Color::White)),
    ])
}

fn empty_or_joined(values: &[String], separator: &str) -> String {
    if values.is_empty() {
        "(not set)".to_string()
    } else {
        values.join(separator)
    }
}

fn node_path(app: &App, node_id: usize) -> String {
    let mut parts = Vec::new();
    let mut current = Some(node_id);
    while let Some(id) = current {
        let node = &app.nodes[id];
        if !matches!(node.kind, NodeKind::Root) {
            parts.push(node.name.clone());
        }
        current = node.parent;
    }
    parts.reverse();
    parts.join("/")
}

fn count_descendants(app: &App, node_id: usize) -> (usize, usize) {
    let mut folders = 0;
    let mut hosts = 0;
    for child in &app.nodes[node_id].children {
        match app.nodes[*child].kind {
            NodeKind::Folder => {
                folders += 1;
                let (sub_folders, sub_hosts) = count_descendants(app, *child);
                folders += sub_folders;
                hosts += sub_hosts;
            }
            NodeKind::Host(_) => hosts += 1,
            NodeKind::Root => {}
        }
    }
    (folders, hosts)
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use crate::{ResolvedHost, ssh_config::HostEntry};

    use super::*;

    #[test]
    fn draws_without_panicking() {
        let config = crate::SshConfig {
            source: "config".into(),
            groups: Vec::new(),
            hosts: vec![HostEntry {
                alias: "bastion".into(),
                description: Some("Entry host".into()),
                group_path: Vec::new(),
                source: "config".into(),
                line: 1,
                options: Default::default(),
                resolved: ResolvedHost {
                    host_name: Some("bastion.example.test".into()),
                    user: Some("deploy".into()),
                    port: Some(22),
                    identity_files: Vec::new(),
                    proxy_jump: Vec::new(),
                },
            }],
        };
        let mut app = App::new(config);
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("bastion"));
        assert!(rendered.contains("HostName"));
    }
}
