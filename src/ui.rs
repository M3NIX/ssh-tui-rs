use std::collections::HashSet;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, Wrap},
};
use unicode_width::UnicodeWidthStr;

use crate::{App, ConnectionFailure, HostReachability, Node, NodeKind, VisibleRow};

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
        .constraints([Constraint::Ratio(1, 3), Constraint::Ratio(2, 3)])
        .split(layout[1]);

    render_tree(frame, app, body[0]);
    render_details(frame, app, body[1]);
    render_footer(frame, app, layout[2]);
    if let Some(failure) = &app.connection_failure {
        render_connection_failure(frame, failure);
    }
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
        NodeKind::Host(host_index) => (
            "● ",
            Style::default().fg(reachability_color(app.host_reachability(host_index))),
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
    let base = if selected {
        base.add_modifier(Modifier::BOLD)
    } else {
        base
    };
    highlighted_chars(name, matched, base)
}

fn highlighted_chars(value: &str, matched: &[usize], base: Style) -> Vec<Span<'static>> {
    let matched = matched.iter().copied().collect::<HashSet<_>>();
    let mut spans = Vec::new();
    let highlight = Style::default()
        .fg(Color::Black)
        .bg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    for (index, character) in value.chars().enumerate() {
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
        NodeKind::Host(host_index) => render_host_details(
            frame,
            &app.config.hosts[host_index],
            app.host_reachability(host_index),
            &app.search,
            area,
        ),
    }
}

fn render_folder_details(frame: &mut Frame<'_>, app: &App, node: &Node, area: Rect) {
    let path = node_path(app, node.id);
    let (folders, hosts) = count_descendants(app, node.id);
    let mut overview = vec![
        Line::from(fuzzy_highlighted(
            &node.name,
            &app.search,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        search_field_line(
            "Path",
            if path.is_empty() { "/" } else { &path },
            &app.search,
        ),
    ];
    if let Some(description) = &node.description {
        overview.push(search_field_line("Description", description, &app.search));
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

    let hosts_in_folder = descendant_hosts(app, node.id);
    let selected_path = node_path_parts(app, node.id);
    let column_widths = folder_column_widths(&hosts_in_folder, &selected_path, panes[1].width);
    let rows = hosts_in_folder
        .into_iter()
        .map(|(_, host)| folder_host_row(host, &selected_path, app.search.trim()))
        .collect::<Vec<_>>();
    frame.render_widget(
        Table::new(rows, column_widths)
            .header(
                Row::new(["Alias", "HostName", "Description"]).style(
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                ),
            )
            .column_spacing(1)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!("Hosts ({hosts})")),
            ),
        panes[1],
    );
}

fn render_host_details(
    frame: &mut Frame<'_>,
    host: &crate::HostEntry,
    reachability: HostReachability,
    query: &str,
    area: Rect,
) {
    let mut title = vec![host_dot(reachability)];
    title.extend(fuzzy_highlighted(
        &host.alias,
        query,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    ));
    let mut overview = vec![Line::from(title)];
    if !host.group_path.is_empty() {
        overview.push(search_field_line(
            "Group",
            &host.group_path.join("/"),
            query,
        ));
    }
    if let Some(description) = &host.description {
        overview.push(search_field_line("Description", description, query));
    }

    let connection = connection_lines(host, query);
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

fn connection_lines(host: &crate::HostEntry, query: &str) -> Vec<Line<'static>> {
    let resolved = &host.resolved;
    let mut lines = Vec::new();
    if let Some(host_name) = &resolved.host_name {
        lines.push(search_field_line("HostName", host_name, query));
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

fn render_connection_failure(frame: &mut Frame<'_>, failure: &ConnectionFailure) {
    let screen = frame.area();
    if screen.width == 0 || screen.height == 0 {
        return;
    }

    let mut lines = vec![
        Line::from(Span::styled(
            failure.alias.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    lines.extend(
        failure
            .message
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), Color::White))),
    );
    lines.push(Line::from(""));
    if let Some(exit_status) = failure.exit_status {
        lines.push(field_line("Exit status", &exit_status.to_string()));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter or Esc to close",
        Style::default().fg(Color::DarkGray),
    )));

    let width = screen.width.saturating_sub(2).clamp(1, 76);
    let available_height = screen.height.saturating_sub(2).max(1);
    let height = section_height(&lines, width).min(available_height);
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Red))
                    .title(Span::styled(
                        " Connection failed ",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    )),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn field_line(label: &str, value: &str) -> Line<'static> {
    field_line_with_spans(
        label,
        vec![Span::styled(
            value.to_string(),
            Style::default().fg(Color::White),
        )],
    )
}

