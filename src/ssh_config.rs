use std::{
    collections::{BTreeMap, HashSet},
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use glob::glob;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshConfig {
    pub source: PathBuf,
    pub hosts: Vec<HostEntry>,
    pub groups: Vec<GroupEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupEntry {
    pub path: Vec<String>,
    pub description: Option<String>,
    pub expanded_by_default: bool,
    pub source: PathBuf,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEntry {
    pub alias: String,
    pub description: Option<String>,
    pub group_path: Vec<String>,
    pub source: PathBuf,
    pub line: usize,
    pub options: BTreeMap<String, Vec<String>>,
    pub resolved: ResolvedHost,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedHost {
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_files: Vec<PathBuf>,
    pub proxy_jump: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct PendingHostMeta {
    description: Option<String>,
    hidden: bool,
}

#[derive(Debug, Clone)]
struct ActiveGroup {
    path: Vec<String>,
}

#[derive(Debug, Default)]
struct Scan {
    hosts: Vec<ScannedHost>,
    groups: BTreeMap<Vec<String>, GroupEntry>,
    visited: HashSet<PathBuf>,
}

#[derive(Debug)]
struct ScannedHost {
    alias: String,
    description: Option<String>,
    group_path: Vec<String>,
    source: PathBuf,
    line: usize,
    options: BTreeMap<String, Vec<String>>,
}

impl SshConfig {
    pub fn load(config: Option<&Path>) -> Result<Self> {
        let source = match config {
            Some(path) => expand_path(path),
            None => default_config_path()?,
        };

        let mut scan = Scan::default();
        scan_file(&source, &mut scan)
            .with_context(|| format!("failed to read {}", source.display()))?;

        let hosts = scan
            .hosts
            .into_iter()
            .map(|host| {
                let resolved = resolve_host(&host.options);
                HostEntry {
                    alias: host.alias,
                    description: host.description,
                    group_path: host.group_path,
                    source: host.source,
                    line: host.line,
                    options: host.options,
                    resolved,
                }
            })
            .collect();

        Ok(Self {
            source,
            hosts,
            groups: scan.groups.into_values().collect(),
        })
    }
}

fn default_config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not determine home directory")?;
    Ok(home.join(".ssh").join("config"))
}

fn resolve_host(options: &BTreeMap<String, Vec<String>>) -> ResolvedHost {
    ResolvedHost {
        host_name: option_value(options, "HostName"),
        user: option_value(options, "User"),
        port: option_value(options, "Port").and_then(|port| port.parse().ok()),
        identity_files: option_values(options, "IdentityFile")
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        proxy_jump: option_values(options, "ProxyJump")
            .into_iter()
            .flat_map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect(),
    }
}

fn option_value(options: &BTreeMap<String, Vec<String>>, key: &str) -> Option<String> {
    option_values(options, key).into_iter().next()
}

fn option_values(options: &BTreeMap<String, Vec<String>>, key: &str) -> Vec<String> {
    options
        .iter()
        .filter(|(option, _)| option.eq_ignore_ascii_case(key))
        .flat_map(|(_, values)| values.clone())
        .collect()
}

fn scan_file(path: &Path, scan: &mut Scan) -> Result<()> {
    let path = expand_path(path);
    if !path.exists() {
        return Ok(());
    }

    let canonical = path.canonicalize().unwrap_or(path.clone());
    if !scan.visited.insert(canonical) {
        return Ok(());
    }

    let file = File::open(&path)?;
    let reader = BufReader::new(file);
    let mut current_hosts: Vec<usize> = Vec::new();
    let mut active_group: Option<ActiveGroup> = None;
    let mut pending_host = PendingHostMeta::default();

    for (index, line) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = line?;
        let trimmed = line.trim();

        if let Some(comment) = trimmed.strip_prefix('#') {
            apply_comment_metadata(
                comment.trim(),
                scan,
                &mut active_group,
                &mut pending_host,
                &path,
                line_number,
            );
            continue;
        }

        let Some((keyword, values)) = parse_directive(trimmed) else {
            if !trimmed.is_empty() {
                current_hosts.clear();
                pending_host = PendingHostMeta::default();
            }
            continue;
        };

        if keyword.eq_ignore_ascii_case("include") {
            for include in resolve_includes(&values) {
                scan_file(&include, scan)?;
            }
            current_hosts.clear();
            pending_host = PendingHostMeta::default();
            continue;
        }

        if keyword.eq_ignore_ascii_case("host") {
            current_hosts.clear();
            if pending_host.hidden {
                pending_host = PendingHostMeta::default();
                continue;
            }
            let aliases = values
                .iter()
                .filter(|value| is_visible_host_alias(value))
                .cloned()
                .collect::<Vec<_>>();

            if aliases.is_empty() {
                pending_host = PendingHostMeta::default();
                continue;
            }

            for alias in aliases {
                let host = ScannedHost {
                    alias,
                    description: pending_host.description.clone(),
                    group_path: active_group
                        .as_ref()
                        .map(|group| group.path.clone())
                        .unwrap_or_default(),
                    source: path.clone(),
                    line: line_number,
                    options: BTreeMap::new(),
                };
                scan.hosts.push(host);
                current_hosts.push(scan.hosts.len() - 1);
            }
            pending_host = PendingHostMeta::default();
            continue;
        }

        if current_hosts.is_empty() || keyword.eq_ignore_ascii_case("match") {
            continue;
        }

        for host_index in &current_hosts {
            scan.hosts[*host_index]
                .options
                .entry(keyword.clone())
                .or_default()
                .extend(values.clone());
        }
    }

    Ok(())
}

fn apply_comment_metadata(
    comment: &str,
    scan: &mut Scan,
    active_group: &mut Option<ActiveGroup>,
    pending_host: &mut PendingHostMeta,
    source: &Path,
    line: usize,
) {
    let Some((key, value)) = parse_metadata(comment) else {
        return;
    };

    match key.as_str() {
        "group" | "folder" => {
            let (path, description) = split_metadata_value(&value);
            let path = split_group_path(&path);
            if path.is_empty() {
                return;
            }
            *active_group = Some(ActiveGroup { path: path.clone() });
            scan.groups.insert(
                path.clone(),
                GroupEntry {
                    path,
                    description,
                    expanded_by_default: false,
                    source: source.to_path_buf(),
                    line,
                },
            );
        }
        "expanded" => {
            if let Some(group) = active_group
                .as_ref()
                .and_then(|active| scan.groups.get_mut(&active.path))
            {
                group.expanded_by_default = true;
            }
        }
        "hidden" => pending_host.hidden = true,
        "description" | "desc" | "host-description" | "host_description"
            if !value.trim().is_empty() =>
        {
            pending_host.description = Some(value.trim().to_string());
        }
        _ => {}
    }
}

fn parse_metadata(comment: &str) -> Option<(String, String)> {
    let normalized = comment.trim();
    let (key, value) = if let Some(rest) = normalized.strip_prefix('@') {
        let mut parts = rest.splitn(2, char::is_whitespace);
        (parts.next()?, parts.next().unwrap_or(""))
    } else {
        normalized.split_once(':')?
    };

    let key = key.trim().to_ascii_lowercase();
    let value = value.trim();
    (!key.is_empty()).then(|| (key, value.to_string()))
}

fn split_metadata_value(value: &str) -> (String, Option<String>) {
    let Some((name, description)) = value.split_once('|') else {
        return (value.trim().to_string(), None);
    };
    let description = description.trim();
    (
        name.trim().to_string(),
        (!description.is_empty()).then(|| description.to_string()),
    )
}

fn split_group_path(path: &str) -> Vec<String> {
    path.split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_directive(line: &str) -> Option<(String, Vec<String>)> {
    let without_comment = strip_inline_comment(line).trim().to_string();
    if without_comment.is_empty() {
        return None;
    }

    let (keyword, value) = if let Some((keyword, value)) = without_comment.split_once('=') {
        (keyword.trim().to_string(), value.trim().to_string())
    } else {
        let mut parts = without_comment.splitn(2, char::is_whitespace);
        let keyword = parts.next()?.trim().to_string();
        let value = parts.next().unwrap_or("").trim().to_string();
        (keyword, value)
    };

    if keyword.is_empty() {
        return None;
    }

    let values = shell_words::split(&value).unwrap_or_else(|_| {
        value
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
    });
    Some((keyword, values))
}

fn strip_inline_comment(line: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..index],
            _ => {}
        }
    }
    line
}

fn is_visible_host_alias(value: &str) -> bool {
    !value.starts_with('!')
        && !value.contains('*')
        && !value.contains('?')
        && !value.trim().is_empty()
}

fn resolve_includes(values: &[String]) -> Vec<PathBuf> {
    values
        .iter()
        .flat_map(|value| {
            let pattern = resolve_include_pattern(value);
            glob(&pattern)
                .ok()
                .into_iter()
                .flat_map(|paths| paths.filter_map(|entry| entry.ok()))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn resolve_include_pattern(pattern: &str) -> String {
    let expanded = expand_tilde(pattern);
    if Path::new(&expanded).is_absolute() {
        expanded
    } else if let Some(home) = dirs::home_dir() {
        home.join(".ssh").join(expanded).display().to_string()
    } else {
        expanded
    }
}

fn expand_path(path: &Path) -> PathBuf {
    PathBuf::from(expand_tilde(&path.display().to_string()))
}

fn expand_tilde(path: &str) -> String {
    if path == "~" {
        return dirs::home_dir()
            .map(|home| home.display().to_string())
            .unwrap_or_else(|| path.to_string());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return dirs::home_dir()
            .map(|home| home.join(rest).display().to_string())
            .unwrap_or_else(|| path.to_string());
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn parses_metadata_and_nested_groups() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("config");
        fs::write(
            &config,
            r#"
# @group Work/Prod | Production machines
# @description Handles customer traffic
Host prod-api
  HostName 10.0.0.10
  User deploy
  Port 2222
"#,
        )
        .unwrap();

        let parsed = SshConfig::load(Some(&config)).unwrap();
        assert_eq!(parsed.groups[0].path, vec!["Work", "Prod"]);
        assert_eq!(
            parsed.groups[0].description.as_deref(),
            Some("Production machines")
        );
        assert_eq!(parsed.hosts[0].alias, "prod-api");
        assert_eq!(
            parsed.hosts[0].description.as_deref(),
            Some("Handles customer traffic")
        );
        assert_eq!(parsed.hosts[0].group_path, vec!["Work", "Prod"]);
    }

    #[test]
    fn parses_hidden_hosts_and_expanded_groups() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("config");
        fs::write(
            &config,
            r#"
# @group Work/Prod | Production machines
# @expanded
# @hidden
Host internal-helper
  HostName helper.internal

Host prod-api
  HostName api.internal
"#,
        )
        .unwrap();

        let parsed = SshConfig::load(Some(&config)).unwrap();

        assert_eq!(parsed.hosts.len(), 1);
        assert_eq!(parsed.hosts[0].alias, "prod-api");
        assert!(parsed.groups[0].expanded_by_default);
    }

    #[test]
    fn include_files_have_independent_group_state() {
        let dir = tempdir().unwrap();
        let included = dir.path().join("included.conf");
        let config = dir.path().join("config");
        fs::write(
            &included,
            r#"
Host app-01
  HostName app.internal
# group: Other | A new section
Host other-01
"#,
        )
        .unwrap();
        fs::write(
            &config,
            format!(
                r#"
# @group Work/Apps | Application servers
Include {}
Host after-include
"#,
                included.display()
            ),
        )
        .unwrap();

        let parsed = SshConfig::load(Some(&config)).unwrap();
        let app = parsed
            .hosts
            .iter()
            .find(|host| host.alias == "app-01")
            .unwrap();
        let other = parsed
            .hosts
            .iter()
            .find(|host| host.alias == "other-01")
            .unwrap();
        let after = parsed
            .hosts
            .iter()
            .find(|host| host.alias == "after-include")
            .unwrap();

        assert!(app.group_path.is_empty());
        assert_eq!(other.group_path, vec!["Other"]);
        assert_eq!(after.group_path, vec!["Work", "Apps"]);
    }

    #[test]
    fn globbed_file_without_group_stays_ungrouped() {
        let dir = tempdir().unwrap();
        let config_dir = dir.path().join("config.d");
        fs::create_dir(&config_dir).unwrap();
        fs::write(
            config_dir.join("grouped.conf"),
            r#"
# @group Work
Host work
  HostName work.internal
"#,
        )
        .unwrap();
        fs::write(
            config_dir.join("ungrouped.conf"),
            r#"
Host arch
  HostName localhost
  User m3nix
"#,
        )
        .unwrap();
        let config = dir.path().join("config");
        fs::write(
            &config,
            format!("Include {}/*.conf\n", config_dir.display()),
        )
        .unwrap();

        let parsed = SshConfig::load(Some(&config)).unwrap();
        let work = parsed
            .hosts
            .iter()
            .find(|host| host.alias == "work")
            .unwrap();
        let arch = parsed
            .hosts
            .iter()
            .find(|host| host.alias == "arch")
            .unwrap();

        assert_eq!(work.group_path, ["Work"]);
        assert!(arch.group_path.is_empty());
    }

    #[test]
    fn skips_wildcard_hosts() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("config");
        fs::write(
            &config,
            r#"
Host *
  ForwardAgent no
Host bastion *.internal !blocked
"#,
        )
        .unwrap();

        let parsed = SshConfig::load(Some(&config)).unwrap();
        assert_eq!(parsed.hosts.len(), 1);
        assert_eq!(parsed.hosts[0].alias, "bastion");
    }
}
