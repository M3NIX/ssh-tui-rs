#!/usr/bin/env python3
"""Build and record the reproducible README GIF using only Python, Cargo, and agg."""

from __future__ import annotations

import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time


ROOT = Path(__file__).resolve().parent.parent
COLS = 110
APP_ROWS = 30
CAST_ROWS = 32
ONLINE_PORT = 2222
OFFLINE_PORT = 2223
PACE = 1.1
END_TIME = 36.5

# time, bytes sent to the application, displayed key, explanation
ACTIONS = [
    (1.0, b" ", "Space", "Expand Engineering"),
    (1.8, b"\x1b[B", "↓", "Select Development"),
    (2.5, b" ", "Space", "Expand Development"),
    (3.2, b"\x1b[B", "↓", "Select dev-shell-01"),
    (3.8, b"\x1b[B", "↓ × 2", "Move to the next host"),
    (4.4, b"\x1b[A", "↑", "Move to the previous host"),
    (5.0, b"\x1b[D", "←", "Move to the parent group"),
    (5.5, b" ", "Space", "Collapse Development"),
    (6.5, b"/", "/", "Start fuzzy search"),
    (7.5, b"p", "p", "Search query"),
    (7.85, b"r", "pr", "Search query"),
    (8.2, b"d", "prd", "Fuzzy-match prod-api-01"),
    (9.1, b"\x1b[B", "↓", "Select Production"),
    (9.65, b"\x1b[B", "↓ × 2", "Select prod-api-01"),
    (10.4, b"\r", "Enter", "Reveal the search result"),
    (11.8, b"\r", "Enter", "Open full-screen SSH"),
    (13.0, b"hostname\r", "hostname Enter", "Run a remote command"),
    (14.6, b"exit\r", "exit Enter", "Close full-screen SSH"),
    (16.3, b"/", "/", "Search for the development shell"),
    (17.3, b"s", "s", "Search query"),
    (17.65, b"h", "sh", "Search query"),
    (18.0, b"e", "she", "Search query"),
    (18.35, b"l", "shel", "Search query"),
    (18.7, b"l", "shell", "Match dev-shell-01"),
    (19.7, b"\x1b[B", "↓", "Select Development"),
    (20.25, b"\x1b[B", "↓ × 2", "Select dev-shell-01"),
    (21.0, b"\r", "Enter", "Reveal the search result"),
    (22.5, b"\x1b\r", "Alt+Enter", "Open inline SSH"),
    (23.9, b"whoami\r", "whoami Enter", "Run an inline command"),
    (25.4, b"pwd\r", "pwd Enter", "Run another command"),
    (26.9, b"\x1b[15~", "F5", "Return focus to the tree"),
    (27.6, b"x", "x", "Close inline SSH"),
    (29.3, b"/", "/", "Search for the recovery gateway"),
    (30.3, b"gateway", "gateway", "Match dr-gateway-01"),
    (31.3, b"\x1b[B", "↓", "Select dr-gateway-01"),
    (32.0, b"\r", "Enter", "Reveal the search result"),
    (33.4, b"\r", "Enter", "Attempt SSH (expected failure)"),
    (34.0, b"", "Enter", "SSH failed as expected"),
]


def key_overlay(key: str, explanation: str) -> str:
    """Render the asciinema-style lower-right keystroke overlay."""
    plain = f" {key}  {explanation} "
    column = max(1, COLS - len(plain) + 1)
    return (
        "\x1b7"
        f"\x1b[{CAST_ROWS};1H\x1b[2K\x1b[{CAST_ROWS};{column}H"
        f"\x1b[1;30;103m {key} \x1b[0;38;5;15;48;5;236m {explanation} "
        "\x1b[0m\x1b8"
    )


def start_reachable_endpoint() -> tuple[socket.socket, threading.Event, threading.Thread]:
    """Keep the online demo port reachable without implementing an SSH server."""
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", ONLINE_PORT))
    listener.listen()
    listener.settimeout(0.2)
    stopped = threading.Event()

    def accept_connections() -> None:
        while not stopped.is_set():
            try:
                connection, _ = listener.accept()
            except TimeoutError:
                continue
            except OSError:
                break
            connection.close()

    thread = threading.Thread(target=accept_connections, daemon=True)
    thread.start()
    return listener, stopped, thread


def ensure_offline_port_is_free() -> None:
    probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        probe.bind(("127.0.0.1", OFFLINE_PORT))
    except OSError as error:
        raise RuntimeError(f"demo offline port {OFFLINE_PORT} is already in use") from error
    finally:
        probe.close()


