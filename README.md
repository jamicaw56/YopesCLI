# YopesCLI

A better coding CLI, built with Rust, Ratatui, and Tokio.

[![Repo size](https://img.shields.io/github/repo-size/Jamicaw56/YopesCLI)](https://github.com/Jamicaw56/YopesCLI)
[![License](https://img.shields.io/github/license/Jamicaw56/YopesCLI)](LICENSE)
[Discord](https://discord.gg/xrRAJKHAVP)

## Highlights

- Streaming OpenAI-compatible providers, custom endpoints included, with per-tool approval and a diff viewer.
- `delegate` hands subtasks to parallel subagents, streamed live in the sidebar and readable with `/agents`.
- `computer_use` drives the desktop with its own agent cursor, so your mouse never moves.
- Image pasting, thirteen themes, a popout command palette, and one-command self-updates (`yopes update`).
- Persistent sessions and tabs: `/session`, `--resume`, Alt+1..9.

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
3. `/` opens the command palette; `F1` shows keyboard help; `/set theme <name>` picks one of the thirteen built-in palettes (`dark`, `midnight`, `nord`, `gruvbox`, `dracula`, `tokyo`, `rose`, `catppuccin`, `one-dark`, `solarized`, `everforest`, `kanagawa`, `light`).

## Headless runs

`yopes run "task text"` sends one task and exits when the agent is idle: no
interface, one turn chain, exit code `0` on success and non-zero when the
provider fails. The usual flags (`--no-save`, `--system-prompt`, `--model`,
`--session`, …) work and may come before or after `run`. Nothing can answer an
approval prompt in a headless run, so tools run unattended (YOLO mode).

## Ignore rules

A `.yopesignore` file at the workspace root hides paths from the tools that
walk the tree: `glob`, `grep`, `list_dir`, and `multi_read` with `all: true`.
The format is gitignore's: one pattern per line, `#` comments, `!` to
re-include, a trailing `/` for directories only, `/` anywhere in the pattern to
anchor it to the root, and `*`, `?` and `**` inside a segment. Nothing is
filtered without the file, and the built-in skips (`target`, `node_modules`,
`.git`, virtualenvs, caches) stay in force either way.

```
node_modules/
dist/
*.log
!keep.log
```

## Fonts

The interface is text, so the terminal's monospace font does the styling. Pick
one with full box-drawing and braille coverage (frames, spinners, diff rulers):
JetBrains Mono, Cascadia Code, Fira Code, or Iosevka. Any of them works out of
the box; a Nerd Font build adds icons but is not required. In Windows Terminal
set it under **Settings → Profiles → Appearance → Font face**, in VS Code's
integrated terminal under `terminal.integrated.fontFamily`. A font without
braille or CJK glyphs shows replacement boxes; that is the terminal's font, not
the app.

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
