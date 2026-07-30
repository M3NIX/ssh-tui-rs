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

    let (marker, marker_style, name_style) = match node.kind {
        NodeKind::Root => ("  ", Style::default(), Style::default().fg(Color::White)),
        NodeKind::Folder => {
            let marker = if app.expanded.contains(&node.id) || !app.search.is_empty() {
                "▾ "
            } else {
                "▸ "
            };
            (
                marker,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        }
        NodeKind::Host(_) => (
            "● ",
            Style::default().fg(Color::Green),
            Style::default().fg(Color::White),
        ),
    };
    spans.push(Span::styled(marker, marker_style));
    spans.extend(highlighted_name(
        app.display_name(node),
        &row.matched_indices,
        selected,
        name_style,
    ));

    if selected {
        Line::from(spans).style(Style::default().bg(Color::Rgb(31, 47, 53)))
    } else {
        Line::from(spans)
    }
}

fn highlighted_name(
    name: &str,
    matched: &[usize],
    selected: bool,
    base: Style,
) -> Vec<Span<'static>> {
    let matched = matched.iter().copied().collect::<HashSet<_>>();
    let mut spans = Vec::new();
    let base = if selected {
        base.add_modifier(Modifier::BOLD)
    } else {
        base
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
    let Some(node) = app.selected_node() else {
        frame.render_widget(
            Paragraph::new("No hosts found")
                .block(Block::default().borders(Borders::ALL).title("Details")),
            area,
        );
        return;
    };

    match node.kind {
        NodeKind::Root | NodeKind::Folder => render_folder_details(frame, app, node, area),
        NodeKind::Host(host_index) => {
            render_host_details(frame, &app.config.hosts[host_index], area)
        }
    }
}

fn render_folder_details(frame: &mut Frame<'_>, app: &App, node: &Node, area: Rect) {
    let path = node_path(app, node.id);
    let (folders, hosts) = count_descendants(app, node.id);
    let mut overview = vec![
        Line::from(Span::styled(
            node.name.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        field_line("Path", if path.is_empty() { "/" } else { &path }),
    ];
    if let Some(description) = &node.description {
        overview.push(field_line("Description", description));
    }
    if folders > 0 {
        overview.push(field_line("Subfolders", &folders.to_string()));
    }

    let overview_height = section_height(&overview, area.width);
    let panes = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(overview_height), Constraint::Min(3)])
        .split(area);

    frame.render_widget(
        Paragraph::new(overview)
            .block(Block::default().borders(Borders::ALL).title("Folder"))
            .wrap(Wrap { trim: false }),
        panes[0],
    );

    let items = descendant_hosts(app, node.id)
        .into_iter()
        .map(|host| folder_host_line(host, &node_path_parts(app, node.id)))
        .map(ListItem::new)
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Hosts ({hosts})")),
        ),
        panes[1],
    );
}

fn render_host_details(frame: &mut Frame<'_>, host: &crate::HostEntry, area: Rect) {
    let mut overview = vec![Line::from(Span::styled(
        host.alias.clone(),
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    ))];
    if !host.group_path.is_empty() {
        overview.push(field_line("Group", &host.group_path.join("/")));
    }
    if let Some(description) = &host.description {
        overview.push(field_line("Description", description));
    }

    let connection = connection_lines(host);
    let configuration = configuration_lines(host);
    let mut constraints = vec![Constraint::Length(section_height(&overview, area.width))];
    if !connection.is_empty() {
        constraints.push(Constraint::Length(section_height(&connection, area.width)));
    }
    constraints.push(Constraint::Min(3));
    let panes = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut pane = 0;
    frame.render_widget(
        Paragraph::new(overview)
            .block(Block::default().borders(Borders::ALL).title("Host"))
            .wrap(Wrap { trim: false }),
        panes[pane],
    );
    pane += 1;

    if !connection.is_empty() {
        frame.render_widget(
            Paragraph::new(connection)
                .block(Block::default().borders(Borders::ALL).title("Connection"))
                .wrap(Wrap { trim: false }),
            panes[pane],
        );
        pane += 1;
    }

    frame.render_widget(
        Paragraph::new(configuration)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Configuration"),
            )
            .wrap(Wrap { trim: false }),
        panes[pane],
    );
}

