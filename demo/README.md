# Recording the README demo

The demo uses the real release build with a deterministic SSH shim. A tiny
recording-only resolver maps the fictional `*.northstar.internal` inventory to
the local mock endpoint. The recorder opens TCP port `2222` so the normal demo
hosts pass the application's real reachability check. Port `2223` stays closed,
making `dr-gateway-01` visibly unreachable.

The keystroke labels are injected into two extra rows in the recording only.
They are not part of `ssh-tui-rs` and do not affect the real application.

Requirements:

- Rust and Cargo
- Python 3
- A C compiler for the recording-only hostname resolver
- [`agg`](https://docs.asciinema.org/manual/agg/)
- Iosevka Nerd Font Mono (or change `--font-family` in `record.py`)

Regenerate the asset from the repository root:

```bash
python3 demo/record.py
```

Set `AGG=/path/to/agg` when the renderer is not in `PATH`. The generated GIF is
written to `assets/ssh-tui-demo.gif`.
