# Changelog

All notable changes to YopesCLI are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- A real delegate system: the `delegate` tool now spawns a subagent run that
  streams live into a right-hand sidebar (status glyph, elapsed time, task
  excerpt, stacked as they start). Several delegate calls in one turn run in
  parallel, and the queue keeps advancing while they stream. `/agents` opens a
  filterable list of runs; picking one (or clicking it in the sidebar) opens a
  scrollable viewer with the task, the reasoning, and the full streamed
  response. Delegate tasks are capped at 60,000 characters so one call cannot
  silently burn the context.
- A write watchdog for subagent runs: after 8 tool rounds without a
  successful `write_file` / `edit_file` / `multi_edit`, a run is nudged once,
  mid-run, to stop exploring and land its first artifact; a task that is
  read-only by nature may ignore the nudge and continue. The subagent brief
  now tells every run to land changes and report what changed, and
  `docs/DELEGATION.md` documents the task-phrasing playbook for delegation
  that lands committed work.
- A `move_to` tool: the model changes the working directory that relative
  paths resolve against. The target is normalized and must be an existing
  directory; moving outside the workspace or the temp root asks for approval.
- Background user commands: `!cmd` and `/run cmd` run off the event loop
  instead of soft-locking it, and their output lands as its own message when
  the command finishes. `Esc` now interrupts a running command or tool: the
  foreground processes are killed and queued tools are discarded.

### Changed

- Subagent runs no longer stop after a tool-round cap: the 20-round limit
  and its budget nudge are gone. A run ends when the model stops calling
  tools, the stream failure ladder is exhausted, or the main agent (or user)
  stops it.
- Auto compaction is now probabilistic: above ~80% of the context window each
  turn rolls for compaction, with the chance rising to about half near 90% and
  certain at 100% (always past it), instead of firing on a fixed threshold.
- The `compacting context` status renders in the warning color, so it reads
  apart from the thinking caret, which stays in the accent color.

### Fixed

- A reply that hit the provider's output token cap (`finish_reason: "length"`,
  common when long reasoning uses the budget) used to end as a clean finish,
  so it just stopped with no warning. It now shows a provider error naming the
  limit. A turn that ends after thinking with no answer gets a footer notice,
  and a last stream line without a trailing newline is no longer dropped.
- Streams that stall or get cut short no longer end the turn: a silent
  connection (idle past 180s) or a provider closing the stream mid-reply now
  resumes the request automatically — up to three times per turn, keeping any
  partial text as the trailing assistant message so the model continues where
  it stopped — and only then reports the failure. A stream closing without a
  completion marker is detected instead of quietly truncating the reply and
  looking finished; providers that send `finish_reason` without `data: [DONE]`
  still end cleanly.
- The header shows the session file the active tab writes, so a crashed or
  stalled conversation can be resumed by pointing `--session` at it.

### Added

- `/revert` opens a picker over the prompts already sent this session, newest
  first, filterable as you type: picking one cuts the session back to that
  prompt (its reply and every later turn leave the history), and its text and
  any pasted images return to the composer for editing — sending then re-runs
  from there. Works with the keyboard, the mouse, and `F1` help.

- An npm package: `npm install -g @jamicaw56/yopes` downloads the release
  binary for `win32-x64` or `linux-x64`, verifies its SHA256 checksum against
  `SHA256SUMS`, and installs the `yopes` command through a bin shim. The
  Production workflow publishes it after the GitHub release via trusted
  publishing (OIDC), no stored npm token.

- Permission cycling via `Shift+Tab` (`on` · `yolo` · `ask`) and YOLO mode
  execution without interactive confirmation.


- Release builds are self-contained on Ubuntu runners: the Windows binary is
  cross-compiled with MinGW instead of running on the Windows hosted pool, so
  release creation no longer depends on Windows runner availability.

### Fixed

- A tab-bar click hit anywhere in a tab's column range switched tabs, even on
  a lower panel that shared the column range: clicking the Progress title to
  fold it on a second tab switched to tab one instead. Hit-boxes now carry the
  header row, so a click only switches tabs when it lands on the tab bar.