fn connection_lines(host: &crate::HostEntry) -> Vec<Line<'static>> {
    let resolved = &host.resolved;
    let mut lines = Vec::new();
    if let Some(host_name) = &resolved.host_name {
        lines.push(field_line("HostName", host_name));
    }
    if let Some(user) = &resolved.user {
        lines.push(field_line("User", user));
    }
    if let Some(port) = resolved.port {
        lines.push(field_line("Port", &port.to_string()));
    }
    if !resolved.proxy_jump.is_empty() {
        lines.push(field_line("ProxyJump", &resolved.proxy_jump.join(", ")));
    }
    if !resolved.identity_files.is_empty() {
        lines.push(field_line(
            "IdentityFile",
            &resolved
                .identity_files
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    lines
}

fn configuration_lines(host: &crate::HostEntry) -> Vec<Line<'static>> {
    let mut lines = vec![field_line(
        "Source",
        &format!("{}:{}", host.source.display(), host.line),
    )];
    for (key, values) in &host.options {
        if !is_connection_option(key) {
            lines.push(field_line(key, &values.join(" ")));
        }
    }
    lines
}

fn is_connection_option(key: &str) -> bool {
    ["HostName", "User", "Port", "ProxyJump", "IdentityFile"]
        .iter()
        .any(|known| key.eq_ignore_ascii_case(known))
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

fn section_height(lines: &[Line<'_>], width: u16) -> u16 {
    let inner_width = usize::from(width.saturating_sub(2).max(1));
    let content_height = lines
        .iter()
        .map(|line| line.width().div_ceil(inner_width).max(1))
        .sum::<usize>();
    u16::try_from(content_height.saturating_add(2)).unwrap_or(u16::MAX)
}

fn descendant_hosts(app: &App, node_id: usize) -> Vec<&crate::HostEntry> {
    fn collect<'a>(app: &'a App, node_id: usize, hosts: &mut Vec<&'a crate::HostEntry>) {
        for child in &app.nodes[node_id].children {
            match app.nodes[*child].kind {
                NodeKind::Folder | NodeKind::Root => collect(app, *child, hosts),
                NodeKind::Host(host_index) => hosts.push(&app.config.hosts[host_index]),
            }
        }
    }

    let mut hosts = Vec::new();
    collect(app, node_id, &mut hosts);
    hosts.sort_by(|left, right| {
        left.alias
            .to_lowercase()
            .cmp(&right.alias.to_lowercase())
            .then_with(|| left.alias.cmp(&right.alias))
    });
    hosts
}

fn folder_host_line(host: &crate::HostEntry, selected_path: &[String]) -> Line<'static> {
    let mut spans = vec![
        Span::styled("● ", Style::default().fg(Color::Green)),
        Span::styled(
            host.alias.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    let relative_group = host
        .group_path
        .strip_prefix(selected_path)
        .unwrap_or(&host.group_path);
    if !relative_group.is_empty() {
        spans.push(Span::styled(
            format!("  {}", relative_group.join("/")),
            Style::default().fg(Color::Cyan),
        ));
    }
    if let Some(host_name) = &host.resolved.host_name {
        spans.push(Span::styled(
            format!("  {host_name}"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

fn node_path(app: &App, node_id: usize) -> String {
    node_path_parts(app, node_id).join("/")
}

fn node_path_parts(app: &App, node_id: usize) -> Vec<String> {
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
    parts
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
    use std::{collections::BTreeMap, path::PathBuf};

    use ratatui::{Terminal, backend::TestBackend};

    use crate::{GroupEntry, ResolvedHost, ssh_config::HostEntry};

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
        assert!(!rendered.contains("(not set)"));
    }

    #[test]
    fn collapsed_folder_details_list_descendant_hosts_alphabetically() {
        let config = crate::SshConfig {
            source: "config".into(),
            groups: vec![GroupEntry {
                path: vec!["Production".into(), "Databases".into()],
                description: Some("Persistent storage".into()),
                source: PathBuf::from("config"),
                line: 1,
            }],
            hosts: vec![
                HostEntry {
                    alias: "zeta-db".into(),
                    description: None,
                    group_path: vec!["Production".into(), "Databases".into()],
                    source: PathBuf::from("config"),
                    line: 3,
                    options: BTreeMap::new(),
                    resolved: ResolvedHost::default(),
                },
                HostEntry {
                    alias: "alpha-api".into(),
                    description: None,
                    group_path: vec!["Production".into()],
                    source: PathBuf::from("config"),
                    line: 5,
                    options: BTreeMap::new(),
                    resolved: ResolvedHost::default(),
                },
            ],
        };
        let mut app = App::new(config);
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(app.expanded.is_empty());
        assert!(rendered.contains("Hosts (2)"));
        assert!(rendered.contains("alpha-api"));
        assert!(rendered.contains("zeta-db"));
        assert!(rendered.find("alpha-api") < rendered.find("zeta-db"));
    }
}