fn search_field_line(label: &str, value: &str, query: &str) -> Line<'static> {
    field_line_with_spans(
        label,
        fuzzy_highlighted(value, query, Style::default().fg(Color::White)),
    )
}

fn field_line_with_spans(label: &str, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{label:<12}"),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )];
    spans.extend(value);
    Line::from(spans)
}

fn fuzzy_highlighted(value: &str, query: &str, base: Style) -> Vec<Span<'static>> {
    let matched = fuzzy_indices(value, query).unwrap_or_default();
    highlighted_chars(value, &matched, base)
}

fn fuzzy_indices(value: &str, query: &str) -> Option<Vec<usize>> {
    crate::search::fuzzy_indices(value, query)
}

fn section_height(lines: &[Line<'_>], width: u16) -> u16 {
    let inner_width = usize::from(width.saturating_sub(2).max(1));
    let content_height = lines
        .iter()
        .map(|line| line.width().div_ceil(inner_width).max(1))
        .sum::<usize>();
    u16::try_from(content_height.saturating_add(2)).unwrap_or(u16::MAX)
}

fn descendant_hosts(app: &App, node_id: usize) -> Vec<(usize, &crate::HostEntry)> {
    fn collect<'a>(app: &'a App, node_id: usize, hosts: &mut Vec<(usize, &'a crate::HostEntry)>) {
        for child in &app.nodes[node_id].children {
            match app.nodes[*child].kind {
                NodeKind::Folder | NodeKind::Root => collect(app, *child, hosts),
                NodeKind::Host(host_index) => {
                    hosts.push((host_index, &app.config.hosts[host_index]));
                }
            }
        }
    }

    let mut hosts = Vec::new();
    collect(app, node_id, &mut hosts);
    hosts.sort_by(|left, right| {
        left.1
            .group_path
            .iter()
            .map(|segment| segment.to_lowercase())
            .cmp(
                right
                    .1
                    .group_path
                    .iter()
                    .map(|segment| segment.to_lowercase()),
            )
            .then_with(|| left.1.group_path.cmp(&right.1.group_path))
            .then_with(|| {
                left.1
                    .alias
                    .to_lowercase()
                    .cmp(&right.1.alias.to_lowercase())
            })
            .then_with(|| left.1.alias.cmp(&right.1.alias))
    });
    hosts
}

fn folder_column_widths(
    hosts: &[(usize, &crate::HostEntry)],
    selected_path: &[String],
    area_width: u16,
) -> [Constraint; 3] {
    let available = area_width.saturating_sub(4);
    let natural_alias_width = hosts
        .iter()
        .map(|(_, host)| folder_alias(host, selected_path).width())
        .max()
        .unwrap_or_default()
        .max("Alias".len());
    let natural_hostname_width = hosts
        .iter()
        .filter_map(|(_, host)| host.resolved.host_name.as_deref())
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or_default()
        .max("HostName".len());
    let alias_width = u16::try_from(natural_alias_width)
        .unwrap_or(u16::MAX)
        .min(available.saturating_mul(2) / 5);
    let hostname_width = u16::try_from(natural_hostname_width)
        .unwrap_or(u16::MAX)
        .min(available.saturating_mul(3) / 10);

    [
        Constraint::Length(alias_width),
        Constraint::Length(hostname_width),
        Constraint::Fill(1),
    ]
}

fn folder_host_row(host: &crate::HostEntry, selected_path: &[String], query: &str) -> Row<'static> {
    let host_name = host.resolved.host_name.as_deref().unwrap_or_default();
    let description = host.description.as_deref().unwrap_or_default();
    let relative_group = host
        .group_path
        .strip_prefix(selected_path)
        .unwrap_or(&host.group_path);
    let mut alias = Vec::new();
    if !relative_group.is_empty() {
        alias.extend(fuzzy_highlighted(
            &relative_group.join("/"),
            query,
            Style::default().fg(Color::Cyan),
        ));
        alias.push(Span::styled("/", Style::default().fg(Color::DarkGray)));
    }
    alias.extend(fuzzy_highlighted(
        &host.alias,
        query,
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    ));

    Row::new([
        Cell::from(Line::from(alias)),
        Cell::from(Line::from(fuzzy_highlighted(
            host_name,
            query,
            Style::default().fg(Color::White),
        ))),
        Cell::from(Line::from(fuzzy_highlighted(
            description,
            query,
            Style::default().fg(Color::DarkGray),
        ))),
    ])
}

