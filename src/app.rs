use std::collections::{HashMap, HashSet};

use crossterm::event::{KeyEvent, MouseEvent, MouseEventKind};

use crate::{
    EmbeddedFocus, EmbeddedMouseAction, EmbeddedPoll, EmbeddedSession, HostEntry, HostReachability,
    SshConfig,
    reachability::{CheckTarget, ReachabilityTracker},
    search,
    tree::build_tree,
};

pub use crate::tree::{Node, NodeKind, VisibleRow};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Search,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionFailure {
    pub alias: String,
    pub message: String,
    pub exit_status: Option<i32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Area {
    left: u16,
    top: u16,
    width: u16,
    height: u16,
}

impl Area {
    fn contains(self, column: u16, row: u16) -> bool {
        column >= self.left
            && column < self.left.saturating_add(self.width)
            && row >= self.top
            && row < self.top.saturating_add(self.height)
    }

    fn inner(self) -> Self {
        Self {
            left: self.left.saturating_add(1),
            top: self.top.saturating_add(1),
            width: self.width.saturating_sub(2).max(1),
            height: self.height.saturating_sub(2).max(1),
        }
    }
}

#[derive(Debug, Default)]
struct InteractionLayout {
    search: Area,
    tree: Area,
    details: Area,
}

#[derive(Debug)]
pub struct App {
    pub config: SshConfig,
    pub nodes: Vec<Node>,
    pub root_id: usize,
    pub expanded: HashSet<usize>,
    pub selected: usize,
    pub visible: Vec<VisibleRow>,
    pub search: String,
    pub input_mode: InputMode,
    pub visible_offset: usize,
    pub status: String,
    pub reachability: HashMap<usize, HostReachability>,
    pub connection_failure: Option<ConnectionFailure>,
    pub embedded_session: Option<EmbeddedSession>,
    layout: InteractionLayout,
    embedded_ssh_enabled: bool,
    reachability_tracker: ReachabilityTracker,
}

impl App {
    #[must_use]
    pub fn new(config: SshConfig) -> Self {
        Self::with_network_checks(config, true)
    }

    #[must_use]
    pub fn with_network_checks(config: SshConfig, network_checks_enabled: bool) -> Self {
        Self::with_features(config, network_checks_enabled, false)
    }

    #[must_use]
    pub fn with_features(
        config: SshConfig,
        network_checks_enabled: bool,
        embedded_ssh_enabled: bool,
    ) -> Self {
        let (nodes, root_id, expanded, initially_expanded) = build_tree(&config);
        let mut app = Self {
            config,
            nodes,
            root_id,
            expanded,
            selected: 0,
            visible: Vec::new(),
            search: String::new(),
            input_mode: InputMode::Normal,
            visible_offset: 0,
            status: String::new(),
            reachability: HashMap::new(),
            connection_failure: None,
            embedded_session: None,
            layout: InteractionLayout::default(),
            embedded_ssh_enabled,
            reachability_tracker: ReachabilityTracker::new(network_checks_enabled),
        };
        app.rebuild_visible();
        app.status = match app.config.hosts.len() {
            1 => "1 host".to_string(),
            count => format!("{count} hosts"),
        };
        let ungrouped_hosts = app.nodes[app.root_id]
            .children
            .iter()
            .filter_map(|node_id| match app.nodes[*node_id].kind {
                NodeKind::Host(host_index) => Some(host_index),
                NodeKind::Root | NodeKind::Group => None,
            })
            .collect::<Vec<_>>();
        app.check_host_indices(ungrouped_hosts);
        for group_id in initially_expanded {
            app.check_hosts_below(group_id);
        }
        app
    }

    pub fn start_search(&mut self) {
        self.input_mode = InputMode::Search;
    }

    pub fn reveal_search_selection(&mut self) {
        let Some(node_id) = self.selected_node_id() else {
            self.clear_search();
            return;
        };

        let mut current = match self.nodes[node_id].kind {
            NodeKind::Root => None,
            NodeKind::Group => Some(node_id),
            NodeKind::Host(_) => self.nodes[node_id].parent,
        };
        let mut highest_newly_expanded = None;
        while let Some(group_id) = current {
            if group_id == self.root_id {
                break;
            }
            if self.expanded.insert(group_id) {
                highest_newly_expanded = Some(group_id);
            }
            current = self.nodes[group_id].parent;
        }

        self.search.clear();
        self.input_mode = InputMode::Normal;
        self.rebuild_visible();
        self.select_node(node_id);

        if let Some(group_id) = highest_newly_expanded {
            self.check_hosts_below(group_id);
        }
    }

    pub fn clear_search(&mut self) {
        self.search.clear();
        self.input_mode = InputMode::Normal;
        self.rebuild_visible();
    }

    pub fn push_search(&mut self, character: char) {
        self.search.push(character);
        self.rebuild_visible();
    }

    pub fn pop_search(&mut self) {
        self.search.pop();
        self.rebuild_visible();
    }

    pub fn select_next(&mut self) {
        if self.visible.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected + 1).min(self.visible.len() - 1);
        self.keep_selection_visible();
    }

    pub fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.keep_selection_visible();
    }

    pub fn toggle_selected_group(&mut self) {
        let Some(node_id) = self.selected_node_id() else {
            return;
        };
        if !matches!(self.nodes[node_id].kind, NodeKind::Group | NodeKind::Root) {
            return;
        }
        if self.expanded.insert(node_id) {
            self.check_hosts_below(node_id);
        } else {
            self.expanded.remove(&node_id);
        }
        self.rebuild_visible();
    }

    pub fn collapse_selected(&mut self) {
        let Some(node_id) = self.selected_node_id() else {
            return;
        };
        if matches!(self.nodes[node_id].kind, NodeKind::Group | NodeKind::Root)
            && self.expanded.contains(&node_id)
        {
            self.expanded.remove(&node_id);
            self.rebuild_visible();
        } else if let Some(parent) = self.nodes[node_id].parent {
            self.select_node(parent);
        }
    }

    pub fn expand_or_enter_selected(&mut self) {
        let Some(node_id) = self.selected_node_id() else {
            return;
        };
        if matches!(self.nodes[node_id].kind, NodeKind::Group | NodeKind::Root) {
            if self.expanded.insert(node_id) {
                self.check_hosts_below(node_id);
            }
            self.rebuild_visible();
        }
    }

    pub fn selected_host(&self) -> Option<&HostEntry> {
        let node = self.selected_node()?;
        match node.kind {
            NodeKind::Host(host_index) => self.config.hosts.get(host_index),
            _ => None,
        }
    }

    pub fn selected_node(&self) -> Option<&Node> {
        self.selected_node_id()
            .and_then(|node_id| self.nodes.get(node_id))
    }

    pub fn selected_node_id(&self) -> Option<usize> {
        self.visible.get(self.selected).map(|row| row.node_id)
    }

    pub fn select_at(&mut self, terminal_column: u16, terminal_row: u16) -> bool {
        if !self.layout.tree.contains(terminal_column, terminal_row) {
            return false;
        }
        let visible_index = self.visible_offset + usize::from(terminal_row - self.layout.tree.top);
        if visible_index >= self.visible.len() {
            return false;
        }
        self.selected = visible_index;
        true
    }

    pub fn click_at(&mut self, terminal_column: u16, terminal_row: u16) -> bool {
        let selected = self.select_at(terminal_column, terminal_row);
        if selected
            && self
                .selected_node_id()
                .is_some_and(|node_id| matches!(self.nodes[node_id].kind, NodeKind::Group))
        {
            self.toggle_selected_group();
        }
        selected
    }

    pub fn search_contains(&self, terminal_column: u16, terminal_row: u16) -> bool {
        self.layout.search.contains(terminal_column, terminal_row)
    }

    pub fn set_search_area(&mut self, left: u16, top: u16, width: u16, height: u16) {
        self.layout.search = Area {
            left,
            top,
            width,
            height,
        };
    }

    pub fn set_tree_area(&mut self, left: u16, top: u16, width: u16, height: u16) {
        self.layout.tree = Area {
            left,
            top,
            width,
            height,
        };
        self.keep_selection_visible();
    }

    pub fn set_details_area(&mut self, left: u16, top: u16, width: u16, height: u16) {
        self.layout.details = Area {
            left,
            top,
            width,
            height,
        };
    }

    pub fn embedded_ssh_enabled(&self) -> bool {
        self.embedded_ssh_enabled
    }

    pub fn start_embedded_session(&mut self) -> tastty::Result<()> {
        if self.embedded_session.is_some() {
            return Ok(());
        }
        let Some(alias) = self.selected_host().map(|host| host.alias.clone()) else {
            return Ok(());
        };
        let (rows, cols) = self.embedded_terminal_size();
        self.embedded_session = Some(EmbeddedSession::spawn_ssh(
            &alias,
            &self.config.source,
            rows,
            cols,
        )?);
        Ok(())
    }

    pub fn poll_embedded_session(&mut self) {
        let update = self.embedded_session.as_mut().map(EmbeddedSession::poll);
        if update == Some(EmbeddedPoll::Succeeded) {
            self.embedded_session = None;
        }
    }

    pub fn sync_embedded_terminal_size(&mut self) -> tastty::Result<()> {
        let (rows, cols) = self.embedded_terminal_size();
        if let Some(session) = &mut self.embedded_session
            && session.is_running()
        {
            session.resize(rows, cols)?;
        }
        Ok(())
    }

    pub fn embedded_session_running(&self) -> bool {
        self.embedded_session
            .as_ref()
            .is_some_and(EmbeddedSession::is_running)
    }

    pub fn embedded_session_failed(&self) -> bool {
        self.embedded_session
            .as_ref()
            .is_some_and(|session| !session.is_running())
    }

    pub fn embedded_terminal_focused(&self) -> bool {
        self.embedded_session
            .as_ref()
            .is_some_and(|session| session.focus == EmbeddedFocus::Terminal)
    }

    pub fn focus_embedded_terminal(&mut self) {
        if let Some(session) = &mut self.embedded_session
            && session.is_running()
        {
            session.focus = EmbeddedFocus::Terminal;
        }
    }

    pub fn focus_tree(&mut self) {
        if let Some(session) = &mut self.embedded_session {
            session.focus = EmbeddedFocus::Tree;
        }
    }

    pub fn toggle_embedded_focus(&mut self) {
        if let Some(session) = &mut self.embedded_session
            && session.is_running()
        {
            session.focus = match session.focus {
                EmbeddedFocus::Terminal => EmbeddedFocus::Tree,
                EmbeddedFocus::Tree => EmbeddedFocus::Terminal,
            };
        }
    }

    pub fn close_embedded_session(&mut self) {
        self.embedded_session = None;
    }

    pub fn send_embedded_key(&mut self, key: KeyEvent) -> tastty::Result<()> {
        if let Some(session) = &mut self.embedded_session
            && session.is_running()
        {
            session.clear_selection();
            session.send_key(key)?;
        }
        Ok(())
    }

    pub fn send_embedded_paste(&mut self, text: &str) -> tastty::Result<()> {
        if let Some(session) = &mut self.embedded_session
            && session.is_running()
        {
            session.clear_selection();
            session.send_paste(text)?;
        }
        Ok(())
    }

    pub fn send_embedded_focus(&self, gained: bool) -> tastty::Result<()> {
        if let Some(session) = &self.embedded_session
            && session.is_running()
        {
            session.send_focus(gained)?;
        }
        Ok(())
    }

    pub fn handle_embedded_mouse(
        &mut self,
        mut mouse: MouseEvent,
    ) -> tastty::Result<EmbeddedMouseAction> {
        let Some(session) = &mut self.embedded_session else {
            return Ok(EmbeddedMouseAction::Ignored);
        };
        let Area {
            left,
            top,
            width,
            height,
        } = self.layout.details.inner();
        if mouse.column < left
            || mouse.column >= left.saturating_add(width)
            || mouse.row < top
            || mouse.row >= top.saturating_add(height)
        {
            return Ok(EmbeddedMouseAction::Ignored);
        }

        if !session.reports_mouse() {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    session.clear_selection();
                    session.scroll_up(3);
                    return Ok(EmbeddedMouseAction::Handled);
                }
                MouseEventKind::ScrollDown => {
                    session.clear_selection();
                    session.scroll_down(3);
                    return Ok(EmbeddedMouseAction::Handled);
                }
                _ => {}
            }
        }

        mouse.column -= left;
        mouse.row -= top;
        session.handle_mouse(mouse)
    }

    pub fn details_contains(&self, terminal_column: u16, terminal_row: u16) -> bool {
        self.layout.details.contains(terminal_column, terminal_row)
    }

    pub fn embedded_selection_text(&self) -> Option<String> {
        self.embedded_session
            .as_ref()
            .and_then(EmbeddedSession::selected_text)
            .filter(|text| !text.is_empty())
    }

    pub fn show_connection_failure(&mut self, failure: ConnectionFailure) {
        self.connection_failure = Some(failure);
    }

    pub fn dismiss_connection_failure(&mut self) {
        self.connection_failure = None;
    }

    pub fn poll_reachability(&mut self) {
        self.reachability_tracker.poll_into(&mut self.reachability);
    }

    pub fn host_reachability(&self, host_index: usize) -> HostReachability {
        if !self.reachability_tracker.enabled() {
            return HostReachability::Reachable;
        }
        self.reachability
            .get(&host_index)
            .copied()
            .unwrap_or(HostReachability::Unchecked)
    }

    fn embedded_terminal_area(&self) -> (u16, u16, u16, u16) {
        let area = self.layout.details.inner();
        (area.left, area.top, area.width, area.height)
    }

    fn embedded_terminal_size(&self) -> (u16, u16) {
        let (_, _, cols, rows) = self.embedded_terminal_area();
        (rows, cols)
    }

    fn check_hosts_below(&mut self, node_id: usize) {
        let mut host_indices = Vec::new();
        self.collect_host_indices(node_id, &mut host_indices);
        self.check_host_indices(host_indices);
    }

    fn check_host_indices(&mut self, host_indices: Vec<usize>) {
        if !self.reachability_tracker.enabled() {
            return;
        }
        let mut targets = Vec::new();
        for host_index in host_indices {
            if self.host_reachability(host_index) == HostReachability::Checking {
                continue;
            }
            self.reachability
                .insert(host_index, HostReachability::Checking);
            let host = &self.config.hosts[host_index];
            targets.push(CheckTarget {
                host_index,
                host: host
                    .resolved
                    .host_name
                    .clone()
                    .unwrap_or_else(|| host.alias.clone()),
                port: host.resolved.port.unwrap_or(22),
            });
        }
        if !targets.is_empty() {
            self.reachability_tracker.spawn(targets);
        }
    }

    fn collect_host_indices(&self, node_id: usize, host_indices: &mut Vec<usize>) {
        for child in &self.nodes[node_id].children {
            match self.nodes[*child].kind {
                NodeKind::Root | NodeKind::Group => {
                    self.collect_host_indices(*child, host_indices);
                }
                NodeKind::Host(host_index) => host_indices.push(host_index),
            }
        }
    }

    fn select_node(&mut self, node_id: usize) {
        if let Some(index) = self.visible.iter().position(|row| row.node_id == node_id) {
            self.selected = index;
            self.keep_selection_visible();
        }
    }

    pub fn rebuild_visible(&mut self) {
        let previous = self.selected_node_id();
        let mut rows = Vec::new();

        for child in self.nodes[self.root_id].children.clone() {
            self.collect_visible(child, 0, false, &mut rows);
        }

        self.visible = rows;
        if let Some(previous) = previous {
            self.selected = self
                .visible
                .iter()
                .position(|row| row.node_id == previous)
                .unwrap_or(0);
        }
        if self.selected >= self.visible.len() {
            self.selected = self.visible.len().saturating_sub(1);
        }
        self.keep_selection_visible();
    }

    fn keep_selection_visible(&mut self) {
        let height = usize::from(self.layout.tree.height.max(1));
        if self.selected < self.visible_offset {
            self.visible_offset = self.selected;
        } else if self.selected >= self.visible_offset + height {
            self.visible_offset = self.selected.saturating_sub(height.saturating_sub(1));
        }
    }

    fn collect_visible(
        &self,
        node_id: usize,
        depth: usize,
        ancestor_matches: bool,
        rows: &mut Vec<VisibleRow>,
    ) -> bool {
        let node = &self.nodes[node_id];
        let (self_matches, indices) = self.match_node(node);
        let mut child_rows = Vec::new();
        let search_active = !self.search.trim().is_empty();
        let traverse_children = search_active || self.expanded.contains(&node_id);
        let mut descendant_matches = false;

        if traverse_children {
            for child in &node.children {
                descendant_matches |= self.collect_visible(
                    *child,
                    depth + 1,
                    ancestor_matches || self_matches,
                    &mut child_rows,
                );
            }
        }

        if !search_active || ancestor_matches || self_matches || descendant_matches {
            rows.push(VisibleRow {
                node_id,
                depth,
                matched_indices: indices,
            });
            rows.extend(child_rows);
            true
        } else {
            false
        }
    }

    fn match_node(&self, node: &Node) -> (bool, Vec<usize>) {
        let query = self.search.trim();
        if query.is_empty() {
            return (true, Vec::new());
        }

        if let Some(indices) = search::fuzzy_indices(&node.name, query) {
            return (true, indices);
        }

        (
            node.search_fields
                .iter()
                .any(|field| search::fuzzy_indices(field, query).is_some()),
            Vec::new(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use crate::{GroupEntry, ResolvedHost, SshConfig};

    use super::*;

    #[test]
    fn tree_contains_nested_groups_and_hosts() {
        let config = SshConfig {
            source: PathBuf::from("config"),
            groups: vec![GroupEntry {
                path: vec!["Work".into(), "Prod".into()],
                description: Some("Production".into()),
                expanded_by_default: false,
                source: PathBuf::from("config"),
                line: 1,
            }],
            hosts: vec![HostEntry {
                alias: "prod-api".into(),
                description: Some("Customer API".into()),
                group_path: vec!["Work".into(), "Prod".into()],
                source: PathBuf::from("config"),
                line: 3,
                options: BTreeMap::new(),
                resolved: ResolvedHost::default(),
            }],
        };

        let app = App::new(config);
        let visible_names = app
            .visible
            .iter()
            .map(|row| app.nodes[row.node_id].name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(visible_names, vec!["Work"]);
        assert!(app.expanded.is_empty());
        assert_eq!(app.status, "1 host");
    }

    #[test]
    fn fuzzy_search_matches_descriptions_and_usernames_and_keeps_ancestors() {
        let config = SshConfig {
            source: PathBuf::from("config"),
            groups: vec![GroupEntry {
                path: vec!["Work".into(), "Prod".into()],
                description: Some("Customer environment".into()),
                expanded_by_default: false,
                source: PathBuf::from("config"),
                line: 1,
            }],
            hosts: vec![HostEntry {
                alias: "prod-api".into(),
                description: Some("Billing frontend".into()),
                group_path: vec!["Work".into(), "Prod".into()],
                source: PathBuf::from("config"),
                line: 3,
                options: BTreeMap::new(),
                resolved: ResolvedHost {
                    user: Some("deploy".into()),
                    ..ResolvedHost::default()
                },
            }],
        };

        let mut app = App::new(config);
        app.search = "bill".into();
        app.rebuild_visible();

        let visible_names = app
            .visible
            .iter()
            .map(|row| app.nodes[row.node_id].name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(visible_names, vec!["Work", "Prod", "prod-api"]);
        assert_eq!(app.host_reachability(0), HostReachability::Unchecked);

        app.search = "deploy".into();
        app.rebuild_visible();

        let visible_names = app
            .visible
            .iter()
            .map(|row| app.nodes[row.node_id].name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(visible_names, vec!["Work", "Prod", "prod-api"]);
    }

    #[test]
    fn revealing_search_results_expands_and_preserves_group_and_host_selection() {
        let config = SshConfig {
            source: PathBuf::from("config"),
            groups: vec![GroupEntry {
                path: vec!["Work".into(), "Prod".into()],
                description: Some("Production".into()),
                expanded_by_default: false,
                source: PathBuf::from("config"),
                line: 1,
            }],
            hosts: vec![HostEntry {
                alias: "prod-api".into(),
                description: Some("Billing frontend".into()),
                group_path: vec!["Work".into(), "Prod".into()],
                source: PathBuf::from("config"),
                line: 3,
                options: BTreeMap::new(),
                resolved: ResolvedHost::default(),
            }],
        };

        let mut group_app = App::with_network_checks(config.clone(), false);
        group_app.start_search();
        for character in "prod".chars() {
            group_app.push_search(character);
        }
        group_app.select_next();
        group_app.reveal_search_selection();

        assert_eq!(group_app.input_mode, InputMode::Normal);
        assert!(group_app.search.is_empty());
        assert_eq!(group_app.selected_node().unwrap().name, "Prod");
        assert_eq!(group_app.expanded.len(), 2);
        assert!(
            group_app
                .visible
                .iter()
                .any(|row| group_app.nodes[row.node_id].name == "prod-api")
        );

        let mut host_app = App::with_network_checks(config, false);
        host_app.start_search();
        for character in "billing".chars() {
            host_app.push_search(character);
        }
        host_app.select_next();
        host_app.select_next();
        host_app.reveal_search_selection();

        assert_eq!(host_app.input_mode, InputMode::Normal);
        assert!(host_app.search.is_empty());
        assert_eq!(host_app.selected_host().unwrap().alias, "prod-api");
        let mut parent = host_app.selected_node().unwrap().parent;
        while let Some(group_id) = parent {
            if group_id == host_app.root_id {
                break;
            }
            assert!(host_app.expanded.contains(&group_id));
            parent = host_app.nodes[group_id].parent;
        }
    }

    #[test]
    fn search_ignores_source_paths_and_rejects_scattered_description_matches() {
        let config = SshConfig {
            source: PathBuf::from("/home/m3nix/.ssh/config"),
            groups: vec![GroupEntry {
                path: vec!["Testlab".into()],
                description: Some("all hosts from testlab".into()),
                expanded_by_default: false,
                source: PathBuf::from("/home/m3nix/.ssh/config.d/test.conf"),
                line: 1,
            }],
            hosts: vec![HostEntry {
                alias: "lab-api".into(),
                description: Some("internal endpoint".into()),
                group_path: vec!["Testlab".into()],
                source: PathBuf::from("/home/m3nix/.ssh/config.d/test.conf"),
                line: 3,
                options: BTreeMap::new(),
                resolved: ResolvedHost {
                    host_name: Some("test1.internal".into()),
                    ..ResolvedHost::default()
                },
            }],
        };

        let mut app = App::new(config);
        app.search = "home".into();
        app.rebuild_visible();

        assert!(app.visible.is_empty());
    }

    #[test]
    fn disabled_network_checks_report_every_host_as_reachable() {
        let config = SshConfig {
            source: PathBuf::from("config"),
            groups: vec![GroupEntry {
                path: vec!["Work".into()],
                description: None,
                expanded_by_default: false,
                source: PathBuf::from("config"),
                line: 1,
            }],
            hosts: vec![HostEntry {
                alias: "offline".into(),
                description: None,
                group_path: vec!["Work".into()],
                source: PathBuf::from("config"),
                line: 2,
                options: BTreeMap::new(),
                resolved: ResolvedHost::default(),
            }],
        };

        let mut app = App::with_network_checks(config, false);
        assert_eq!(app.host_reachability(0), HostReachability::Reachable);

        app.toggle_selected_group();

        assert_eq!(app.host_reachability(0), HostReachability::Reachable);
        assert!(app.reachability.is_empty());
        assert!(app.reachability_tracker.is_idle());
    }

    #[test]
    fn connection_failures_do_not_replace_the_host_count() {
        let config = SshConfig {
            source: PathBuf::from("config"),
            groups: Vec::new(),
            hosts: vec![HostEntry {
                alias: "offline".into(),
                description: None,
                group_path: Vec::new(),
                source: PathBuf::from("config"),
                line: 1,
                options: BTreeMap::new(),
                resolved: ResolvedHost::default(),
            }],
        };

        let mut app = App::with_network_checks(config, false);
        app.show_connection_failure(ConnectionFailure {
            alias: "offline".into(),
            message: "Connection refused".into(),
            exit_status: Some(255),
        });

        assert_eq!(app.status, "1 host");
    }

    #[test]
    fn click_toggles_group_rows() {
        let config = SshConfig {
            source: PathBuf::from("config"),
            groups: vec![GroupEntry {
                path: vec!["Work".into()],
                description: None,
                expanded_by_default: false,
                source: PathBuf::from("config"),
                line: 1,
            }],
            hosts: vec![HostEntry {
                alias: "prod-api".into(),
                description: None,
                group_path: vec!["Work".into()],
                source: PathBuf::from("config"),
                line: 2,
                options: BTreeMap::new(),
                resolved: ResolvedHost {
                    host_name: Some("127.0.0.1".into()),
                    port: Some(0),
                    ..ResolvedHost::default()
                },
            }],
        };

        let mut app = App::new(config);
        assert_eq!(app.host_reachability(0), HostReachability::Unchecked);
        app.set_tree_area(1, 2, 20, 10);
        assert!(!app.click_at(21, 2));
        assert!(app.click_at(1, 2));

        assert!(app.expanded.contains(&app.visible[0].node_id));
        assert_eq!(app.visible.len(), 2);
        assert_eq!(app.host_reachability(0), HostReachability::Checking);

        app.reachability.insert(0, HostReachability::Reachable);
        app.click_at(1, 2);
        assert_eq!(app.host_reachability(0), HostReachability::Reachable);
        app.click_at(1, 2);
        assert_eq!(app.host_reachability(0), HostReachability::Checking);
    }

    #[test]
    fn siblings_are_sorted_alphabetically_regardless_of_kind() {
        let config = SshConfig {
            source: PathBuf::from("config"),
            groups: vec![
                GroupEntry {
                    path: vec!["zebra".into()],
                    description: None,
                    expanded_by_default: false,
                    source: PathBuf::from("config"),
                    line: 1,
                },
                GroupEntry {
                    path: vec!["Bravo".into()],
                    description: None,
                    expanded_by_default: false,
                    source: PathBuf::from("config"),
                    line: 2,
                },
            ],
            hosts: vec![
                HostEntry {
                    alias: "charlie".into(),
                    description: None,
                    group_path: Vec::new(),
                    source: PathBuf::from("config"),
                    line: 3,
                    options: BTreeMap::new(),
                    resolved: ResolvedHost::default(),
                },
                HostEntry {
                    alias: "alpha".into(),
                    description: None,
                    group_path: Vec::new(),
                    source: PathBuf::from("config"),
                    line: 4,
                    options: BTreeMap::new(),
                    resolved: ResolvedHost::default(),
                },
            ],
        };

        let app = App::new(config);
        let visible_names = app
            .visible
            .iter()
            .map(|row| app.nodes[row.node_id].name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(visible_names, vec!["alpha", "Bravo", "charlie", "zebra"]);
        assert_eq!(app.host_reachability(0), HostReachability::Checking);
        assert_eq!(app.host_reachability(1), HostReachability::Checking);
    }

    #[test]
    fn configured_group_starts_expanded_with_its_ancestors() {
        let config = SshConfig {
            source: PathBuf::from("config"),
            groups: vec![GroupEntry {
                path: vec!["Work".into(), "Prod".into()],
                description: None,
                expanded_by_default: true,
                source: PathBuf::from("config"),
                line: 1,
            }],
            hosts: vec![HostEntry {
                alias: "prod-api".into(),
                description: None,
                group_path: vec!["Work".into(), "Prod".into()],
                source: PathBuf::from("config"),
                line: 3,
                options: BTreeMap::new(),
                resolved: ResolvedHost {
                    host_name: Some("127.0.0.1".into()),
                    port: Some(0),
                    ..ResolvedHost::default()
                },
            }],
        };

        let app = App::new(config);
        let visible_names = app
            .visible
            .iter()
            .map(|row| app.nodes[row.node_id].name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(visible_names, ["Work", "Prod", "prod-api"]);
        assert_eq!(app.expanded.len(), 2);
        assert_eq!(app.host_reachability(0), HostReachability::Checking);
    }
}
