use ssh_tui_rs::{App, SshConfig};

#[test]
fn example_config_loads_with_nested_groups_and_hostnames() {
    let config = SshConfig::load(Some(std::path::Path::new("examples/ssh_config"))).unwrap();

    assert_eq!(config.hosts.len(), 4);
    assert!(
        config
            .groups
            .iter()
            .any(|group| group.path == ["Work", "Production", "Databases"])
    );

    let db = config
        .hosts
        .iter()
        .find(|host| host.alias == "prod-db")
        .unwrap();
    assert_eq!(db.group_path, ["Work", "Production", "Databases"]);
    assert_eq!(db.resolved.host_name.as_deref(), Some("prod-db.internal"));
    assert_eq!(db.resolved.proxy_jump, ["bastion"]);

    let mut app = App::new(config);
    app.search = "persistent".into();
    app.rebuild_visible();

    let names = app
        .visible
        .iter()
        .map(|row| app.nodes[row.node_id].name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"Databases"));
    assert!(names.contains(&"prod-db"));
}
