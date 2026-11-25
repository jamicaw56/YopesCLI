# Changelog

All notable changes to YopesCLI are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- A provider that refuses `reasoning_effort` (strict endpoints reject `xhigh`) no longer fails the turn: the request retries without the hint and remembers the refusal for that model.
- `/session` no longer lags: the sessions directory is read once when the picker opens instead of on every keystroke and frame.
- Custom providers that reject tool-call history now retry without tools instead of dumping a validation error.
- Sending a message in a large directory no longer freezes the CLI: AGENTS.md discovery is bounded and cached.
- Gemini tool calls replay their thought signature, so Google models keep working after the first tool call.

### Added

- `assets/icon-512.png` (plus 128/256/1024 and `icon.ico`): the blue pixel-Y mark, drawn on a 16×16 grid so it stays sharp at every size. Upload it to a Discord application as `yopes_logo`.
- Discord Rich Presence, off until `/set discord <application id>`: the card shows the workspace, mode and model, with the Y as its large image.
- `@path` mentions: typing `@` completes paths from the workspace, and the mentioned file is attached to the message when it is sent.
- `/models` is a top-level command again: pick a model, or switch by name.
- `/compact` summarizes older turns now, without going through `/set compact now`.
- `/export [file]` copies the transcript as Markdown for pasting into another session, or saves it to a new file (never over an existing one) that `@file` can attach.
- `/fork` copies the conversation into a new tab.
- `/init` hands the model the task of drafting `AGENTS.md` for the workspace, through the normal tool approvals.
- Hooks in `~/.yopes/hooks.json`: `PreToolUse` (a non-zero exit blocks the call), `PostToolUse` (output joins the result) and `Stop` run shell commands around tool calls.
- The footer shows billed input/output tokens once the provider reports usage.
- `/set weather <city>` (opt-in, `off` to stop) makes theme `auto` follow the forecast: rain and snow fall, clouds and fog gray the sky, storms darken it, and a clear day shows a sun that rises and sets. Checked every 15 minutes through Open-Meteo; only the city's coordinates are sent, and only while theme `auto` is on.
- `/set theme auto` moves the background with the clock: the palette and a sky gradient blend nine local-hour keyframes (midnight, pre-dawn, dawn, morning, noon, afternoon, sunset, evening), with stars that fade in at night, repainting on the minute even when the session is idle.
- Custom API setup is now a four-field quick form: name, completion URL, model ID, token. Models URL, auth, model name and context window default sensibly and stay editable later.
- The UI has a theme-aware ambient backdrop (soft vertical wash, sparse dot grid) and a branded header mark.
- `read_all` reads every text file under a directory recursively, honoring `.yopesignore`.
- `yopes --resume` continues a session that was cut off mid-turn instead of
  waiting for a typed prompt.
- `computer_use` tolerates partial cursor-ready signals and gives WPF 15 seconds to start.

### Changed

- `web_search` works without a Tavily key: it falls back to DuckDuckGo's no-JavaScript page, and a configured Tavily key still takes precedence.
- Images in tool results are sharper: they fit the frame width instead of being cropped on the right, and keep their aspect ratio.
- Saved custom providers show a name-based monogram instead of identical star placeholders.
- Provider setup is custom-API-only; the first-run screen and installers direct users to configure their own compatible API.
- Custom provider model fetch reconciles the saved list against the `/models`
  response: models the endpoint dropped are removed from disk too, and the
  hand-set context window of kept models survives.
- Credential failures stop retrying immediately instead of freezing the send screen.
- Landing shortcuts aligned; image paste hint is `Ctrl+G`.
- Provider errors say "provider" instead of "OpenAI-compatible".
- README: headless, ignore rules and fonts moved out; image paste warning added.
- Common tool-name slips (`bash`, `readfile`, `ls`, ...) resolve to the real tool.
- Permission (Shift+Tab) and diff view are saved and restored on restart.
- Read-only `/settings` and similar commands run while a tool is busy.
- Post-update popup lists the running version's changes once, with scrolling.
- `computer_use` clicks inject real input (`SendInput`) and restore the pointer,
  so Windows Terminal and browsers respond instead of ignoring the click.
