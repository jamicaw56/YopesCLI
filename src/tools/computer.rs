//! `computer_use`: drive the desktop and show the model what happened.
//!
//! A call takes one screenshot, then runs its `actions` in order: mouse moves,
//! clicks, scrolls, drags, typed text, key presses, key chords, waits and
//! window focus changes. A second screenshot after the actions is written to
//! the temp root, where the app attaches it to the tool message so a vision
//! model sees the result. The result also names where the captured frame sits
//! on the desktop and where the pointer ended up, so the next call can aim.
//!
//! Windows only. The Windows implementation drives the desktop through
//! `System.Windows.Forms` (mouse and typing) and `System.Drawing` (screen
//! capture) from one PowerShell script, so the binary links no platform
//! libraries. Everywhere else the tool answers with a plain error instead of
//! pretending it worked.

use serde::Deserialize;

use super::{parse, truncate_output};

#[derive(Debug, Deserialize)]
struct ComputerUseArgs {
    /// Free-form description of what the call is trying to do; shown to the
    /// user as the preview and kept in the result.
    #[serde(default)]
    action: String,
    #[serde(default)]
    actions: Vec<Step>,
    /// The screen to capture, for multi-monitor desktops: 0 is the primary.
    #[serde(default)]
    screen: u32,
    /// Capture only part of the screen, in screen pixels. A vision model
    /// reads a small region far better than a whole 4K desktop, and the crop
    /// is what the click coordinates are relative to as well.
    #[serde(default)]
    region: Option<Region>,
    /// How long to let the desktop paint after the actions, in milliseconds.
    /// Defaults to 350; raise it for slow apps, and use a `wait` step when
    /// the delay belongs between two clicks.
    #[serde(default)]
    settle_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
struct Region {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

/// The fields are read by the Windows script builder and by the tests. A
/// non-test Linux build has neither, so the enum is allowed to keep them:
/// they are what `serde` deserializes the model's arguments into.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
enum Step {
    /// Move the pointer, in screen pixels.
    Move { x: i32, y: i32 },
    /// Move the pointer, then press and release a button.
    Click {
        x: i32,
        y: i32,
        #[serde(default)]
        button: Button,
        #[serde(default = "one_click")]
        count: u8,
    },
    /// Two clicks at one spot.
    DoubleClick { x: i32, y: i32 },
    /// Turn one or two notches at a time.
    Scroll {
        #[serde(default)]
        x: i32,
        #[serde(default)]
        y: i32,
        #[serde(default)]
        amount: i32,
    },
    /// Press, move, release: selecting text or moving a window.
    Drag {
        from_x: i32,
        from_y: i32,
        to_x: i32,
        to_y: i32,
        #[serde(default)]
        button: Button,
    },
    /// Type text into whatever holds focus.
    Type { text: String },
    /// Press named keys, or a chord such as `ctrl+s` or `alt+tab`.
    Key {
        #[serde(default)]
        keys: Vec<String>,
        #[serde(default)]
        chord: String,
    },
    /// Pause between two steps, for an app that needs a moment to open.
    Wait {
        #[serde(default = "default_wait_ms")]
        ms: u64,
    },
    /// Bring the window whose title contains this text to the front, so the
    /// keys that follow land in it instead of wherever focus was.
    FocusWindow { title: String },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

const fn one_click() -> u8 {
    1
}

const fn default_wait_ms() -> u64 {
    500
}

/// The most steps one call may carry: a runaway plan must not hold the
/// pointer for minutes.
const MAX_STEPS: usize = 32;
/// The longest text one `type` step may send.
const MAX_TYPE_CHARS: usize = 4_000;
/// The longest one `wait` step may pause, or one call may settle after its
/// actions.
const MAX_WAIT_MS: u64 = 10_000;
/// How long the desktop gets to repaint after the actions, by default.
const DEFAULT_SETTLE_MS: u64 = 350;

pub(super) fn computer_use(arguments: &str) -> anyhow::Result<String> {
    let args: ComputerUseArgs = parse(arguments)?;
    let intent = args.action.trim().to_owned();
    if args.actions.len() > MAX_STEPS {
        anyhow::bail!(
            "computer_use takes at most {MAX_STEPS} actions per call (got {}); split the work",
            args.actions.len()
        );
    }
    for step in &args.actions {
        match step {
            Step::Type { text } => {
                let chars = text.chars().count();
                if chars > MAX_TYPE_CHARS {
                    anyhow::bail!(
                        "one type action may send at most {MAX_TYPE_CHARS} characters (got {chars})"
                    );
                }
            }
            Step::Wait { ms } if *ms > MAX_WAIT_MS => {
                anyhow::bail!("one wait action may pause at most {MAX_WAIT_MS} ms (got {ms})");
            }
            Step::FocusWindow { title } if title.trim().is_empty() => {
                anyhow::bail!("a focus_window action needs a title to look for");
            }
            _ => {}
        }
    }
    if args
        .region
        .is_some_and(|region| region.width == 0 || region.height == 0)
    {
        anyhow::bail!("region needs a width and a height above zero");
    }
    if args.actions.is_empty() && intent.is_empty() {
        anyhow::bail!("computer_use needs an action: describe it, or list the actions to run");
    }
    let settle = args.settle_ms.unwrap_or(DEFAULT_SETTLE_MS).min(MAX_WAIT_MS);

    // The "before" capture proves the desktop answered; the model receives
    // only the state after the actions, one screenshot per call.
    let before = screenshot(args.screen, args.region)?;
    // A wait step writes into the same PowerShell run as the clicks around
    // it, so one call can wait for a window to open instead of the model
    // guessing a settle time from outside.
    let ran = run_actions(&args.actions)?;
    // The pointer and focus moved, so give the desktop a moment to paint the
    // result before the second capture; the wait is what makes the model see
    // the new state instead of the old one.
    if !args.actions.is_empty() && settle > 0 {
        std::thread::sleep(std::time::Duration::from_millis(settle));
    }
    let after = screenshot(args.screen, args.region)?;

    let mut report = String::new();
    report.push_str("Action: ");
    report.push_str(if intent.is_empty() {
        "(no description)"
    } else {
        intent.as_str()
    });
    report.push('\n');
    report.push_str("Actions run: ");
    report.push_str(&joined_actions(&ran));
    report.push('\n');
    // The geometry is what makes the next call aimable: where the captured
    // frame sits in screen pixels, how big the desktop is, and where the
    // pointer ended up after the steps.
    report.push_str(&before.describe());
    report.push_str(&after.cursor_line());
    report.push_str("After: ");
    report.push_str(&after.path.display().to_string());
    Ok(truncate_output(report))
}

fn joined_actions(ran: &[String]) -> String {
    if ran.is_empty() {
        "none".to_owned()
    } else {
        ran.join("; ")
    }
}

/// The screenshot path a `computer_use` result points at, if it has one: the
/// app attaches that PNG to the tool message for vision models.
pub(crate) fn screenshot_path(content: &str) -> Option<String> {
    content
        .lines()
        .find_map(|line| line.strip_prefix("After: "))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
}

#[cfg(any(windows, test))]
fn describe(step: &Step) -> String {
    match step {
        Step::Move { x, y } => format!("move to ({x}, {y})"),
        Step::Click {
            x,
            y,
            button,
            count,
        } => format!("click {}x {button:?} at ({x}, {y})", (*count).max(1)),
        Step::DoubleClick { x, y } => format!("double-click at ({x}, {y})"),
        Step::Scroll { x, y, amount } => format!("scroll {} at ({x}, {y})", signed(*amount)),
        Step::Drag {
            from_x,
            from_y,
            to_x,
            to_y,
            ..
        } => format!("drag ({from_x}, {from_y}) to ({to_x}, {to_y})"),
        Step::Type { text } => format!("type {} characters", text.chars().count()),
        Step::Key { chord, .. } if !chord.is_empty() => format!("press {chord}"),
        Step::Key { keys, .. } => format!("press {}", keys.join("+")),
        Step::Wait { ms } => format!("wait {ms} ms"),
        Step::FocusWindow { title } => format!("focus window `{title}`"),
    }
}

#[cfg(any(windows, test))]
fn signed(value: i32) -> String {
    if value > 0 {
        format!("+{value}")
    } else {
        value.to_string()
    }
}

#[cfg(windows)]
fn run_actions(steps: &[Step]) -> anyhow::Result<Vec<String>> {
    if steps.is_empty() {
        return Ok(Vec::new());
    }
    let script = windows_script(steps)?;
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &script,
        ])
        .output()
        .map_err(|error| anyhow::anyhow!("running powershell to drive the desktop: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let text = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        anyhow::bail!("computer_use failed: {}", text.trim());
    }
    Ok(steps.iter().map(describe).collect())
}

#[cfg(not(windows))]
fn run_actions(steps: &[Step]) -> anyhow::Result<Vec<String>> {
    let _ = steps;
    anyhow::bail!("computer_use is only implemented on Windows")
}

/// One captured PNG and the geometry it came from. On a plain non-Windows
/// build nothing constructs it, so its fields are allowed to sit unread.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
struct Shot {
    path: std::path::PathBuf,
    origin: (i32, i32),
    size: (u32, u32),
    desktop: (i32, i32, u32, u32),
    cursor: Option<(i32, i32)>,
}

/// What the capture script reports: the frame's origin and size, the whole
/// desktop's bounds, and where the pointer sits.
#[cfg(any(windows, test))]
struct Geometry {
    origin: (i32, i32),
    size: (u32, u32),
    desktop: (i32, i32, u32, u32),
    cursor: Option<(i32, i32)>,
}

impl Shot {
    /// The geometry lines of the report. A pixel picked in the image plus the
    /// frame origin gives a screen coordinate, and the desktop bounds tell
    /// the model how much room the frame sits in.
    fn describe(&self) -> String {
        let (x, y) = self.origin;
        let (width, height) = self.size;
        let (dx, dy, dw, dh) = self.desktop;
        format!(
            "Frame: ({x}, {y}) {width}x{height} screen pixels\nDesktop: ({dx}, {dy}) {dw}x{dh}\n"
        )
    }

