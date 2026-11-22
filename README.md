# YopesCLI

A better coding CLI, built with Rust, Ratatui, and Tokio.

[![Repo size](https://img.shields.io/github/repo-size/Jamicaw56/YopesCLI)](https://github.com/Jamicaw56/YopesCLI)
[![License](https://img.shields.io/github/license/Jamicaw56/YopesCLI)](LICENSE)
[Discord](https://discord.gg/xrRAJKHAVP)

## Highlights

- Streaming custom OpenAI-compatible APIs, with per-tool approval and a diff viewer.
- `delegate` hands subtasks to parallel subagents, streamed live in the sidebar and readable with `/agents`.
- `computer_use` drives the desktop with a visible, click-through agent cursor, so your mouse never moves.
- Image pasting, thirteen themes, a popout command palette, and one-command self-updates (`yopes update`).
- Persistent sessions and tabs: Alt+H session browser, `/session`, `--resume`, Alt+1..9.

## Install

`npm install -g yopes` (Node 16+): the package downloads the release binary for
your platform, verifies its checksum, and installs the `yopes` command.

Download the latest build for your platform from [Releases](https://github.com/Jamicaw56/YopesCLI/releases): `yopes-Windows-X64.zip` (Windows) or `yopes-Linux-X64.tar.gz` (Linux), then install it with PowerShell on Windows:

```powershell
irm https://raw.githubusercontent.com/Jamicaw56/YopesCLI/main/install.ps1 | iex
```

The script picks the latest release, verifies its SHA256SUMS checksum, installs `yopes.exe` to `%LOCALAPPDATA%\Programs\yopes`, and adds it to your user PATH. To build or run from a checkout, see [CONTRIBUTING.md](CONTRIBUTING.md).

## First run

> **Provider setup supports custom APIs only.** Connect your own compatible API endpoint; there is no built-in provider directory.

1. `/provider connect` — add a custom API endpoint and model. Keys are stored per endpoint, so reconnecting never asks for the key again.
2. Type a message. Tools ask for approval before they change anything; press `Enter` to allow one, `a` for the session, `Esc` to deny.
3. `/` opens the command palette; `F1` shows keyboard help; `/set theme <name>` picks one of the thirteen built-in palettes (`dark`, `midnight`, `nord`, `gruvbox`, `dracula`, `tokyo`, `rose`, `catppuccin`, `one-dark`, `solarized`, `everforest`, `kanagawa`, `light`).

## Docs

The [wiki](https://github.com/Jamicaw56/YopesCLI/wiki) holds the detail:

- [Commands](https://github.com/Jamicaw56/YopesCLI/wiki/Commands) — every slash command.
- [Keys](https://github.com/Jamicaw56/YopesCLI/wiki/Keys) — the full keyboard and mouse map.
- [Connect](https://github.com/Jamicaw56/YopesCLI/wiki/Connect) — providers, key storage, `/disconnect`.
- [Tools](https://github.com/Jamicaw56/YopesCLI/wiki/Tools) — the tool surface, approval modes, plan mode.
- [Skills](https://github.com/Jamicaw56/YopesCLI/wiki/Skills) — `SKILL.md` procedure packs.
- [Configuration](https://github.com/Jamicaw56/YopesCLI/wiki/Configuration) — flags, environment, config file schema.
- [Troubleshooting](https://github.com/Jamicaw56/YopesCLI/wiki/Troubleshooting) — common problems.
- [Architecture](https://github.com/Jamicaw56/YopesCLI/wiki/Architecture) — the module map.

In the checkout: [Architecture](docs/ARCHITECTURE.md), [Adding a tool](docs/ADDING-A-TOOL.md), [Changelog](CHANGELOG.md), [Security policy](SECURITY.md).

## Known limitations
- **Image pasting**: `Ctrl+V` on Windows Terminal pastes text only, so a
  screenshot clipboard sends nothing. Use **`Ctrl+G`** to attach a clipboard
  image instead.

> [!WARNING]
> `Ctrl+V` does not attach images in Windows Terminal. Use `Ctrl+G`.

## Tests

Run `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` before a pull request; CI runs the same checks on Linux and Windows. See [CONTRIBUTING.md](CONTRIBUTING.md#tests).

## License

Distributed under GPL-3.0-only. See [LICENSE](LICENSE).