fn folder_alias(host: &crate::HostEntry, selected_path: &[String]) -> String {
    let relative_group = host
        .group_path
        .strip_prefix(selected_path)
        .unwrap_or(&host.group_path);
    if relative_group.is_empty() {
        host.alias.clone()
    } else {
        format!("{}/{}", relative_group.join("/"), host.alias)
    }
}

fn host_dot(reachability: HostReachability) -> Span<'static> {
    Span::styled(
        "● ",
        Style::default()
            .fg(reachability_color(reachability))
            .add_modifier(Modifier::BOLD),
    )
}

fn reachability_color(reachability: HostReachability) -> Color {
    match reachability {
        HostReachability::Unchecked => Color::DarkGray,
        HostReachability::Checking => Color::Yellow,
        HostReachability::Reachable => Color::Green,
        HostReachability::Unreachable => Color::Red,
    }
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

    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    use crate::{GroupEntry, ResolvedHost, ssh_config::HostEntry};

    use super::*;

    fn highlighted_text(buffer: &Buffer) -> String {
        buffer
            .content
            .iter()
            .filter(|cell| cell.bg == Color::Yellow)
            .map(|cell| cell.symbol())
            .collect()
    }

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
        let header = buffer
            .content
            .iter()
            .take(usize::from(buffer.area.width * 3))
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!header.contains("1 hosts"));
        let checking_dots = buffer
            .content
            .iter()
            .filter(|cell| cell.symbol() == "●" && cell.fg == Color::Yellow)
            .count();
        assert_eq!(checking_dots, 2);
    }

    #[test]
    fn highlights_description_and_hostname_matches_in_host_details() {
        let config = crate::SshConfig {
            source: "config".into(),
            groups: Vec::new(),
            hosts: vec![HostEntry {
                alias: "bastion".into(),
                description: Some("Primary entry point".into()),
                group_path: Vec::new(),
                source: "config".into(),
                line: 1,
                options: Default::default(),
                resolved: ResolvedHost {
                    host_name: Some("gateway.internal.test".into()),
                    ..ResolvedHost::default()
                },
            }],
        };
        let mut app = App::new(config);
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        app.search = "primary".into();
        app.rebuild_visible();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert!(highlighted_text(terminal.backend().buffer()).contains("Primary"));

        app.search = "internal".into();
        app.rebuild_visible();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert!(highlighted_text(terminal.backend().buffer()).contains("internal"));
    }

    #[test]
    fn unreachable_hosts_use_red_dots() {
        let config = crate::SshConfig {
            source: "config".into(),
            groups: Vec::new(),
            hosts: vec![HostEntry {
                alias: "offline".into(),
                description: None,
                group_path: Vec::new(),
                source: "config".into(),
                line: 1,
                options: Default::default(),
                resolved: ResolvedHost::default(),
            }],
        };
        let mut app = App::new(config);
        app.reachability.insert(0, HostReachability::Unreachable);
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let red_dots = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .filter(|cell| cell.symbol() == "●" && cell.fg == Color::Red)
            .count();

        assert_eq!(red_dots, 2);
    }

    #[test]
    fn draws_connection_failure_dialog() {
        let config = crate::SshConfig {
            source: "config".into(),
            groups: Vec::new(),
            hosts: Vec::new(),
        };
        let mut app = App::new(config);
        app.show_connection_failure(ConnectionFailure {
            alias: "test-web".into(),
            message: "ssh: Could not resolve hostname test1 because the configured DNS name is intentionally long enough to wrap across terminal rows".into(),
            exit_status: Some(255),
        });
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("Connection failed"));
        assert!(rendered.contains("test-web"));
        assert!(rendered.contains("Could not resolve hostname"));
        assert!(rendered.contains("255"));
        assert!(rendered.contains("Enter or Esc to close"));
    }

    #[test]
    fn collapsed_folder_details_group_hosts_by_subfolder_then_alias() {
        let config = crate::SshConfig {
            source: "config".into(),
            groups: vec![GroupEntry {
                path: vec!["Production".into(), "Databases".into()],
                description: Some("Persistent storage".into()),
                expanded_by_default: false,
                source: PathBuf::from("config"),
                line: 1,
            }],
            hosts: vec![
                HostEntry {
                    alias: "zeta-db".into(),
                    description: Some("Primary database".into()),
                    group_path: vec!["Production".into(), "Databases".into()],
                    source: PathBuf::from("config"),
                    line: 3,
                    options: BTreeMap::new(),
                    resolved: ResolvedHost {
                        host_name: Some("db.internal".into()),
                        ..ResolvedHost::default()
                    },
                },
                HostEntry {
                    alias: "alpha-api".into(),
                    description: Some("Public API".into()),
                    group_path: vec!["Production".into()],
                    source: PathBuf::from("config"),
                    line: 5,
                    options: BTreeMap::new(),
                    resolved: ResolvedHost {
                        host_name: Some("api.internal".into()),
                        ..ResolvedHost::default()
                    },
                },
                HostEntry {
                    alias: "zulu-app".into(),
                    description: None,
                    group_path: vec!["Production".into(), "Applications".into()],
                    source: PathBuf::from("config"),
                    line: 7,
                    options: BTreeMap::new(),
                    resolved: ResolvedHost::default(),
                },
                HostEntry {
                    alias: "alpha-db".into(),
                    description: None,
                    group_path: vec!["Production".into(), "Databases".into()],
                    source: PathBuf::from("config"),
                    line: 9,
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
        assert!(rendered.contains("Hosts (4)"));
        assert!(rendered.contains("Alias"));
        assert!(rendered.contains("HostName"));
        assert!(rendered.contains("Description"));
        assert!(rendered.contains("alpha-api"));
        assert!(rendered.contains("zeta-db"));
        assert!(rendered.contains("Primary database"));
        assert!(!rendered.contains('●'));
        let folder_id = app.visible[0].node_id;
        let aliases = descendant_hosts(&app, folder_id)
            .into_iter()
            .map(|(_, host)| host.alias.as_str())
            .collect::<Vec<_>>();
        assert_eq!(aliases, ["alpha-api", "zulu-app", "alpha-db", "zeta-db"]);
        let rendered_rows = terminal
            .backend()
            .buffer()
            .content
            .chunks(100)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>();
        let alpha_row = rendered_rows
            .iter()
            .find(|row| row.contains("alpha-api"))
            .expect("alpha-api row");
        let zeta_row = rendered_rows
            .iter()
            .find(|row| row.contains("zeta-db"))
            .expect("zeta-db row");
        assert!(zeta_row.contains("Databases/zeta-db"));
        assert_eq!(alpha_row.find("api.internal"), zeta_row.find("db.internal"));
        assert_eq!(
            alpha_row.find("Public API"),
            zeta_row.find("Primary database")
        );

        app.search = "primary".into();
        app.rebuild_visible();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("Primary database"));
        assert!(highlighted_text(buffer).contains("Primary"));
    }
}