    fn cursor_line(&self) -> String {
        match self.cursor {
            Some((x, y)) => format!("Cursor: ({x}, {y})\n"),
            None => String::new(),
        }
    }
}

/// Capture `screen`, cropped to `region` when one is given, to a PNG in the
/// temp root, and report where that frame sits on the desktop.
#[cfg(windows)]
fn screenshot(screen: u32, region: Option<Region>) -> anyhow::Result<Shot> {
    let path = crate::temp::unique_path("yopes-screen", "png");
    let quoted = ps_quote(&path.display().to_string());
    let (region_x, region_y, region_w, region_h) = match region {
        Some(region) => (
            region.x,
            region.y,
            i64::from(region.width),
            i64::from(region.height),
        ),
        None => (0, 0, 0, 0),
    };
    // VirtualScreen covers every monitor; a per-screen capture selects one
    // display's bounds so multi-monitor desktops can be addressed by index,
    // and a `region` crops further. The script prints one `geometry` line
    // holding the desktop bounds, the frame's origin and size, and the
    // cursor, so the model can turn an image pixel into a screen pixel
    // without a second call.
    let script = format!(
        r#"Add-Type -AssemblyName System.Windows.Forms;
Add-Type -AssemblyName System.Drawing;
$desktop = [System.Windows.Forms.SystemInformation]::VirtualScreen;
$bounds = $desktop;
$index = {screen};
if ($index -gt 0) {{
  $screens = [System.Windows.Forms.Screen]::AllScreens;
  if ($index -ge $screens.Count) {{ [Console]::Error.Write('no screen ' + $index); exit 3 }}
  $bounds = $screens[$index].Bounds;
}}
if ({region_w} -gt 0) {{
  $bounds = New-Object System.Drawing.Rectangle({region_x}, {region_y}, {region_w}, {region_h});
}}
$bmp = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height);
$gfx = [System.Drawing.Graphics]::FromImage($bmp);
$gfx.CopyFromScreen($bounds.X, $bounds.Y, 0, 0, $bounds.Size);
$bmp.Save({quoted}, [System.Drawing.Imaging.ImageFormat]::Png);
$gfx.Dispose();
$bmp.Dispose();
$cursor = '-';
try {{
  $at = [System.Windows.Forms.Cursor]::Position;
  $cursor = ('' + $at.X + ',' + $at.Y);
}} catch {{ }}
Write-Output ('geometry ' + $desktop.X + ' ' + $desktop.Y + ' ' + $desktop.Width + ' ' + $desktop.Height + ' ' + $bounds.X + ' ' + $bounds.Y + ' ' + $bounds.Width + ' ' + $bounds.Height + ' ' + $cursor);
exit 0;"#
    );
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &script,
        ])
        .output()
        .map_err(|error| anyhow::anyhow!("running powershell to capture the screen: {error}"))?;
    if !output.status.success() || !path.is_file() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("could not capture the screen: {}", stderr.trim().to_owned());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let geometry = parse_geometry(&stdout).ok_or_else(|| {
        anyhow::anyhow!(
            "could not read the screen geometry from `{}`",
            stdout.trim()
        )
    })?;
    Ok(Shot {
        path,
        origin: geometry.origin,
        size: geometry.size,
        desktop: geometry.desktop,
        cursor: geometry.cursor,
    })
}

