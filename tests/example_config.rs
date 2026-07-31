use std::io::Write;

use ssh_tui_rs::{App, SshConfig};

#[test]
fn example_config_loads_with_nested_groups_and_hostnames() {
    let mut config_file = tempfile::NamedTempFile::new().unwrap();
    write!(
        config_file,
        r#"# @group Work/Production
# @description Customer-facing production systems
# @description Public API node
Host prod-api
  HostName 10.0.0.10
  User deploy
  Port 2222

# @description SSH jump host
Host bastion
  HostName bastion.example.com
  User ops

# @group Work/Production/Databases
# @description Persistent storage
# @description Primary database node
Host prod-db
  HostName prod-db.internal
  User postgres
  ProxyJump bastion

# @group Personal/Lab
# @description Local experiments
# @description Home lab controller
Host lab-controller
  HostName 192.168.1.50
  User m3nix
"#
    )
    .unwrap();

    let config = SshConfig::load(Some(config_file.path())).unwrap();

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