- Context windows come from a LiteLLM snapshot (4800 ids) before the family
  rules, so new models get the right window.
- Config writes use a unique temp file with retries: two instances no longer
  clobber each other's save.
- `/goal` shows checklist progress in the footer (`goal 3/7 · 43%`).
- Pasted images render larger and sharper (Lanczos3, taller cap).

### Added

- Custom API-only provider setup: connect your own compatible endpoint and add or fetch model IDs.
- Custom providers can select Bearer, JWT, X-API-Key, or no authentication.
- MCP client: stdio servers listed in `~/.yopes/mcp.json` under `mcpServers`.
  Their tools appear as `mcp__<server>__<tool>` and ask for approval on every call.
- Global state lives in `~/.yopes/` (config, sessions, skills, `mcp.json`). The
  old per-OS config and session files are copied in on first run, not moved.
- The mode (`build`/`plan`/`chat`) sits in the footer beside the permission
  badge. The header keeps only the provider and model line.
- Removed the shortcut hint row under the composer.
- Custom provider model fetch replaces the list: models the endpoint no longer
  lists are dropped. A hand-set context window is kept for models that remain.
- `Alt+H` opens a centered, filterable session browser from both the landing
  screen and an active conversation. Empty sessions now show a compact grid of
  common keys, and a persistent hint above the footer names `Alt+H`, `/` and
  `F1`.
- `computer_use` drives the desktop with its own agent pointer. Mouse steps no
  longer move the system cursor: they post button messages to the window under
  the point, so the person at the keyboard keeps their mouse while the agent
  works. A real click-through, topmost desktop arrow follows every mouse step
  and stays visible between calls without taking focus or intercepting the
  user's clicks. It closes when yopes exits. The pointer is also drawn into
  each capture as a highlighted black arrow on a soft halo, and the result's
  `Cursor:` line names it instead of the system pointer.

- `/settings` opens a popup with every `/set` key as a row, so a setting can
  be changed with the arrow keys instead of memorising a command: `↑↓` picks
  the row, `←→` steps the value and saves it, `esc` closes. The rows drive the
  same setters the typed command does, so both paths agree. The permission
  row cycles on · yolo · ask · off and leaves `dangerous` out, which stays
  reachable by `/set tools dangerous` only.
- Four more palettes: `one-dark`, `solarized`, `everforest` and `kanagawa`
  join the roster (thirteen built-in themes).

### Changed

- The header uses the supplied Nerd Font mode icons: `` build, `` plan and
  `` chat.

- The welcome screen drops the mascot and names the workspace instead, with
  the home directory shortened to `~`. The idle welcome no longer ticks.
- The provider dialog was rebuilt as a form: the connection in use, a labeled
  field with its caret, a rule under it, the list, and per-stage hints in the
  border.
- Fenced code inside a model's thinking draws as a code box with the language
  named, instead of raw backticks.
- `/set tools dangerous` now tells the model explicitly that it is pre-approved
  for every step and must not ask for permission, while still following the
  repository's own instructions.
- The Progress panel follows the first unfinished task, marks it with `▶`, and
  the mouse wheel scrolls the checklist; folding hands the window back to
  follow mode.
- The streaming footer and the reply stats drop the `in Nk` token count.
- The README's highlights were trimmed to the essentials.
- The base system prompt gained a writing-style rule: answer first, concrete
  nouns, no filler, no recap, never claim an unverified result.

### Fixed

- Clicking a framed tool result or edit diff expands it again. The click map
  treated a multi-line frame's line offsets as slot indices, so the rows it
  registered pointed at the wrong lines or nowhere; an expanded frame now
  stays clickable too, so the same click collapses it.
- A multi-line paste no longer submits one prompt per line on terminals that
  lack bracketed paste: an Enter that follows another key within 20ms is
  inserted as a line break. Bracketed paste already inserted the whole block.
- A failed provider request now reaches the model as a `[provider error]` note
  in the next prompt (user role, all three request shapes) instead of being
  hidden from it.