/// Read the `geometry` line the capture script prints: desktop x/y/w/h,
/// frame x/y/w/h, then `x,y` for the cursor or `-` when it is unknown. A
/// missing or short line is refused rather than guessed at, because a wrong
/// origin would make the next click land somewhere else.
#[cfg(any(windows, test))]
fn parse_geometry(stdout: &str) -> Option<Geometry> {
    let line = stdout
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("geometry "))?;
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 9 {
        return None;
    }
    let number = |index: usize| -> Option<i64> { parts.get(index)?.parse().ok() };
    let cursor_token = *parts.get(8)?;
    let cursor = if cursor_token == "-" {
        None
    } else {
        let (x, y) = cursor_token.split_once(',')?;
        Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
    };
    Some(Geometry {
        desktop: (
            i32::try_from(number(0)?).ok()?,
            i32::try_from(number(1)?).ok()?,
            u32::try_from(number(2)?).ok()?,
            u32::try_from(number(3)?).ok()?,
        ),
        origin: (
            i32::try_from(number(4)?).ok()?,
            i32::try_from(number(5)?).ok()?,
        ),
        size: (
            u32::try_from(number(6)?).ok()?,
            u32::try_from(number(7)?).ok()?,
        ),
        cursor,
    })
}

#[cfg(not(windows))]
fn screenshot(_screen: u32, _region: Option<Region>) -> anyhow::Result<Shot> {
    anyhow::bail!("computer_use is only implemented on Windows")
}

