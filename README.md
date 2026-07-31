# ssh-tui-rs

A read-only, keyboard-first TUI for browsing OpenSSH configuration, inspecting
effective host options, and starting SSH sessions.

![ssh-tui-rs demo showing tree navigation, fuzzy search, full-screen and inline SSH sessions, and a failed connection](assets/ssh-tui-demo.gif)

## Features

- Foldable, alphabetically sorted tree with nested folders and host details
- Native `Include` support for split configurations such as `~/.ssh/config.d/`
- Metadata comments for groups, descriptions, hidden hosts, and default expansion
- Effective inherited options resolved during the in-process configuration scan
- Compact fuzzy search across aliases, hostnames, descriptions, and folder paths
- On-demand TCP reachability indicators for hosts
- Embedded SSH terminal that keeps the host tree visible
- Keyboard and mouse navigation
- Read-only operation: SSH configuration files are never modified

## Requirements

- Linux
- OpenSSH client (`ssh`) in `PATH`
- Rust stable when building from source

## Installation & Usage

Build and install the release binary for the current user:

```bash
cargo build --locked --release
install -Dm755 target/release/ssh-tui-rs ~/.local/bin/ssh-tui-rs
```

Alternatively, let Cargo build and install it to the same location:

```bash
cargo install --locked --path . --root ~/.local
```

Make sure `~/.local/bin` is in `PATH`. Add this to your shell configuration,
such as `~/.bashrc` or `~/.zshrc`, if necessary:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

By default, `ssh-tui-rs` reads `~/.ssh/config`.

```bash
ssh-tui-rs
ssh-tui-rs --config ~/.ssh/other-config
ssh-tui-rs --no-network-check
ssh-tui-rs --embedded-ssh
```

| Key | Action |
| --- | --- |
| `j`, `k`, `Up`, `Down` | Move through the tree or search results |
| `Space` | Fold/unfold a folder; reveal a search result |
| `h`, `l`, `Left`, `Right` | Fold/unfold folders |
| `Enter` | Connect to a host; toggle a folder; reveal a search result |
| `Alt+Enter` | Open the selected host in the inline terminal |
| `/` | Enter search mode |
| `F6` | Switch between the tree and an embedded SSH session |
| `x` | Close an embedded session while the tree is focused |
| `Backspace`, `Esc` | Edit or leave search |
| `q`, `Esc` | Quit outside search |

Mouse scrolling and selection are supported. Click the search box to start
typing, click folders to toggle them, and double-click hosts to connect.

Use `Alt+Enter` for an inline session, or `--embedded-ssh` to make inline
sessions the default. The terminal runs in the details pane and keeps the tree
visible. Drag over text to select and copy it on mouse release.

Reachability checks run when folders are unfolded. Ungrouped hosts and hosts in
folders marked with `@expanded` are checked at startup. The probe is a direct
TCP connection to the effective `HostName` and `Port`; proxy-only hosts may
therefore appear unreachable.

Linux release artifacts are built when a `v*` or `release-*` tag is pushed.
The `linux-x86_64-glibc` artifact uses the standard GNU C library. Use the
`linux-x86_64-musl` artifact for a statically linked binary that does not
depend on the host's glibc version.

## Config Examples

Use OpenSSH's native `Include` directive in the main configuration:

```sshconfig
# ~/.ssh/config
Include ~/.ssh/config.d/*.conf

Host arch
  HostName localhost
  User m3nix
```

Each included file starts without an active group. This makes one file per
environment an easy way to organize the tree. Hosts without `@group`, such as
`arch` above, remain at the root.

```sshconfig
# ~/.ssh/config.d/work.conf
# @group Work
# @description Company systems
# @expanded

Host work-*
  User bob
  IdentityFile ~/.ssh/work_ed25519

# @description SSH jump host
Host work-bastion
  HostName bastion.example.com

# @group Work/Production
# @description Customer-facing systems

Host work-web
  HostName web.internal
  ProxyJump work-bastion

# @description Primary database
Host work-db
  HostName db.internal
  ProxyJump work-bastion
```

```sshconfig
# ~/.ssh/config.d/homelab.conf
# @group Personal/Lab
# @description Home lab systems

Host lab-controller
  HostName 192.168.1.50
  User m3nix

# @hidden
Host lab-helper
  HostName helper.internal
```

Supported metadata:

| Comment | Effect |
| --- | --- |
| `# @group Work/Production` | Assign following hosts to a nested folder |
| `# @description text` | Describe the active group or next host |
| `# @expanded` | Open the active group on startup |
| `# @hidden` | Hide the next `Host` block from the TUI |

The first `@description` after `@group` describes that group. Later
descriptions apply to the following host. A group remains active until another
`@group` appears in the same physical file. Wildcard `Host` blocks are hidden
from the tree but their options are inherited by matching concrete hosts.