- The session tabs moved out of the footer onto their own row under the
  header. A bar full of sessions no longer squeezes the permission badge and
  the context load, which keep the footer's right side to themselves.
- `/settings` gained a session mode row and a diff view row next to
  permission, theme, effort and auto compact: `←→` cycles build · plan · chat
  and side-by-side · unified. A list taller than the popup scrolls with the
  selection instead of clipping rows.
- The command palette is capped at ten rows and scrolls to follow the
  selection, showing its position in the border (`commands 5/12`), instead of
  growing over the transcript.

- A full visual redesign around a two-accent theme system. Every palette
  now defines two extra roles beyond the classic eight: `dim`, for
  separators, tool frames and hint rows that must sit quieter than `muted`
  text, and `accent2`, a secondary accent for the brand mark, the composer
  caret, links and highlights, so the primary accent stays reserved for
  interactive state. All six existing palettes were retuned around the new
  roles, and three joined the roster: `tokyo`, `rose`, and `catppuccin`
  (`/set theme`, nine built-ins total).
- The welcome screen's mascot trades the cat for a cursor owl: it scans its
  pupils like a block cursor hunting a prompt, blinks, lifts its brow on a
  once-per-cycle mew, and dozes with a floating z when the session sits
  idle.
- The divider above the composer fades from the secondary accent into the
  dim role across its width, so the input edge reads as one soft band
  instead of a hard rule.
- The approval dialog's border breathes between the two accents on the
  ticker instead of blinking bold: the eye is drawn without strobing. Run
  reads green, Deny red, and the dialog dot matches the header's mode dot.
- The Progress checklist swaps checkbox glyphs for a ✓/○ pair and its title
  row carries a tiny ■/□ meter beside the done/total ratio.
- Markdown headings read their depth from color: h1 takes the primary
  accent, h2 the secondary, deeper levels stay bold body text. Code fences,
  rules, quote bars and table borders move to the dim role; links take the
  secondary accent underlined.
- Popup chrome (pickers, dialogs, sidebar, help) drops its borders and hint
  rows to the dim role, so the list rows step forward. The help dialog's
  section headers take the secondary accent.
- The footer spinner takes the secondary accent while a turn runs, and
  compaction says its own color, so waiting, thinking and summarizing read
  apart at a glance. The header's mode dot becomes a ◆ and the ask/off
  badges drop to dim.

### Added

- Delegated runs are verified instead of trusted. A subagent saying it is
  done no longer finishes anything: every run moves pending → assigned →
  running → submitted → verifying → one terminal state (completed, failed,
  blocked, cancelled), and `completed` is reachable only through
  verification. The verdict comes from evidence yopes observes itself —
  byte-level file snapshots around each write, git worktree fingerprints
  around each command, exit codes, and refused calls — so a claim with no
  changes fails, a skipped `expect_files` path fails, and a red `cargo test`
  fails. `delegate` gained `kind`, `expect_files`, `verify_command` and
  `timeout_minutes` (default 45, max 240).