/// A PowerShell single-quoted literal: `'` doubles, so a path or a piece of
/// typed text cannot end the string early.
#[cfg(any(windows, test))]
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// The script that performs every step in order. Coordinates come from the
/// model as plain integers, and text only crosses as a quoted literal.
#[cfg(any(windows, test))]
fn windows_script(steps: &[Step]) -> anyhow::Result<String> {
    let mut script = String::from(
        "Add-Type -AssemblyName System.Windows.Forms;\n\
         Add-Type -AssemblyName System.Drawing;\n\
         Add-Type @'\n\
         using System;\n\
         using System.Runtime.InteropServices;\n\
         public static class YopesInput {\n\
           [DllImport(\"user32.dll\")] public static extern bool SetCursorPos(int x, int y);\n\
           [DllImport(\"user32.dll\")] public static extern void mouse_event(uint flags, uint dx, uint dy, int data, UIntPtr extra);\n\
           public const uint LeftDown = 0x0002;\n\
           public const uint LeftUp = 0x0004;\n\
           public const uint RightDown = 0x0008;\n\
           public const uint RightUp = 0x0010;\n\
           public const uint MiddleDown = 0x0020;\n\
           public const uint MiddleUp = 0x0040;\n\
           public const uint Wheel = 0x0800;\n\
         }\n\
         '@;\n\
         Add-Type @'\n\
         using System;\n\
         using System.Runtime.InteropServices;\n\
         public static class YopesWindow {\n\
           [DllImport(\"user32.dll\")] public static extern bool SetForegroundWindow(IntPtr window);\n\
           [DllImport(\"user32.dll\")] public static extern bool ShowWindow(IntPtr window, int command);\n\
         }\n\
         '@;\n\
         function OnTitle([string]$needle) {\n\
           $found = Get-Process -ErrorAction SilentlyContinue | Where-Object { try { $_.MainWindowTitle -and $_.MainWindowTitle.ToLower().Contains($needle.ToLower()) } catch { $false } } | Select-Object -First 1;\n\
           if (-not $found) { [Console]::Error.Write('no window title contains ' + $needle); exit 4 }\n\
           [void][YopesWindow]::ShowWindow($found.MainWindowHandle, 9);\n\
           [void][YopesWindow]::SetForegroundWindow($found.MainWindowHandle);\n\
         }\n\
         $ErrorActionPreference = 'Stop';\n",
    );
    for step in steps {
        match step {
            Step::Move { x, y } => {
                script.push_str(&format!(
                    "[void][YopesInput]::SetCursorPos({x}, {y}); Start-Sleep -Milliseconds 60;\n"
                ));
            }
            Step::Click {
                x,
                y,
                button,
                count,
            } => {
                let (down, up) = click_flags(*button);
                let count = (*count).clamp(1, 3);
                script.push_str(&format!(
                    "[void][YopesInput]::SetCursorPos({x}, {y}); Start-Sleep -Milliseconds 60;\n"
                ));
                script.push_str(&format!(
                    "for ($i = 0; $i -lt {count}; $i++) {{ [YopesInput]::mouse_event([YopesInput]::{down}, 0, 0, 0, [UIntPtr]::Zero); [YopesInput]::mouse_event([YopesInput]::{up}, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 90 }}\n"
                ));
            }
            Step::DoubleClick { x, y } => {
                script.push_str(&format!(
                    "[void][YopesInput]::SetCursorPos({x}, {y}); Start-Sleep -Milliseconds 60;\n\
                     for ($i = 0; $i -lt 2; $i++) {{ [YopesInput]::mouse_event([YopesInput]::LeftDown, 0, 0, 0, [UIntPtr]::Zero); [YopesInput]::mouse_event([YopesInput]::LeftUp, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 90 }}\n"
                ));
            }
            Step::Scroll { x, y, amount } => {
                let delta = (*amount).clamp(-10, 10) * 120;
                script.push_str(&format!(
                    "[void][YopesInput]::SetCursorPos({x}, {y}); Start-Sleep -Milliseconds 60;\n\
                     [YopesInput]::mouse_event([YopesInput]::Wheel, 0, 0, {delta}, [UIntPtr]::Zero);\n\
                     Start-Sleep -Milliseconds 120;\n"
                ));
            }
            Step::Drag {
                from_x,
                from_y,
                to_x,
                to_y,
                button,
            } => {
                let (down, up) = click_flags(*button);
                script.push_str(&format!(
                    "[void][YopesInput]::SetCursorPos({from_x}, {from_y}); Start-Sleep -Milliseconds 80;\n\
                     [YopesInput]::mouse_event([YopesInput]::{down}, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 80;\n\
                     [void][YopesInput]::SetCursorPos({to_x}, {to_y}); Start-Sleep -Milliseconds 80;\n\
                     [YopesInput]::mouse_event([YopesInput]::{up}, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 120;\n"
                ));
            }
            Step::Type { text } => {
                script.push_str(&format!(
                    "[System.Windows.Forms.SendKeys]::SendWait({});\nStart-Sleep -Milliseconds 120;\n",
                    send_keys_literal(text)
                ));
            }
            Step::Key { keys, chord } => {
                let literal = if !chord.trim().is_empty() {
                    chord_literal(chord)?
                } else {
                    keys_literal(keys)?
                };
                script.push_str(&format!(
                    "[System.Windows.Forms.SendKeys]::SendWait('{literal}');\n\
                     Start-Sleep -Milliseconds 120;\n"
                ));
            }
            Step::Wait { ms } => {
                script.push_str(&format!(
                    "Start-Sleep -Milliseconds {};\n",
                    (*ms).min(MAX_WAIT_MS)
                ));
            }
            Step::FocusWindow { title } => {
                script.push_str(&format!(
                    "OnTitle({});\nStart-Sleep -Milliseconds 150;\n",
                    ps_quote(title)
                ));
            }
        }
    }
    script.push_str("exit 0;\n");
    Ok(script)
}

