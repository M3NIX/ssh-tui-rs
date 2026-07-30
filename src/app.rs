use std::collections::{HashMap, HashSet};

use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};

use crate::{GroupEntry, HostEntry, SshConfig};

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
}

impl App {
    pub fn new(config: SshConfig) -> Self {
        let (nodes, root_id, expanded) = build_tree(&config);
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
        };
        app.rebuild_visible();
        app.status = format!("{} hosts loaded", app.config.hosts.len());
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
            self.expanded.insert(node_id);
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

    pub fn display_name<'a>(&'a self, node: &'a Node) -> &'a str {
        &node.name
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

fn build_tree(config: &SshConfig) -> (Vec<Node>, usize, HashSet<usize>) {
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
    let expanded = HashSet::new();
    let mut folders: HashMap<Vec<String>, usize> = HashMap::new();

    for group in &config.groups {
        ensure_folder_path(&mut nodes, &mut folders, root_id, group);
    }

    for (host_index, host) in config.hosts.iter().enumerate() {
        let parent = if host.group_path.is_empty() {
            root_id
        } else {
            let synthetic = GroupEntry {
                path: host.group_path.clone(),
                description: None,
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
    (nodes, root_id, expanded)
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
    }

    #[test]
    fn click_toggles_folder_rows() {
        let config = SshConfig {
            source: PathBuf::from("config"),
            groups: vec![GroupEntry {
                path: vec!["Work".into()],
                description: None,
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
                resolved: ResolvedHost::default(),
            }],
        };

        let mut app = App::new(config);
        app.set_tree_area(2, 10);
        app.click_at(2);

        assert!(app.expanded.contains(&app.visible[0].node_id));
        assert_eq!(app.visible.len(), 2);
    }

    #[test]
    fn siblings_are_sorted_alphabetically_regardless_of_kind() {
        let config = SshConfig {
            source: PathBuf::from("config"),
            groups: vec![
                GroupEntry {
                    path: vec!["zebra".into()],
                    description: None,
                    source: PathBuf::from("config"),
                    line: 1,
                },
                GroupEntry {
                    path: vec!["Bravo".into()],
                    description: None,
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
    }
}
