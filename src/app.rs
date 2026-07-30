use std::{
    collections::{HashMap, HashSet},
    sync::mpsc,
    time::Duration,
};

use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};

use crate::{
    GroupEntry, HostEntry, HostReachability, SshConfig,
    reachability::{CheckResult, CheckTarget, spawn_checks},
};

const REACHABILITY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Search,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Root,
    Folder,
    Host(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: usize,
    pub name: String,
    pub description: Option<String>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub kind: NodeKind,
    pub search_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleRow {
    pub node_id: usize,
    pub depth: usize,
    pub matched_indices: Vec<usize>,
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
    pub tree_top: u16,
    pub tree_height: u16,
    pub visible_offset: usize,
    pub status: String,
    pub reachability: HashMap<usize, HostReachability>,
    reachability_updates: Vec<mpsc::Receiver<CheckResult>>,
}

impl App {
    pub fn new(config: SshConfig) -> Self {
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
            tree_top: 0,
            tree_height: 0,
            visible_offset: 0,
            status: String::new(),
            reachability: HashMap::new(),
            reachability_updates: Vec::new(),
        };
        app.rebuild_visible();
        app.status = format!("{} hosts loaded", app.config.hosts.len());
        let ungrouped_hosts = app.nodes[app.root_id]
            .children
            .iter()
            .filter_map(|node_id| match app.nodes[*node_id].kind {
                NodeKind::Host(host_index) => Some(host_index),
                NodeKind::Root | NodeKind::Folder => None,
            })
            .collect::<Vec<_>>();
        app.check_host_indices(ungrouped_hosts);
        for folder_id in initially_expanded {
            app.check_hosts_below(folder_id);
        }
        app
    }

    pub fn start_search(&mut self) {
        self.input_mode = InputMode::Search;
    }

    pub fn finish_search(&mut self) {
        self.input_mode = InputMode::Normal;
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

    pub fn toggle_selected_folder(&mut self) {
        let Some(node_id) = self.selected_node_id() else {
            return;
        };
        if !matches!(self.nodes[node_id].kind, NodeKind::Folder | NodeKind::Root) {
            return;
        }
        if !self.expanded.insert(node_id) {
            self.expanded.remove(&node_id);
        } else {
            self.check_hosts_below(node_id);
        }
        self.rebuild_visible();
    }

    pub fn collapse_selected(&mut self) {
        let Some(node_id) = self.selected_node_id() else {
            return;
        };
        if matches!(self.nodes[node_id].kind, NodeKind::Folder | NodeKind::Root)
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
        if matches!(self.nodes[node_id].kind, NodeKind::Folder | NodeKind::Root) {
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

    pub fn select_at(&mut self, terminal_row: u16) -> bool {
        if terminal_row < self.tree_top || terminal_row >= self.tree_top + self.tree_height {
            return false;
        }
        let visible_index = self.visible_offset + usize::from(terminal_row - self.tree_top);
        if visible_index >= self.visible.len() {
            return false;
        }
        self.selected = visible_index;
        true
    }

    pub fn click_at(&mut self, terminal_row: u16) {
        if self.select_at(terminal_row)
            && self
                .selected_node_id()
                .map(|node_id| matches!(self.nodes[node_id].kind, NodeKind::Folder))
                .unwrap_or(false)
        {
            self.toggle_selected_folder();
        }
    }

    pub fn set_tree_area(&mut self, top: u16, height: u16) {
        self.tree_top = top;
        self.tree_height = height;
        self.keep_selection_visible();
    }

    pub fn set_status(&mut self, status: String) {
        self.status = status;
    }

    pub fn poll_reachability(&mut self) {
        let mut active = Vec::new();
        for updates in self.reachability_updates.drain(..) {
            loop {
                match updates.try_recv() {
                    Ok(result) => {
                        self.reachability
                            .insert(result.host_index, result.reachability);
                    }
                    Err(mpsc::TryRecvError::Empty) => {
                        active.push(updates);
                        break;
                    }
                    Err(mpsc::TryRecvError::Disconnected) => break,
                }
            }
        }
        self.reachability_updates = active;
    }

    pub fn host_reachability(&self, host_index: usize) -> HostReachability {
        self.reachability
            .get(&host_index)
            .copied()
            .unwrap_or(HostReachability::Unchecked)
    }

    pub fn display_name<'a>(&'a self, node: &'a Node) -> &'a str {
        &node.name
    }

    fn check_hosts_below(&mut self, node_id: usize) {
        let mut host_indices = Vec::new();
        self.collect_host_indices(node_id, &mut host_indices);
        self.check_host_indices(host_indices);
    }

    fn check_host_indices(&mut self, host_indices: Vec<usize>) {
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
            self.reachability_updates
                .push(spawn_checks(targets, REACHABILITY_TIMEOUT));
        }
    }

    fn collect_host_indices(&self, node_id: usize, host_indices: &mut Vec<usize>) {
        for child in &self.nodes[node_id].children {
            match self.nodes[*child].kind {
                NodeKind::Root | NodeKind::Folder => {
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
        let matcher = SkimMatcherV2::default();
        let mut rows = Vec::new();

        for child in self.nodes[self.root_id].children.clone() {
            self.collect_visible(child, 0, false, &matcher, &mut rows);
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
        let height = usize::from(self.tree_height.max(1));
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
        matcher: &SkimMatcherV2,
        rows: &mut Vec<VisibleRow>,
    ) -> bool {
        let node = &self.nodes[node_id];
        let (self_matches, indices) = self.match_node(node, matcher);
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
                    matcher,
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

    fn match_node(&self, node: &Node, matcher: &SkimMatcherV2) -> (bool, Vec<usize>) {
        let query = self.search.trim();
        if query.is_empty() {
            return (true, Vec::new());
        }

        let display = self.display_name(node);
        if let Some((_score, indices)) = matcher.fuzzy_indices(display, query) {
            return (true, indices);
        }

        (
            matcher.fuzzy_match(&node.search_text, query).is_some(),
            Vec::new(),
        )
    }
}

fn build_tree(config: &SshConfig) -> (Vec<Node>, usize, HashSet<usize>, Vec<usize>) {
    let mut nodes = vec![Node {
        id: 0,
        name: "All Hosts".to_string(),
        description: None,
        parent: None,
        children: Vec::new(),
        kind: NodeKind::Root,
        search_text: String::new(),
    }];
    let root_id = 0;
    let mut expanded = HashSet::new();
    let mut initially_expanded = Vec::new();
    let mut folders: HashMap<Vec<String>, usize> = HashMap::new();

    for group in &config.groups {
        let folder_id = ensure_folder_path(&mut nodes, &mut folders, root_id, group);
        if group.expanded_by_default {
            initially_expanded.push(folder_id);
            expand_with_ancestors(folder_id, root_id, &nodes, &mut expanded);
        }
    }

    for (host_index, host) in config.hosts.iter().enumerate() {
        let parent = if host.group_path.is_empty() {
            root_id
        } else {
            let synthetic = GroupEntry {
                path: host.group_path.clone(),
                description: None,
                expanded_by_default: false,
                source: host.source.clone(),
                line: host.line,
            };
            ensure_folder_path(&mut nodes, &mut folders, root_id, &synthetic)
        };
        let id = nodes.len();
        let search_text = [
            &host.alias,
            host.description.as_deref().unwrap_or_default(),
            &host.group_path.join("/"),
            host.resolved.host_name.as_deref().unwrap_or_default(),
        ]
        .join(" ");
        nodes.push(Node {
            id,
            name: host.alias.clone(),
            description: host.description.clone(),
            parent: Some(parent),
            children: Vec::new(),
            kind: NodeKind::Host(host_index),
            search_text,
        });
        nodes[parent].children.push(id);
    }

    sort_children(root_id, &mut nodes);
    (nodes, root_id, expanded, initially_expanded)
}

fn expand_with_ancestors(
    node_id: usize,
    root_id: usize,
    nodes: &[Node],
    expanded: &mut HashSet<usize>,
) {
    let mut current = Some(node_id);
    while let Some(id) = current {
        if id == root_id {
            break;
        }
        expanded.insert(id);
        current = nodes[id].parent;
    }
}

fn ensure_folder_path(
    nodes: &mut Vec<Node>,
    folders: &mut HashMap<Vec<String>, usize>,
    root_id: usize,
    group: &GroupEntry,
) -> usize {
    let mut parent = root_id;
    let mut path = Vec::new();

    for segment in &group.path {
        path.push(segment.clone());
        if let Some(id) = folders.get(&path) {
            parent = *id;
            continue;
        }

        let id = nodes.len();
        let description = if path == group.path {
            group.description.clone()
        } else {
            None
        };
        let search_text = [
            path.join("/"),
            segment.clone(),
            description.clone().unwrap_or_default(),
        ]
        .join(" ");
        nodes.push(Node {
            id,
            name: segment.clone(),
            description,
            parent: Some(parent),
            children: Vec::new(),
            kind: NodeKind::Folder,
            search_text,
        });
        nodes[parent].children.push(id);
        folders.insert(path.clone(), id);
        parent = id;
    }

    parent
}

fn sort_children(node_id: usize, nodes: &mut [Node]) {
    let mut children = std::mem::take(&mut nodes[node_id].children);
    children.sort_by(|left, right| {
        let left_node = &nodes[*left];
        let right_node = &nodes[*right];
        left_node
            .name
            .to_lowercase()
            .cmp(&right_node.name.to_lowercase())
            .then_with(|| kind_rank(&left_node.kind).cmp(&kind_rank(&right_node.kind)))
            .then_with(|| left_node.name.cmp(&right_node.name))
    });
    for child in &children {
        sort_children(*child, nodes);
    }
    nodes[node_id].children = children;
}

fn kind_rank(kind: &NodeKind) -> u8 {
    match kind {
        NodeKind::Root | NodeKind::Folder => 0,
        NodeKind::Host(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use crate::{ResolvedHost, SshConfig};

    use super::*;

    #[test]
    fn tree_contains_nested_folders_and_hosts() {
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
    }

    #[test]
    fn fuzzy_search_matches_descriptions_and_keeps_ancestors() {
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
                resolved: ResolvedHost::default(),
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
    }

    #[test]
    fn click_toggles_folder_rows() {
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
        app.set_tree_area(2, 10);
        app.click_at(2);

        assert!(app.expanded.contains(&app.visible[0].node_id));
        assert_eq!(app.visible.len(), 2);
        assert_eq!(app.host_reachability(0), HostReachability::Checking);

        app.reachability.insert(0, HostReachability::Reachable);
        app.click_at(2);
        assert_eq!(app.host_reachability(0), HostReachability::Reachable);
        app.click_at(2);
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