#[cfg(any(windows, test))]
fn click_flags(button: Button) -> (&'static str, &'static str) {
    match button {
        Button::Left => ("LeftDown", "LeftUp"),
        Button::Right => ("RightDown", "RightUp"),
        Button::Middle => ("MiddleDown", "MiddleUp"),
    }
}

/// A literal for `SendKeys.SendWait`: braces wrap a single character so the
/// key names cannot collide with it. Braces, brackets, plus, caret, percent
/// and tilde have meanings of their own, so they are wrapped one by one.
/// Typing goes character by character; a newline becomes a real `ENTER`.
#[cfg(any(windows, test))]
fn send_keys_literal(text: &str) -> String {
    let mut literal = String::from("'");
    for character in text.chars() {
        match character {
            '\r' => {}
            '\n' => literal.push_str("{ENTER}"),
            '\t' => literal.push_str("{TAB}"),
            '{' | '}' | '(' | ')' | '[' | ']' | '+' | '^' | '%' | '~' => {
                literal.push('{');
                literal.push(character);
                literal.push('}');
            }
            '\'' => literal.push_str("{'}"),
            other => literal.push(other),
        }
    }
    literal.push('\'');
    literal
}

/// The named keys one `key` step accepts, mapped to SendKeys names.
#[cfg(any(windows, test))]
fn key_name(name: &str) -> anyhow::Result<&'static str> {
    let upper = name.trim().to_ascii_uppercase();
    let name = match upper.as_str() {
        "ENTER" | "RETURN" => "ENTER",
        "TAB" => "TAB",
        "ESC" | "ESCAPE" => "ESC",
        "SPACE" => " ",
        "BACKSPACE" => "BACKSPACE",
        "DELETE" | "DEL" => "DELETE",
        "INSERT" => "INSERT",
        "HOME" => "HOME",
        "END" => "END",
        "PAGEUP" => "PGUP",
        "PAGEDOWN" => "PGDN",
        "UP" => "UP",
        "DOWN" => "DOWN",
        "LEFT" => "LEFT",
        "RIGHT" => "RIGHT",
        "F1" => "F1",
        "F2" => "F2",
        "F3" => "F3",
        "F4" => "F4",
        "F5" => "F5",
        "F6" => "F6",
        "F7" => "F7",
        "F8" => "F8",
        "F9" => "F9",
        "F10" => "F10",
        "F11" => "F11",
        "F12" => "F12",
        other => anyhow::bail!(
            "unknown key `{other}`; use ENTER, TAB, ESC, SPACE, BACKSPACE, DELETE, INSERT, \
             HOME, END, PAGEUP, PAGEDOWN, UP, DOWN, LEFT, RIGHT or F1..F12"
        ),
    };
    Ok(name)
}

