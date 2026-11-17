# YopesCLI

A better coding CLI, built with Rust, Ratatui, and Tokio. [Discord](https://discord.gg/xrRAJKHAVP) Join ig if you want

## Highlights

- Streaming OpenAI-compatible providers, including custom endpoints.
- Tool calling for reading, writing, editing, and running commands, with per-tool approval and a diff viewer.
- A real delegate system: `delegate` hands a subtask to a subagent on the same provider. Runs stream live in a right-hand sidebar, stack in parallel, and every response is readable with `/agents`.
- Image pasting: `Ctrl+V` and right-click attach clipboard images, file paths paste as attachments, and `read_image` sends screenshots to vision models.
- A quiet, minimal interface with a popout command palette.
- Provider and model discovery from [models.dev](https://models.dev) plus custom endpoints with `/provider`.
- One-command self-updates with `yopes update`.
- Persistent sessions with `/session`, fresh session on launch, `--resume` to continue.

## Install

`npm install -g yopes` (Node 16+): the package downloads the release binary for
your platform, verifies its checksum, and installs the `yopes` command.

Download the latest build for your platform from [Releases](https://github.com/Jamicaw56/YopesCLI/releases): `yopes-Windows-X64.zip` (Windows) or `yopes-Linux-X64.tar.gz` (Linux), then install it with PowerShell on Windows:

```powershell
irm https://raw.githubusercontent.com/Jamicaw56/YopesCLI/main/install.ps1 | iex
```

The script picks the latest release, verifies its SHA256SUMS checksum, installs `yopes.exe` to `%LOCALAPPDATA%\Programs\yopes`, and adds it to your user PATH. To build or run from a checkout, see [CONTRIBUTING.md](CONTRIBUTING.md).

## First run

1. `/provider` — pick a provider and model. Keys are stored per endpoint, so connecting a second time never asks for the key again.
2. Type a message. Tools ask for approval before they change anything; press `Enter` to allow one, `a` for the session, `Esc` to deny.
3. `/` opens the command palette; `F1` shows keyboard help.

## Headless runs

`yopes run "task text"` sends one task and exits when the agent is idle: no
interface, one turn chain, exit code `0` on success and non-zero when the
provider fails. The usual flags (`--no-save`, `--system-prompt`, `--model`,
`--session`, …) work and may come before or after `run`. Nothing can answer an
approval prompt in a headless run, so tools run unattended (YOLO mode).

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

- **`Ctrl+V` on Windows Terminal** performs the terminal's own text-only
  paste, so a screenshot-only clipboard sends nothing to the app. Use
  **right-click** to paste a clipboard image, or paste an image file path as
  plain text; both attach the image for vision models.

## Tests

Run `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` before a pull request; CI runs the same checks on Linux and Windows. See [CONTRIBUTING.md](CONTRIBUTING.md#tests).

## License

Distributed under GPL-3.0-only. See [LICENSE](LICENSE).
