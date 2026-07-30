# ssh-tui

A read-only terminal UI for browsing and launching hosts from OpenSSH config files.

## Metadata

Metadata is stored as comments above `Host` blocks. The app never writes to SSH
config files.

```sshconfig
# @group Work/Production | Customer-facing systems
# @description API entry point
Host prod-api
  HostName 10.0.0.10
  User deploy
  Port 2222

# @group Work/Production/Databases | Persistent storage
# @description Primary PostgreSQL node
Host prod-db
  HostName prod-db.internal
```

Supported comments:

- `# @group path/to/folder | optional description`
- `# @folder path/to/folder | optional description`
- `# group: path/to/folder | optional description`
- `# folder: path/to/folder | optional description`
- `# @description text`
- `# description: text`

A group applies to every following host, including hosts from `Include` files,
until another group/folder comment is found. Folder paths use `/` for nesting.

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

## Release Artifacts

Pushing a tag matching `v*` or `release-*` runs the GitHub Actions workflow in
`.github/workflows/release-artifact.yml`. It builds `cargo build --locked
--release` and uploads `ssh-tui-linux-x86_64.tar.gz` as a workflow artifact.