#[cfg(any(windows, test))]
fn keys_literal(keys: &[String]) -> anyhow::Result<String> {
    let keys: Vec<&str> = keys
        .iter()
        .map(|key| key_name(key))
        .collect::<anyhow::Result<_>>()?;
    if keys.is_empty() {
        anyhow::bail!("a key action needs keys or a chord");
    }
    // Several keys press together as one chord; a single key is plain.
    let mut literal = String::new();
    for key in &keys {
        wrap_modified(key, &mut literal);
    }
    Ok(literal)
}

/// A chord such as `ctrl+s`, `alt+tab` or `ctrl+shift+t`, in SendKeys syntax:
/// `^` ctrl, `%` alt, `+` shift, and the final key wrapped in braces.
#[cfg(any(windows, test))]
fn chord_literal(chord: &str) -> anyhow::Result<String> {
    let parts: Vec<&str> = chord
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        anyhow::bail!("a key action needs keys or a chord");
    }
    // `ctrl++` and `win+` split awkwardly: a trailing plus means the `+` key.
    let (modifiers, final_key) = parts.split_at(parts.len().saturating_sub(1));
    let mut literal = String::new();
    for modifier in modifiers {
        match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => literal.push('^'),
            "alt" => literal.push('%'),
            "shift" => literal.push('+'),
            other => {
                anyhow::bail!("unknown modifier `{other}` in `{chord}`; use ctrl, alt or shift")
            }
        }
    }
    // The last token is one character, or a named key such as `tab`.
    let key = final_key[0];
    let key = if key.chars().count() == 1 {
        key.to_owned()
    } else {
        key_name(key)?.to_owned()
    };
    wrap_modified(&key, &mut literal);
    Ok(literal)
}