def record(cast_path: Path, resolver_library: Path) -> None:
    binary = ROOT / "target/release/ssh-tui-rs"
    config = ROOT / "demo/ssh_config"
    demo_bin = ROOT / "demo/bin"
    pid, fd = pty.fork()
    if pid == 0:
        environment = os.environ.copy()
        environment["PATH"] = str(demo_bin) + os.pathsep + environment.get("PATH", "")
        environment["TERM"] = "xterm-256color"
        environment["COLORTERM"] = "truecolor"
        environment["LD_PRELOAD"] = str(resolver_library)
        environment.pop("NO_COLOR", None)
        os.chdir(ROOT)
        os.execve(
            binary,
            [binary, "--config", config.relative_to(ROOT)],
            environment,
        )

    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", APP_ROWS, COLS, 0, 0))
    flags = fcntl.fcntl(fd, fcntl.F_GETFL)
    fcntl.fcntl(fd, fcntl.F_SETFL, flags | os.O_NONBLOCK)

    started = time.monotonic()
    next_action = 0
    events: list[list[object]] = []
    child_done = False

    while not child_done:
        now = time.monotonic() - started
        while next_action < len(ACTIONS) and now >= ACTIONS[next_action][0] * PACE:
            _, input_bytes, key, explanation = ACTIONS[next_action]
            events.append([round(now, 6), "o", key_overlay(key, explanation)])
            os.write(fd, input_bytes)
            next_action += 1

        readable, _, _ = select.select([fd], [], [], 0.02)
        if readable:
            try:
                data = os.read(fd, 65536)
            except (BlockingIOError, OSError):
                data = b""
            if data:
                if b"\x1b[6n" in data:
                    # A real terminal answers Crossterm's cursor position query.
                    os.write(fd, b"\x1b[1;1R")
                events.append(
                    [
                        round(time.monotonic() - started, 6),
                        "o",
                        data.decode("utf-8", "replace"),
                    ]
                )

        waited, _ = os.waitpid(pid, os.WNOHANG)
        child_done = waited == pid
        if now > END_TIME * PACE:
            # Preserve the failure modal as the final looping frame.
            os.kill(pid, 9)
            os.waitpid(pid, 0)
            child_done = True

    header = {
        "version": 2,
        "width": COLS,
        "height": CAST_ROWS,
        "timestamp": int(time.time()),
        "env": {"SHELL": "/bin/bash", "TERM": "xterm-256color"},
        "title": "ssh-tui-rs demo",
    }
    with cast_path.open("w", encoding="utf-8") as recording:
        recording.write(json.dumps(header, ensure_ascii=False) + "\n")
        for event in events:
            recording.write(json.dumps(event, ensure_ascii=False) + "\n")


def main() -> None:
    agg = os.environ.get("AGG") or shutil.which("agg")
    if not agg:
        raise SystemExit(
            "agg is required; install it with:\n"
            "cargo install --locked --git https://github.com/asciinema/agg"
        )

    subprocess.run(["cargo", "build", "--locked", "--release"], cwd=ROOT, check=True)
    ensure_offline_port_is_free()
    listener, stopped, thread = start_reachable_endpoint()
    try:
        with tempfile.TemporaryDirectory(prefix="ssh-tui-demo-") as temporary:
            resolver_library = Path(temporary) / "resolve_demo_hosts.so"
            subprocess.run(
                [
                    "cc",
                    "-shared",
                    "-fPIC",
                    "-o",
                    str(resolver_library),
                    str(ROOT / "demo/resolve_demo_hosts.c"),
                    "-ldl",
                ],
                check=True,
            )
            cast_path = Path(temporary) / "demo.cast"
            record(cast_path, resolver_library)
            subprocess.run(
                [
                    agg,
                    "--quiet",
                    "--font-family",
                    "Iosevka Nerd Font Mono",
                    "--font-size",
                    "14",
                    "--line-height",
                    "1.2",
                    "--theme",
                    "asciinema",
                    "--bold-is-bright",
                    "--fps-cap",
                    "12",
                    "--last-frame-duration",
                    "2",
                    str(cast_path),
                    str(ROOT / "assets/ssh-tui-demo.gif"),
                ],
                check=True,
            )
    finally:
        stopped.set()
        listener.close()
        thread.join(timeout=1)

    print("updated assets/ssh-tui-demo.gif")


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(130)
