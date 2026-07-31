use std::collections::{HashMap, HashSet};

use crate::{GroupEntry, SshConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Root,
    Group,
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
    pub search_fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleRow {
    pub node_id: usize,
    pub depth: usize,
    pub matched_indices: Vec<usize>,
}

pub(crate) fn build_tree(config: &SshConfig) -> (Vec<Node>, usize, HashSet<usize>, Vec<usize>) {
    let mut nodes = vec![Node {
        id: 0,
        name: "All Hosts".to_string(),
        description: None,
        parent: None,
        children: Vec::new(),
        kind: NodeKind::Root,
        search_fields: Vec::new(),
    }];
    let root_id = 0;
    let mut expanded = HashSet::new();
    let mut initially_expanded = Vec::new();
    let mut groups: HashMap<Vec<String>, usize> = HashMap::new();

    for group in &config.groups {
        let group_id = ensure_group_path(&mut nodes, &mut groups, root_id, group);
        if group.expanded_by_default {
            initially_expanded.push(group_id);
            expand_with_ancestors(group_id, root_id, &nodes, &mut expanded);
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
            ensure_group_path(&mut nodes, &mut groups, root_id, &synthetic)
        };
        let id = nodes.len();
        let search_fields = [
            Some(host.alias.clone()),
            host.description.clone(),
            Some(host.group_path.join("/")),
            host.resolved.host_name.clone(),
        ]
        .into_iter()
        .flatten()
        .filter(|field| !field.is_empty())
        .collect();
        nodes.push(Node {
            id,
            name: host.alias.clone(),
            description: host.description.clone(),
            parent: Some(parent),
            children: Vec::new(),
            kind: NodeKind::Host(host_index),
            search_fields,
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

fn ensure_group_path(
    nodes: &mut Vec<Node>,
    groups: &mut HashMap<Vec<String>, usize>,
    root_id: usize,
    group: &GroupEntry,
) -> usize {
    let mut parent = root_id;
    let mut path = Vec::new();

    for segment in &group.path {
        path.push(segment.clone());
        if let Some(id) = groups.get(&path) {
            parent = *id;
            continue;
        }

        let id = nodes.len();
        let description = if path == group.path {
            group.description.clone()
        } else {
            None
        };
        let search_fields = [
            Some(path.join("/")),
            Some(segment.clone()),
            description.clone(),
        ]
        .into_iter()
        .flatten()
        .filter(|field| !field.is_empty())
        .collect();
        nodes.push(Node {
            id,
            name: segment.clone(),
            description,
            parent: Some(parent),
            children: Vec::new(),
            kind: NodeKind::Group,
            search_fields,
        });
        nodes[parent].children.push(id);
        groups.insert(path.clone(), id);
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
        NodeKind::Root | NodeKind::Group => 0,
        NodeKind::Host(_) => 1,
    }
}