#[cfg(any(windows, test))]
fn wrap_modified(key: &str, literal: &mut String) {
    if key.chars().count() == 1 {
        let character = key.chars().next().unwrap_or(' ');
        literal.push('{');
        literal.push(character);
        literal.push('}');
    } else {
        literal.push('{');
        literal.push_str(key);
        literal.push('}');
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_SETTLE_MS, MAX_STEPS, MAX_WAIT_MS, describe, key_name, screenshot_path,
        send_keys_literal, windows_script,
    };

    #[test]
    fn the_result_points_at_the_after_screenshot() {
        let content = "Action: open the menu\nActions run: press ctrl+s\nAfter: C:\\Temp\\b.png";
        assert_eq!(screenshot_path(content), Some("C:\\Temp\\b.png".to_owned()));
        assert_eq!(screenshot_path("Error: nothing"), None);
        assert_eq!(
            screenshot_path("After: /tmp/a.png\n").as_deref(),
            Some("/tmp/a.png")
        );
    }

    #[test]
    fn an_unsupported_platform_says_so_instead_of_pretending() {
        if cfg!(windows) {
            return;
        }
        assert!(super::run_actions(&[]).is_err());
        assert!(
            super::computer_use(r#"{"action":"look"}"#)
                .unwrap_err()
                .to_string()
                .contains("only implemented on Windows")
        );
    }

    #[test]
    fn an_empty_or_unknown_call_refuses_readably() {
        let error = super::computer_use(r#"{"action":"x","actions":[{"type":"fly"}]}"#)
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid tool arguments"), "{error}");
        if cfg!(windows) {
            // On Windows a valid call would capture the screen, so the
            // remaining argument check is gated.
            let error = super::computer_use("{}").unwrap_err().to_string();
            assert!(error.contains("needs an action"), "{error}");
        }
    }

    #[test]
    fn special_characters_are_wrapped_for_send_keys() {
        assert_eq!(send_keys_literal("a+b{c}"), "'a{+}b{{}c{}}'");
        assert_eq!(send_keys_literal("line\nnext"), "'line{ENTER}next'");
        assert_eq!(send_keys_literal("it's"), "'it{'}s'");
    }

    #[test]
    fn keys_map_to_send_keys_names_and_unknown_ones_refuse() {
        assert_eq!(key_name("enter").unwrap(), "ENTER");
        assert_eq!(key_name("PageDown").unwrap(), "PGDN");
        assert!(key_name("hyperdrive").is_err());
    }

    #[test]
    fn chords_compile_to_send_keys_syntax() {
        assert_eq!(super::chord_literal("ctrl+s").unwrap(), "^{s}");
        assert_eq!(super::chord_literal("ctrl+shift+t").unwrap(), "^+{t}");
        assert_eq!(super::chord_literal("alt+tab").unwrap(), "%{TAB}");
        assert!(super::chord_literal("hyper+s").is_err());
    }

    #[test]
    fn the_script_keeps_every_step_and_ends_cleanly() {
        let steps = vec![
            super::Step::Move { x: 5, y: 6 },
            super::Step::Type {
                text: "it's".to_owned(),
            },
        ];
        let script = windows_script(&steps).unwrap();
        assert!(script.contains("SetCursorPos(5, 6)"), "{script}");
        assert!(script.contains("SendWait('it{'}s')"), "{script}");
        assert!(script.ends_with("exit 0;\n"), "{script}");
    }

    #[test]
    fn every_step_kind_parses_and_describes() {
        let all = r#"{"action":"mix","actions":[
            {"type":"move","x":1,"y":2},
            {"type":"click","x":3,"y":4,"button":"right","count":2},
            {"type":"double_click","x":5,"y":6},
            {"type":"scroll","x":7,"y":8,"amount":-3},
            {"type":"drag","from_x":1,"from_y":2,"to_x":3,"to_y":4},
            {"type":"type","text":"hello"},
            {"type":"key","chord":"ctrl+s"},
            {"type":"key","keys":["ENTER"]},
            {"type":"wait","ms":250},
            {"type":"focus_window","title":"Notepad"}
        ]}"#;
        let parsed: super::ComputerUseArgs = serde_json::from_str(all).unwrap();
        assert_eq!(parsed.actions.len(), 10);
        assert_eq!(describe(&parsed.actions[0]), "move to (1, 2)");
        assert_eq!(describe(&parsed.actions[1]), "click 2x Right at (3, 4)");
        assert_eq!(describe(&parsed.actions[2]), "double-click at (5, 6)");
        assert_eq!(describe(&parsed.actions[3]), "scroll -3 at (7, 8)");
        assert_eq!(describe(&parsed.actions[4]), "drag (1, 2) to (3, 4)");
        assert_eq!(describe(&parsed.actions[5]), "type 5 characters");
        assert_eq!(describe(&parsed.actions[6]), "press ctrl+s");
        assert_eq!(describe(&parsed.actions[7]), "press ENTER");
        assert_eq!(describe(&parsed.actions[8]), "wait 250 ms");
        assert_eq!(describe(&parsed.actions[9]), "focus window `Notepad`");
        assert_eq!(MAX_STEPS, 32);
    }

    #[test]
    fn a_wait_step_defaults_and_quotes_into_the_script() {
        let parsed: super::ComputerUseArgs = serde_json::from_str(
            r#"{"action":"x","actions":[{"type":"wait"},{"type":"focus_window","title":"a'b"}]}"#,
        )
        .unwrap();
        assert_eq!(describe(&parsed.actions[0]), "wait 500 ms");
        let script = windows_script(&parsed.actions).unwrap();
        assert!(
            script.contains("Start-Sleep -Milliseconds 500;"),
            "{script}"
        );
        assert!(script.contains("OnTitle('a''b');"), "{script}");
    }

    #[test]
    fn a_wait_longer_than_the_cap_refuses() {
        let error = super::computer_use(r#"{"action":"x","actions":[{"type":"wait","ms":60000}]}"#)
            .unwrap_err()
            .to_string();
        assert!(error.contains("at most 10000 ms"), "{error}");
        assert_eq!(MAX_WAIT_MS, 10_000);
        assert!(DEFAULT_SETTLE_MS <= MAX_WAIT_MS);
    }

    #[test]
    fn a_region_must_have_an_area() {
        let error =
            super::computer_use(r#"{"action":"x","region":{"x":0,"y":0,"width":0,"height":10}}"#)
                .unwrap_err()
                .to_string();
        assert!(error.contains("width and a height"), "{error}");
    }

    #[test]
    fn the_geometry_line_is_read_back_exactly() {
        let geometry =
            super::parse_geometry("noise\ngeometry -1920 0 3840 1080 100 200 800 600 640,480\n")
                .unwrap();
        assert_eq!(geometry.desktop, (-1920, 0, 3840, 1080));
        assert_eq!(geometry.origin, (100, 200));
        assert_eq!(geometry.size, (800, 600));
        assert_eq!(geometry.cursor, Some((640, 480)));
        // A cursor the script could not read stays unknown instead of (0, 0).
        let no_cursor = super::parse_geometry("geometry 0 0 1920 1080 0 0 1920 1080 -").unwrap();
        assert_eq!(no_cursor.cursor, None);
        // A short or absent line is refused, never guessed at.
        assert!(super::parse_geometry("geometry 0 0 1920 1080").is_none());
        assert!(super::parse_geometry("nothing here").is_none());
        assert!(super::parse_geometry("geometry 0 0 1920 1080 0 0 800 600 foo,bar").is_none());
    }

    #[test]
    fn the_report_names_the_frame_the_desktop_and_the_cursor() {
        let shot = super::Shot {
            path: std::path::PathBuf::from("C:\\Temp\\a.png"),
            origin: (100, 200),
            size: (800, 600),
            desktop: (-1920, 0, 3840, 1080),
            cursor: Some((640, 480)),
        };
        let report = format!("{}{}", shot.describe(), shot.cursor_line());
        assert!(report.contains("Frame: (100, 200) 800x600"), "{report}");
        assert!(report.contains("Desktop: (-1920, 0) 3840x1080"), "{report}");
        assert!(report.contains("Cursor: (640, 480)"), "{report}");
        // A click target survives the report: the path stays the last line.
        assert_eq!(
            screenshot_path(&format!("{report}After: C:\\Temp\\a.png")),
            Some("C:\\Temp\\a.png".to_owned())
        );
    }
}