- The coordinator carries a delegation ledger in its system prompt on every
  request, rebuilt from records persisted in the session: outstanding runs,
  failed runs and their reasons survive compaction, tab switches and a
  restart. A run still active at restart is restored as interrupted instead
  of being assumed finished. Reports are structured (verdict, reasons,
  evidence, then the subagent's answer labelled as a claim) and a batch only
  wakes the model once, after the last sibling finishes.

### Fixed

- A truncated response body (`unexpected EOF during chunk size line`, a
  connection reset mid-body) was a hard error that ended the turn. It is now
  a resumable stall, so the existing resume ladder finishes the reply, and
  the resume no longer leaves an empty assistant turn that makes a provider
  reject the retry.
- A duplicate delegation report, a duplicate parked report and a re-sent
  talk message are each delivered once. A report whose owning session is
  gone no longer lands in whichever tab happens to be on screen.
- Esc now cancels the delegated runs the tab owns, and closing a tab stops
  its runs instead of stranding them; a cancelled run reports as cancelled
  and does not wake the model to announce success. A run task that panics
  reports a failure instead of staying "running" forever.
- Eviction drops the oldest finished run instead of a live one, and no
  longer shifts the run the viewer has open.
- `/provider connect` no longer names models.dev while it is not talking to
  models.dev: the loading row of a custom endpoint says which host it is
  fetching the model list from, and only the provider catalog names
  models.dev.
- The default context window is 256K instead of 128K: a model with no known
  limit is assumed to hold 256K tokens, which raises both the compaction point
  and the point where auto-compaction starts.
- Thinking from OpenAI-compatible proxies now shows: `delta.reasoning` is
  read alongside `delta.reasoning_content`, and inline `<think>...</think>`
  text in the content stream is split out as reasoning (tags split across
  chunks included).
- Session ids no longer collide when two tabs are created inside the same
  millisecond (a fast `alt+t`, or test processes starting together). The id
  folds a per-process counter and the process id into the wall clock, which
  stops `talk_to` from routing a message to the wrong tab.

### Added

- `/goal <text>`: an objective the model is told to reach "no matter what".
  It rides in every request's system note, ahead of the mode instruction, so
  compaction cannot drop it and every tab keeps its own; it persists in the
  session file until `/goal clear`. `/goal` with no argument shows the current
  one.
- `apply_patch` tool: one `*** Begin Patch` envelope adds, deletes, updates
  and renames files. Every hunk is matched in memory first (exact, then
  whitespace-tolerant), so a patch applies fully or touches nothing. It shows
  diffs in the approval dialog and transcript, asks before writing outside the
  workspace, and is blocked in plan mode.
- Ollama thinking models stream their reasoning: `think` is sent when a
  reasoning effort is set, and `message.thinking` lands in the thinking block.
- A `dangerous` permission mode (`/set tools dangerous`): every tool call runs
  unattended, `computer_use` included, so no approval prompt ever appears.
  It is deliberately outside the Shift+Tab cycle - a mode that hands the model
  the mouse and keyboard takes a typed command, not a key next to the one
  people press all day. The header shows `[DANGER]`, the build-mode note warns
  the model to take the least destructive action, and `/set tools on` or one
  Shift+Tab restores the prompts.
- A `computer_use` tool: drive the real desktop on Windows. One call runs a
  list of mouse and keyboard actions (move, click, double-click, scroll, drag,
  type text, press named keys or chords such as `ctrl+s`), then attaches a
  fresh screenshot of the result to the tool message, so a vision-capable
  model sees what its click did and can aim the next one. Coordinates are
  screen pixels, `screen` picks a display on multi-monitor desktops, the tool
  is denied in plan mode, every call asks for approval unless the user allows
  it with `a`, yolo or dangerous mode, and screenshots land in the temp root. The captured frame shows in the
  transcript as a capped pixel-art thumbnail.
- `computer_use` now aims by numbers as well as by eye: the result reports the
  captured frame's origin and size in screen pixels, the desktop bounds and
  the final cursor position, so a pixel picked in the image plus the frame
  origin is the coordinate to click. A `region` crops the capture to one
  rectangle (far easier for a vision model to read than a whole 4K desktop),
  `settle_ms` sets how long the desktop gets to repaint before the second
  screenshot, and two new steps joined the list: `wait` pauses between two
  clicks for an app that needs a moment, and `focus_window` brings a window
  whose title contains a given string to the front before the keys start
  typing into it. Waits are capped at 10 s per step, and a capture whose
  geometry cannot be read is refused instead of guessed at.
- A `.yopesignore` file at the workspace root filters the filesystem walks:
  `glob`, `grep`, `list_dir` and `multi_read` with `all: true` skip whatever it
  names, so build output and vendored trees stop eating the context. The format
  is gitignore's (comments, `!` re-include, trailing `/` for directories, `/`
  to anchor, `*`, `?`, `**`), rules resolve deepest-match-first, the built-in
  skips stay in force, and with no file present nothing changes.
