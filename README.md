# ssh-tui

A read-only terminal UI for browsing and launching hosts from OpenSSH config files.

## Metadata

Metadata is stored as comments above `Host` blocks. The app never writes to SSH
config files.

```sshconfig
# @group Work/Production
# @description Customer-facing systems
# @expanded
# @description API entry point
Host prod-api
  HostName 10.0.0.10
  User deploy
  Port 2222

# @group Work/Production/Databases
# @description Persistent storage
# @description Primary PostgreSQL node
Host prod-db
  HostName prod-db.internal
```

Supported comments:

- `# @group path/to/folder`
- `# @description text`
- `# @hidden` above a host omits it from the TUI
- `# @expanded` after a group opens that folder on launch

The first `# @description` after `# @group` describes that group. A subsequent
description before a `Host` block describes that host. A group applies to every
following host in the same physical file until another `# @group` comment is
found. Each included file starts ungrouped, so metadata from one
`config.d/*.conf` file cannot leak into the next. Folder paths use `/` for
nesting.

Wildcard `Host` blocks are not shown as connections, but their inherited
options are resolved for matching concrete hosts with `ssh -G`. Inherited
connection values such as `User`, `Port`, `IdentityFile`, and `ProxyJump` appear
in the Connection pane; other inherited options appear under Configuration.

## Usage

```bash
cargo run -- --config ~/.ssh/config
```

Controls:

- `j/k` or arrow keys move through the tree.
- `h/l`, left/right, or space fold and unfold folders.
- `/` starts fuzzy search across host names, host descriptions, folder names,
  folder descriptions, and group paths.
- `Enter` launches `ssh <HostAlias>`.
- `--browse-only` disables launching SSH.
- Mouse wheel scrolls selection; left click selects and toggles folders.

Folders start folded. Folder and host entries are sorted alphabetically, and
selecting a folder shows its descendant hosts in the details pane.

Host dots are gray until their folder is unfolded, yellow while a background
TCP check is running, green when the configured `HostName` and `Port` are
reachable, and red otherwise. Checks use a five-second timeout and port `22`
when `Port` is not set. Folding and unfolding a folder refreshes its descendant
hosts. Ungrouped hosts and hosts in `# @expanded` folders are checked on launch.

## Release Artifacts

Pushing a tag matching `v*` or `release-*` runs the GitHub Actions workflow in
`.github/workflows/release-artifact.yml`. It builds `cargo build --locked
--release` and uploads `ssh-tui-linux-x86_64.tar.gz` as a workflow artifact.
