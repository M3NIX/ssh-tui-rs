use std::{fmt::Write, fs};

use ssh_tui_rs::SshConfig;
use tempfile::tempdir;

#[test]
fn loads_large_configs_in_a_single_scan() {
    let dir = tempdir().unwrap();
    let config = dir.path().join("config");
    let mut contents = String::from("Host *\n  User deploy\n  IdentityFile ~/.ssh/deploy\n\n");
    for index in 0..1_000 {
        write!(
            contents,
            "Host host-{index}\n  HostName 10.0.{}.{index}\n\nMatch originalhost conditional-{index}\n  Port 2222\n\n",
            index / 256
        )
        .unwrap();
    }
    fs::write(&config, contents).unwrap();

    let parsed = SshConfig::load(Some(&config)).unwrap();

    assert_eq!(parsed.hosts.len(), 1_000);
    assert_eq!(parsed.hosts[999].resolved.user.as_deref(), Some("deploy"));
    assert_eq!(parsed.hosts[999].resolved.port, None);
    assert_eq!(
        parsed.hosts[999].resolved.host_name.as_deref(),
        Some("10.0.3.999")
    );
}