- Pasting an image path or a clipboard image now shows an `[Image N]`
  placeholder in the composer instead of nothing. The real path stays hidden
  in an internal map and is swapped back on submit (non-vision models get the
  path as text, vision models also get the image block); backspace deletes a
  whole placeholder with its map entry, and the numbering restarts with every
  sent prompt. `file://` URIs with percent escapes and shell-escaped spaces
  are understood. An empty paste event now reads the clipboard for an image,
  and `.webp`, `.gif` and `.bmp` file paths attach like the other formats.
- `/theme` is registered as a built-in command: the palette now shows it, `/t`
  completion offers it, and it runs while a response streams.

### Added

- Rich Markdown rendering in the transcript: headings, emphasis, links, fenced
  code with language labels, nested lists and task boxes, blockquotes, rules,
  and compact tables. The system prompt now asks the model for terminal-friendly
  Markdown.
- Per-provider key storage: API keys are saved per endpoint and reused across
  connections, so connecting a second time never asks for the key again.
- `/disconnect [name]` forgets a provider's stored key and model list, and
  resets the active connection when it matches.
- `/version` shows the version the app is running.
- `/model` switches persist with the connection.
- Provider logos from models.dev, rasterised and cached on disk, drawn above
  the connect list.
- An animated mascot: flicking tail, blinking, and a rolling ball of yarn.
- Visible retries: a footer line shows the attempt, the delay, and the reason
  while a failed request retries (bounded, honouring `Retry-After`).
- Read-only commands work while a response is streaming.
- The system prompt tells the model where scratch files belong
  (`%USERPROFILE%\Temp` on Windows).
- Help popup scrolls with ↑↓/PgUp/PgDn and the mouse wheel.

### Changed

- Session tabs each keep their own provider and model: switching models or
  connecting a provider in one tab never reaches another tab, and new tabs
  start from the current tab's provider.

- Split `src/app.rs` into `src/app/` modules (input, turns, streaming,
  `/connect`, palette, pickers, images, formatting, modes) and `src/ui.rs` into
  `src/ui/` modules, one per panel. No behaviour change.
- Only `AGENTS.md` files feed the project instructions; other Markdown files
  in the workspace are no longer sent to the model.
- Palette rows pad by display width, so CJK text and emoji keep their columns.
- The tool frame caret stays inside the frame's bottom border.
- `/tools` is deprecated and marked as such; it still works.
- Releases build for Linux and Windows only; the hosted macOS runners
  starved the publish job, so the macOS archive is gone until capacity
  improves. macOS still works from a checkout (`cargo build --release`).

## [0.1.0]

First public release.

### Added

- Streaming chat against OpenAI-compatible endpoints and Ollama, with custom
  base URLs.
- Provider and model discovery from [models.dev](https://models.dev), plus the
  Fasfas model list, through `/connect`.
- Tool calling with per-tool approval and a diff viewer: `list_dir`,
  `read_file`, `multi_read`, `glob`, `grep`, `write_file`, `edit_file`,
  `multi_edit`, `run_command`, `bash_background`, `bash_output`, `bash_kill`,
  `web_search`, `web_fetch`, `todo_write`, and `delegate`.
- Tool modes `on`, `off`, `auto`, and `ask`, plus build, plan, and chat turn
  modes.
- A native task list in a Progress panel, saved with the session.
- `SKILL.md` procedure packs in the Agent Skills format, discovered from
  `.yopes/skills`, `~/.yopes/skills`, and the Claude Code skill directories.
- One focused session, restored between runs, with `/sessions` to switch and
  `/compact` to summarize older turns.
- Slash commands, a popout command palette, a help overlay, scrollback, and
  cancellation.
- `/effort` reasoning-effort control, saved with the connection.
- `yopes update` self-update.

[Unreleased]: https://github.com/Jamicaw56/YopesCLI/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Jamicaw56/YopesCLI/releases/tag/v0.1.0