- A headless one-shot mode: `yopes run "task text"` submits one task, runs the
  turn chain until the agent is idle, and exits — `0` on success, non-zero when
  the provider fails. It needs no tty, so CI runners can drive the agent. Tools
  run unattended (YOLO mode), since nothing can answer an approval prompt, and
  the usual flags (`--no-save`, `--system-prompt`, `--model`, `--session`, …)
  are accepted before or after the subcommand.
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
- Auto-continue: a reply the provider stops at its output token cap
  (`finish_reason: "length"`) no longer ends the turn with an error asking the
  user to type "continue". The stream raises a typed `OutputLimit` event
  instead of a failure, the partial answer stays in the transcript, and a
  continuation prompt asks the model to carry on from where it stopped — up to
  three times per turn. When that budget runs out the cap is reported in the
  reply block, with the `/set effort` hint for reasoning-heavy models.

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
- The header no longer prints the session file the active tab writes, and the
  idle footer no longer prints the shortcut hint line: the mode and permission
  badges in the header already say what the hint repeated, and the shortcuts
  stay in `F1` help.
- A model with no known price — a provider catalog entry with no cost and no
  built-in price — no longer prints a `—` placeholder in the header: the spend
  segment is dropped, leaving the provider, model and context load.
- The busy spinner is now one turning braille ring everywhere it appears: the
  footer status, the session tabs, the subagent sidebar, the transcript's
  delegate rows, the model picker and `/connect` all draw the same eight-frame
  ring from one clock. The tabs used to park a static `◐` while the rest cycled
  four half-circles.
- The slash-command list is down from twenty commands to ten, grouped by what
  they act on: `/session` (list, new, tab, rename, close, next, prev, 1-9)
  absorbs `/new`, `/sessions`, `/tab` and `/rename`; `/provider` (connect,
  disconnect, models) absorbs `/connect`, `/disconnect` and `/models`; `/set`
  (tools, theme, system, compact, effort) absorbs `/tools`, `/theme`, `/system`,
  `/compact` and `/effort`; the version now shows in the `F1` help title
  instead of `/version`. `Ctrl+R` opens `/session rename` prefilled with the
  current title, and `/set` with no key lists every setting and its value.

### Added

- Delegated runs are verified instead of trusted. A subagent saying it is
  done no longer finishes anything: every run moves pending → assigned →
  running → submitted → verifying → one terminal state (completed, failed,
  blocked, cancelled), and `completed` is reachable only through
  verification. The verdict comes from evidence yopes observes itself —
  byte-level file snapshots around each write, git worktree fingerprints
  around each command, exit codes, and refused calls — so a claim with no
  changes fails, a skipped `expect_files` path fails, and a red `cargo test`
  fails. `delegate` gained `kind`, `expect_files`, `verify_command` and
  `timeout_minutes` (default 45, max 240).
- The coordinator carries a delegation ledger in its system prompt on every
  request, rebuilt from records persisted in the session: outstanding runs,
  failed runs and their reasons survive compaction, tab switches and a
  restart. A run still active at restart is restored as interrupted instead
  of being assumed finished. Reports are structured (verdict, reasons,
  evidence, then the subagent's answer labelled as a claim) and a batch only
  wakes the model once, after the last sibling finishes.

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

### Added

- Delegated runs are verified instead of trusted. A subagent saying it is
  done no longer finishes anything: every run moves pending → assigned →
  running → submitted → verifying → one terminal state (completed, failed,
  blocked, cancelled), and `completed` is reachable only through
  verification. The verdict comes from evidence yopes observes itself —
  byte-level file snapshots around each write, git worktree fingerprints
  around each command, exit codes, and refused calls — so a claim with no
  changes fails, a skipped `expect_files` path fails, and a red `cargo test`
  fails. `delegate` gained `kind`, `expect_files`, `verify_command` and
  `timeout_minutes` (default 45, max 240).
- The coordinator carries a delegation ledger in its system prompt on every
  request, rebuilt from records persisted in the session: outstanding runs,
  failed runs and their reasons survive compaction, tab switches and a
  restart. A run still active at restart is restored as interrupted instead
  of being assumed finished. Reports are structured (verdict, reasons,
  evidence, then the subagent's answer labelled as a claim) and a batch only
  wakes the model once, after the last sibling finishes.

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
