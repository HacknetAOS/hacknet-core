# hacknet-core

[![CI](https://github.com/HacknetAOS/hacknet-core/actions/workflows/ci.yml/badge.svg)](https://github.com/HacknetAOS/hacknet-core/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Core system daemon, IPC protocol, CLI controller, and theme compiler for HacknetAOS.

## Workspace Crates

| Crate | Target | Description |
|---|---|---|
| [`hacknet-proto`](crates/hacknet-proto) | Library | Unix socket IPC schema, layout math, window classification, and theme parsing. |
| [`hacknetd`](crates/hacknetd) | `hacknetd` | Window placement daemon, NetMap state engine, and CAVA audio visualizer pipeline. |
| [`hacknetctl`](crates/hacknetctl) | `hacknetctl` | CLI client for workspace switching, window cycling, audio control, and notifications. |
| [`hacknet-theme`](crates/hacknet-theme) | `hacknet-theme` | Theme compiler that renders `Theme.sys` / TOML presets into Quickshell, foot, GTK, Qt, Hyprland, and Starship configs. |

## Build & Test

```bash
cargo build --release --locked
cargo test --all --locked
```

### Lint & Layout Parity

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
python3 tools/hacknet-layout.py --check
```

## IPC Socket

`hacknetd` listens on `$XDG_RUNTIME_DIR/hacknet.sock` (fallback `/tmp/hacknet-$UID/hacknet.sock`) using newline-delimited JSON requests and events defined in `hacknet-proto`.

## License

MIT. See [LICENSE](LICENSE).
