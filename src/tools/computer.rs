//! `computer_use`: drive the desktop and show the model what happened.
//!
//! A call takes one screenshot, then runs its `actions` in order: pointer
//! moves, clicks, scrolls, drags, typed text, key presses, key chords, waits
//! and window focus changes. A second screenshot after the actions is written
//! to the temp root, where the app attaches it to the tool message so a vision
//! model sees the result. The result also names where the captured frame sits
//! on the desktop and where the pointer ended up, so the next call can aim.
//!
//! The agent has a pointer of its own. Hovering never moves the system
//! cursor: it posts a move message to the window under the point. Clicks,
//! drags and the wheel are injected as real input, because Windows Terminal,
//! Chromium and most GPU windows ignore posted button messages; the user's
//! pointer is put back where it was immediately afterwards. The agent's
//! pointer is a visible click-through desktop overlay and is also drawn into
//! the capture, so the person at the keyboard keeps their mouse position.
//!
//! Windows only. The Windows implementation drives the desktop through
//! `System.Windows.Forms` (typing), `System.Drawing` (screen capture) and WPF
//! (the overlay) through PowerShell, so the binary links no platform libraries.
//! Everywhere else the tool answers with a plain error instead of pretending it
//! worked.

use anyhow::Context;
use serde::Deserialize;

use super::{parse, truncate_output};

#[cfg(any(windows, test))]
mod cursor;
#[cfg(any(windows, test))]
mod window;

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
    /// Drive one window in the background instead of the whole desktop: input
    /// is posted to that window's handle, so the system pointer never moves
    /// and no window is raised, and the capture comes from the window itself.
    #[serde(default)]
    window: Option<String>,
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
    let window = args
        .window
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty());
    if args.window.is_some() && window.is_none() {
        anyhow::bail!("window needs a title to look for, or leave it out to drive the desktop");
    }
    if window.is_some() && args.screen != 0 {
        anyhow::bail!(
            "screen picks a display for a desktop capture; a window target names the window itself"
        );
    }
    if args.actions.is_empty() && intent.is_empty() {
        anyhow::bail!("computer_use needs an action: describe it, or list the actions to run");
    }
    let settle = args.settle_ms.unwrap_or(DEFAULT_SETTLE_MS).min(MAX_WAIT_MS);

    // One window, driven in the background: posted input, no focus change, and
    // a capture of the window itself, so the person at the keyboard keeps both
    // their pointer and a view of what the agent is doing.
    if let Some(title) = window {
        return background(title, &intent, &args, settle);
    }

    // The "before" capture proves the desktop answered; the model receives
    // only the state after the actions, one screenshot per call.
    let before = screenshot(args.screen, args.region)?;
    // Show the real desktop pointer before the first action. Startup is a
    // handshake: a failed overlay must not leave invisible clicks behind.
    #[cfg(windows)]
    cursor::ensure(remembered_pointer().or(before.cursor))?;
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
    // The agent's pointer is where this call left it, or where the user's
    // cursor sits when the agent has not aimed yet, and it is drawn into the
    // capture so the model sees its own pointer rather than a system cursor
    // that never moved.
    let pointer = pointer_target(&args.actions)
        .or_else(remembered_pointer)
        .or(before.cursor);
    let marked = pointer.is_some_and(|at| {
        remember_pointer(at);
        draw_agent_cursor(&after.path, after.origin, after.size, at).unwrap_or(false)
    });

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
    report.push_str(&cursor_line(pointer, marked));
    report.push_str("After: ");
    report.push_str(&after.path.display().to_string());
    Ok(truncate_output(report))
}

/// The background branch: one window, driven without the foreground.
#[cfg(windows)]
fn background(
    title: &str,
    intent: &str,
    args: &ComputerUseArgs,
    settle: u64,
) -> anyhow::Result<String> {
    window::run(intent, title, &args.actions, args.region, settle)
}

#[cfg(not(windows))]
fn background(
    _title: &str,
    _intent: &str,
    _args: &ComputerUseArgs,
    _settle: u64,
) -> anyhow::Result<String> {
    anyhow::bail!("computer_use is only implemented on Windows")
}

/// The line naming the agent's pointer, and that it is the agent's alone.
fn cursor_line(pointer: Option<(i32, i32)>, marked: bool) -> String {
    let Some((x, y)) = pointer else {
        return String::new();
    };
    let drawn = if marked {
        ", visible on the desktop and drawn in the capture as the agent cursor"
    } else {
        ""
    };
    format!(
        "Cursor: ({x}, {y}){drawn}; clicks inject real input and put the system pointer back where it was\n"
    )
}

/// Where the call leaves the agent's pointer, if any step aims it.
fn pointer_target(steps: &[Step]) -> Option<(i32, i32)> {
    steps.iter().rev().find_map(|step| match step {
        Step::Move { x, y }
        | Step::Click { x, y, .. }
        | Step::DoubleClick { x, y }
        | Step::Scroll { x, y, .. } => Some((*x, *y)),
        Step::Drag { to_x, to_y, .. } => Some((*to_x, *to_y)),
        _ => None,
    })
}

/// The file the agent's pointer survives between calls in: a call that only
/// looks still shows where the agent last aimed.
fn pointer_path() -> std::path::PathBuf {
    crate::temp::temp_root().join("yopes-agent-pointer")
}

fn remembered_pointer() -> Option<(i32, i32)> {
    #[cfg(windows)]
    if let Some(at) = read_pointer(&cursor::state_path()) {
        return Some(at);
    }
    read_pointer(&pointer_path())
}

fn read_pointer(path: &std::path::Path) -> Option<(i32, i32)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut parts = text.split_whitespace();
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn remember_pointer(at: (i32, i32)) {
    let text = format!("{} {}", at.0, at.1);
    let _ = std::fs::write(pointer_path(), &text);
    #[cfg(windows)]
    let _ = std::fs::write(cursor::state_path(), text);
}

/// Draw the agent's own cursor into a capture, at `pointer` in screen pixels:
/// a highlighted black arrow on a soft light halo, where the hotspot sits on
/// the point the call acted on. `false` when the pointer is outside the frame
/// or the icon cannot be drawn, which leaves the capture untouched.
fn draw_agent_cursor(
    path: &std::path::Path,
    origin: (i32, i32),
    size: (u32, u32),
    pointer: (i32, i32),
) -> anyhow::Result<bool> {
    let (left, top) = (pointer.0 - origin.0, pointer.1 - origin.1);
    let (width, height) = size;
    if left < 0 || top < 0 || left as u32 >= width || top as u32 >= height {
        return Ok(false);
    }
    let capture = image::open(path)?.to_rgba8();
    if capture.width() != width || capture.height() != height {
        return Ok(false);
    }
    let Some(mut pixmap) = tiny_skia::Pixmap::from_vec(
        premultiplied(&capture),
        tiny_skia::IntSize::from_wh(width, height).context("sizing the cursor bitmap")?,
    ) else {
        return Ok(false);
    };
    let Some(arrow) = cursor_shape() else {
        return Ok(false);
    };
    // The halo marks the pointer as the agent's, and the white outline keeps
    // the black arrow readable over a dark app.
    let mut halo_paint = tiny_skia::Paint {
        anti_alias: true,
        ..Default::default()
    };
    halo_paint.set_color_rgba8(255, 255, 255, 96);
    let halo = tiny_skia::PathBuilder::from_circle(1.0, 1.0, 11.0).context("building the halo")?;
    let mut fill = tiny_skia::Paint {
        anti_alias: true,
        ..Default::default()
    };
    fill.set_color_rgba8(12, 12, 12, 255);
    let mut outline = tiny_skia::Paint {
        anti_alias: true,
        ..Default::default()
    };
    outline.set_color_rgba8(255, 255, 255, 235);
    let stroke = tiny_skia::Stroke {
        width: 1.5,
        ..tiny_skia::Stroke::default()
    };
    let transform = tiny_skia::Transform::from_translate(left as f32, top as f32);
    let rule = tiny_skia::FillRule::Winding;
    pixmap.fill_path(&halo, &halo_paint, rule, transform, None);
    pixmap.fill_path(&arrow, &fill, rule, transform, None);
    pixmap.stroke_path(&arrow, &outline, &stroke, transform, None);
    image::RgbaImage::from_raw(width, height, straight_alpha(&pixmap))
        .context("rebuilding the capture")?
        .save_with_format(path, image::ImageFormat::Png)?;
    Ok(true)
}

/// The arrow itself, in icon pixels with the hotspot at (1, 1).
fn cursor_shape() -> Option<tiny_skia::Path> {
    let mut builder = tiny_skia::PathBuilder::new();
    builder.move_to(1.0, 1.0);
    builder.line_to(1.0, 17.5);
    builder.line_to(5.2, 13.4);
    builder.line_to(8.0, 19.6);
    builder.line_to(10.7, 18.4);
    builder.line_to(7.9, 12.4);
    builder.line_to(13.2, 12.4);
    builder.close();
    builder.finish()
}

/// tiny-skia paints premultiplied, and a PNG carries straight alpha.
fn premultiplied(image: &image::RgbaImage) -> Vec<u8> {
    let mut data = Vec::with_capacity(image.as_raw().len());
    for pixel in image.pixels() {
        let [red, green, blue, alpha] = pixel.0;
        let alpha = u16::from(alpha);
        for channel in [red, green, blue] {
            data.push(u8::try_from(u16::from(channel) * alpha / 255).unwrap_or(0));
        }
        data.push(u8::try_from(alpha).unwrap_or(0));
    }
    data
}

fn straight_alpha(pixmap: &tiny_skia::Pixmap) -> Vec<u8> {
    let mut data = Vec::with_capacity(pixmap.data().len());
    for pixel in pixmap.pixels() {
        let color = pixel.demultiply();
        data.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
    }
    data
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
    let script = windows_script(steps, remembered_pointer())?;
    run_script(&script)?;
    Ok(steps.iter().map(describe).collect())
}

/// Run one generated PowerShell script in a hidden STA process and hand back
/// its stdout. A failed script becomes the call's error, with the script's own
/// message rather than a bare exit code.
#[cfg(any(windows, test))]
fn run_script(script: &str) -> anyhow::Result<String> {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-WindowStyle",
            "Hidden",
            "-Command",
            script,
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
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(all(not(windows), test))]
fn run_script(script: &str) -> anyhow::Result<String> {
    let _ = script;
    anyhow::bail!("computer_use is only implemented on Windows")
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
    /// Where the system pointer sits: only the agent pointer's starting point
    /// the first time it aims.
    cursor: Option<(i32, i32)>,
    /// The window this frame came from, in background mode; a foreground
    /// frame is the desktop, which is named as such.
    target: Option<String>,
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
        match &self.target {
            Some(title) => format!(
                "Frame: ({x}, {y}) {width}x{height} client pixels of the window `{title}`\n\
                 Window: client area at ({dx}, {dy}) {dw}x{dh} screen pixels\n"
            ),
            None => {
                format!(
                    "Frame: ({x}, {y}) {width}x{height} screen pixels\nDesktop: ({dx}, {dy}) {dw}x{dh}\n"
                )
            }
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
        target: None,
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

/// How long one frame of a pointer glide takes. Fast enough to read as a hand
/// moving, slow enough that the window under the pointer and the agent's
/// overlay both see the points in between instead of one jump.
#[cfg(any(windows, test))]
const GLIDE_STEP_MS: u64 = 14;

/// Points from just after `from` to `to`, along a line eased with a
/// smoothstep: slow off the mark, quick in the middle, gentle into the target,
/// ending exactly on `to`. `from` is where the agent's pointer already sits;
/// when that is unknown, or the target is already under it, the path is the
/// single target point, so a call that aims nowhere new still acts at once.
#[cfg(any(windows, test))]
fn glide(from: Option<(i32, i32)>, to: (i32, i32)) -> Vec<(i32, i32)> {
    let Some((from_x, from_y)) = from else {
        return vec![to];
    };
    let (dx, dy) = (
        i64::from(to.0) - i64::from(from_x),
        i64::from(to.1) - i64::from(from_y),
    );
    let distance = ((dx * dx + dy * dy) as f64).sqrt();
    if distance < 2.0 {
        return vec![to];
    }
    // A short hop takes a handful of frames and a long sweep more, so the
    // pointer keeps a roughly even speed instead of every move taking as long.
    let steps = ((distance / 28.0).ceil() as usize).clamp(6, 22);
    let mut path: Vec<(i32, i32)> = Vec::with_capacity(steps);
    for step in 1..=steps {
        let progress = step as f64 / steps as f64;
        let eased = progress * progress * (3.0 - 2.0 * progress);
        let x = from_x + (dx as f64 * eased).round() as i32;
        let y = from_y + (dy as f64 * eased).round() as i32;
        // A slow start rounds neighbouring frames onto the same pixel.
        if path.last() != Some(&(x, y)) {
            path.push((x, y));
        }
    }
    match path.last_mut() {
        Some(last) => *last = to,
        None => path.push(to),
    }
    path
}

/// The hover posts that walk the agent's pointer from `from` to `to`. Every
/// point but the last may miss a window — a sweep across a gap on the desktop
/// still lands where it was aimed — but the point the step acts on must exist,
/// so a call that aims at nothing fails instead of clicking into empty space.
#[cfg(any(windows, test))]
fn hover_script(from: Option<(i32, i32)>, to: (i32, i32)) -> String {
    let path = glide(from, to);
    let last = path.len().saturating_sub(1);
    let mut script = String::new();
    for (index, (x, y)) in path.iter().enumerate() {
        if index == last {
            script.push_str(&format!(
                "if (-not [YopesInput]::Hover({x}, {y})) {{ [Console]::Error.Write('nothing to point at ({x}, {y})'); exit 5 }};\n"
            ));
        } else {
            script.push_str(&format!(
                "[void][YopesInput]::Hover({x}, {y});\nStart-Sleep -Milliseconds {GLIDE_STEP_MS};\n"
            ));
        }
    }
    script
}

/// The script that performs every step in order, starting from `start` where
/// the agent's pointer already sits so the first aim glides instead of jumping.
/// Coordinates come from the model as plain integers, and text only crosses as
/// a quoted literal.
#[cfg(any(windows, test))]
fn windows_script(steps: &[Step], start: Option<(i32, i32)>) -> anyhow::Result<String> {
    let mut script = String::from(
        "Add-Type -AssemblyName System.Windows.Forms;\n\
         Add-Type -AssemblyName System.Drawing;\n\
         Add-Type @'\n\
         using System;\n\
         using System.IO;\n\
         using System.Runtime.InteropServices;\n\
         public static class YopesInput {\n\
           [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }\n\
           [DllImport(\"user32.dll\")] static extern IntPtr WindowFromPoint(POINT point);\n\
           [DllImport(\"user32.dll\")] static extern IntPtr GetAncestor(IntPtr window, uint flags);\n\
           [DllImport(\"user32.dll\")] static extern bool IsWindowVisible(IntPtr window);\n\
           [DllImport(\"user32.dll\")] static extern bool ScreenToClient(IntPtr window, ref POINT point);\n\
           [DllImport(\"user32.dll\")] static extern bool PostMessage(IntPtr window, uint message, IntPtr wparam, IntPtr lparam);\n\
           [DllImport(\"user32.dll\")] static extern bool SetForegroundWindow(IntPtr window);\n\
           public const uint Move = 0x0200;\n\
           public const uint LeftDown = 0x0201;\n\
           public const uint LeftUp = 0x0202;\n\
           public const uint RightDown = 0x0204;\n\
           public const uint RightUp = 0x0205;\n\
           public const uint MiddleDown = 0x0207;\n\
           public const uint MiddleUp = 0x0208;\n\
           public const uint Wheel = 0x020A;\n\
           [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT { public int dx; public int dy; public uint mouseData; public uint flags; public uint time; public IntPtr extra; }\n\
           [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public MOUSEINPUT mi; }\n\
           [DllImport(\"user32.dll\")] static extern uint SendInput(uint count, INPUT[] inputs, int size);\n\
           [DllImport(\"user32.dll\")] static extern bool SetCursorPos(int x, int y);\n\
           [DllImport(\"user32.dll\")] static extern bool GetCursorPos(out POINT point);\n\
           const uint RawMove = 0x0001;\n\
           const uint RawLeftDown = 0x0002;\n\
           const uint RawLeftUp = 0x0004;\n\
           const uint RawRightDown = 0x0008;\n\
           const uint RawRightUp = 0x0010;\n\
           const uint RawMiddleDown = 0x0020;\n\
           const uint RawMiddleUp = 0x0040;\n\
           const uint RawWheel = 0x0800;\n\
           static void Raw(uint flags, int data) {\n\
             INPUT input = new INPUT();\n\
             input.type = 0;\n\
             input.mi.flags = flags;\n\
             input.mi.mouseData = (uint)data;\n\
             SendInput(1, new INPUT[] { input }, Marshal.SizeOf(typeof(INPUT)));\n\
           }\n\
           static uint RawDown(uint down) {\n\
             if (down == RightDown) return RawRightDown;\n\
             if (down == MiddleDown) return RawMiddleDown;\n\
             return RawLeftDown;\n\
           }\n\
           static uint RawUp(uint up) {\n\
             if (up == RightUp) return RawRightUp;\n\
             if (up == MiddleUp) return RawMiddleUp;\n\
             return RawLeftUp;\n\
           }\n\
           // Posted messages are ignored by Windows Terminal, Chromium and most\n\
           // GPU windows, so the click is also injected as real input. The\n\
           // pointer is put back where the user left it straight afterwards.\n\
           // The user's pointer goes back where it was; the agent's own\n\
           // pointer stays where it aimed.\n\
           static void Restore(POINT saved) { SetCursorPos(saved.X, saved.Y); }\n\
           // Walk the system pointer to the target instead of snapping it:\n\
           // the same eased path the agent's arrow takes, so a click on the\n\
           // far side of the screen reads as a hand moving, not a teleport.\n\
           // A short hop is left to the caller's SetCursorPos.\n\
           static void Glide(int toX, int toY) {\n\
             POINT from; GetCursorPos(out from);\n\
             int dx = toX - from.X, dy = toY - from.Y;\n\
             double distance = Math.Sqrt((double)dx * dx + (double)dy * dy);\n\
             if (distance < 120) { SetCursorPos(toX, toY); return; }\n\
             int steps = (int)Math.Ceiling(distance / 40.0);\n\
             if (steps < 4) steps = 4;\n\
             if (steps > 14) steps = 14;\n\
             for (int i = 1; i < steps; i++) {\n\
               double p = (double)i / steps;\n\
               double e = p * p * (3 - 2 * p);\n\
               SetCursorPos(from.X + (int)Math.Round(dx * e), from.Y + (int)Math.Round(dy * e));\n\
               System.Threading.Thread.Sleep(12);\n\
             }\n\
             SetCursorPos(toX, toY);\n\
           }\n\
           public static void RealClick(int x, int y, uint down, uint up, int count) {\n\
             POINT saved; GetCursorPos(out saved);\n\
             Glide(x, y);\n\
             System.Threading.Thread.Sleep(40);\n\
             for (int i = 0; i < count; i++) {\n\
               Raw(RawDown(down), 0);\n\
               System.Threading.Thread.Sleep(30);\n\
               Raw(RawUp(up), 0);\n\
               System.Threading.Thread.Sleep(90);\n\
             }\n\
             Restore(saved);\n\
           }\n\
           public static void RealWheel(int x, int y, int delta) {\n\
             POINT saved; GetCursorPos(out saved);\n\
             Glide(x, y);\n\
             System.Threading.Thread.Sleep(40);\n\
             Raw(RawWheel, delta);\n\
             System.Threading.Thread.Sleep(120);\n\
             Restore(saved);\n\
           }\n\
           // A drag is a press, a glide and a release; the caller walks the\n\
           // path, so the window under the pointer sees every point.\n\
           static POINT savedDrag;\n\
           public static bool DragBegin(int x, int y, uint down) {\n\
             IntPtr window = At(x, y);\n\
             if (window == IntPtr.Zero) return false;\n\
             Mark(x, y);\n\
             SetForegroundWindow(GetAncestor(window, 2));\n\
             GetCursorPos(out savedDrag);\n\
             Glide(x, y);\n\
             System.Threading.Thread.Sleep(60);\n\
             Raw(RawDown(down), 0);\n\
             System.Threading.Thread.Sleep(80);\n\
             return true;\n\
           }\n\
           public static void DragStep(int x, int y) {\n\
             SetCursorPos(x, y);\n\
             Mark(x, y);\n\
           }\n\
           public static void DragEnd(uint up) {\n\
             Raw(RawUp(up), 0);\n\
             System.Threading.Thread.Sleep(120);\n\
             Restore(savedDrag);\n\
           }\n\
           public static string CursorPath = \"\";\n\
           // The visible overlay polls this file; never move the system cursor.\n\
           static void Mark(int x, int y) {\n\
             if (CursorPath.Length == 0) return;\n\
             try { File.WriteAllText(CursorPath, x + \" \" + y); } catch { }\n\
           }\n\
           // The deepest visible window under a screen point, zero when none.\n\
           static IntPtr At(int x, int y) {\n\
             POINT point; point.X = x; point.Y = y;\n\
             IntPtr window = WindowFromPoint(point);\n\
             if (window == IntPtr.Zero) return IntPtr.Zero;\n\
             IntPtr root = GetAncestor(window, 2);\n\
             if (root == IntPtr.Zero || !IsWindowVisible(root)) return IntPtr.Zero;\n\
             return window;\n\
           }\n\
           // The point in that window's client coordinates, packed for lParam.\n\
           static IntPtr Packed(IntPtr window, int x, int y) {\n\
             POINT point; point.X = x; point.Y = y;\n\
             ScreenToClient(window, ref point);\n\
             return (IntPtr)((point.Y << 16) | (point.X & 0xFFFF));\n\
           }\n\
           // The MK_ flag that belongs in wParam while a button is held.\n\
           static IntPtr Held(uint down) {\n\
             if (down == RightDown) return (IntPtr)2;\n\
             if (down == MiddleDown) return (IntPtr)16;\n\
             return (IntPtr)1;\n\
           }\n\
           public static bool Hover(int x, int y) {\n\
             IntPtr window = At(x, y);\n\
             if (window == IntPtr.Zero) return false;\n\
             Mark(x, y);\n\
             return PostMessage(window, Move, IntPtr.Zero, Packed(window, x, y));\n\
           }\n\
           public static bool Click(int x, int y, uint down, uint up, int count) {\n\
             IntPtr window = At(x, y);\n\
             if (window == IntPtr.Zero) return false;\n\
             Mark(x, y);\n\
             SetForegroundWindow(GetAncestor(window, 2));\n\
             System.Threading.Thread.Sleep(60);\n\
             RealClick(x, y, down, up, count);\n\
             return true;\n\
           }\n\
           public static bool Scroll(int x, int y, int delta) {\n\
             IntPtr window = At(x, y);\n\
             if (window == IntPtr.Zero) return false;\n\
             Mark(x, y);\n\
             RealWheel(x, y, delta);\n\
             return true;\n\
           }\n\
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
    script.push_str(&format!(
        "[YopesInput]::CursorPath = {};\n",
        ps_quote(&cursor::state_path().display().to_string())
    ));
    // Where the agent's pointer already sits, so the first aim glides from it.
    let mut last = start;
    for step in steps {
        match step {
            Step::Move { x, y } => {
                script.push_str(&hover_script(last, (*x, *y)));
                script.push_str("Start-Sleep -Milliseconds 60;\n");
                last = Some((*x, *y));
            }
            Step::Click {
                x,
                y,
                button,
                count,
            } => {
                let (down, up) = click_messages(*button);
                let count = (*count).clamp(1, 3);
                script.push_str(&hover_script(last, (*x, *y)));
                script.push_str(&format!(
                    "if (-not [YopesInput]::Click({x}, {y}, [YopesInput]::{down}, [YopesInput]::{up}, {count})) {{ [Console]::Error.Write('nothing to click at ({x}, {y})'); exit 5 }};\nStart-Sleep -Milliseconds 60;\n"
                ));
                last = Some((*x, *y));
            }
            Step::DoubleClick { x, y } => {
                script.push_str(&hover_script(last, (*x, *y)));
                script.push_str(&format!(
                    "if (-not [YopesInput]::Click({x}, {y}, [YopesInput]::LeftDown, [YopesInput]::LeftUp, 2)) {{ [Console]::Error.Write('nothing to click at ({x}, {y})'); exit 5 }};\nStart-Sleep -Milliseconds 60;\n"
                ));
                last = Some((*x, *y));
            }
            Step::Scroll { x, y, amount } => {
                let delta = (*amount).clamp(-10, 10) * 120;
                script.push_str(&hover_script(last, (*x, *y)));
                script.push_str(&format!(
                    "if (-not [YopesInput]::Scroll({x}, {y}, {delta})) {{ [Console]::Error.Write('nothing to scroll at ({x}, {y})'); exit 5 }};\nStart-Sleep -Milliseconds 120;\n"
                ));
                last = Some((*x, *y));
            }
            Step::Drag {
                from_x,
                from_y,
                to_x,
                to_y,
                button,
            } => {
                let (down, up) = click_messages(*button);
                script.push_str(&hover_script(last, (*from_x, *from_y)));
                script.push_str(&format!(
                    "if (-not [YopesInput]::DragBegin({from_x}, {from_y}, [YopesInput]::{down})) {{ [Console]::Error.Write('nothing to drag at ({from_x}, {from_y})'); exit 5 }};\n"
                ));
                // The press, the glide and the release are three calls so the
                // window under the pointer sees every point of the path.
                for (x, y) in glide(Some((*from_x, *from_y)), (*to_x, *to_y)) {
                    script.push_str(&format!(
                        "[YopesInput]::DragStep({x}, {y});\nStart-Sleep -Milliseconds {GLIDE_STEP_MS};\n"
                    ));
                }
                script.push_str(&format!(
                    "[YopesInput]::DragEnd([YopesInput]::{up});\nStart-Sleep -Milliseconds 120;\n"
                ));
                last = Some((*to_x, *to_y));
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
fn click_messages(button: Button) -> (&'static str, &'static str) {
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
        DEFAULT_SETTLE_MS, GLIDE_STEP_MS, MAX_STEPS, MAX_WAIT_MS, cursor_line, describe, glide,
        hover_script, key_name, pointer_target, screenshot_path, send_keys_literal, windows_script,
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

    /// The C# block is only checked for text above; this runs it through
    /// PowerShell so a broken injection fails here instead of mid-task. No
    /// steps means no clicks: the script only defines its types.
    #[cfg(windows)]
    #[test]
    fn the_generated_script_compiles_in_powershell() {
        let script = super::windows_script(&[], None).expect("script");
        assert!(!script.contains("[YopesInput]::Click("), "{script}");
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write as _;
                child
                    .stdin
                    .as_mut()
                    .expect("stdin")
                    .write_all(script.as_bytes())?;
                child.wait_with_output()
            });
        match output {
            Ok(output) => assert!(
                output.status.success(),
                "powershell rejected the script: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
            // No PowerShell on this host: nothing to check.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("running powershell: {error}"),
        }
    }

    #[test]
    fn a_glide_walks_to_the_target_in_eased_steps() {
        // Without a known start there is nothing to glide from.
        assert_eq!(glide(None, (40, 60)), vec![(40, 60)]);
        // Already on the target: one point, acted on at once.
        assert_eq!(glide(Some((40, 60)), (40, 60)), vec![(40, 60)]);
        let path = glide(Some((0, 0)), (400, 200));
        assert!(path.len() >= 6, "{path:?}");
        assert_eq!(path.last(), Some(&(400, 200)), "{path:?}");
        // Every point sits on the straight line and never passes the target.
        let mut previous = 0.0;
        for (x, y) in &path {
            let off = (*y * 2 - *x).abs();
            assert!(off <= 1, "({x}, {y}) is off the line: {path:?}");
            let at = ((x * x + y * y) as f64).sqrt();
            assert!(at >= previous, "{path:?}");
            previous = at;
        }
        // Eased, not linear: the first hop is shorter than the middle one.
        let hops: Vec<i32> = path.windows(2).map(|pair| pair[1].0 - pair[0].0).collect();
        let first = *hops.first().unwrap();
        let biggest = *hops.iter().max().unwrap();
        assert!(first < biggest, "{hops:?}");
        assert_eq!(GLIDE_STEP_MS, 14);
    }

    #[test]
    fn a_hover_path_posts_every_point_and_fails_only_at_the_target() {
        let script = hover_script(Some((0, 0)), (60, 0));
        let posted = script.matches("[void][YopesInput]::Hover(").count();
        assert!(posted >= 5, "{script}");
        // Only the point the step acts on must find a window: a sweep across
        // a gap on the desktop must not fail the call.
        assert_eq!(script.matches("nothing to point at").count(), 1, "{script}");
        assert!(script.contains("[YopesInput]::Hover(60, 0)"), "{script}");
        assert!(script.contains("Start-Sleep -Milliseconds 14"), "{script}");
    }

    #[test]
    fn the_script_keeps_every_step_and_ends_cleanly() {
        let steps = vec![
            super::Step::Move { x: 5, y: 6 },
            super::Step::Type {
                text: "it's".to_owned(),
            },
        ];
        let script = windows_script(&steps, None).unwrap();
        // Hover is posted, so the pointer stays put; a click injects real
        // input and restores the pointer right after.
        assert!(
            script.contains("static void Restore(POINT saved)"),
            "{script}"
        );
        assert!(script.contains("GetCursorPos(out saved)"), "{script}");
        assert!(script.contains("SendInput(1"), "{script}");
        assert!(script.contains("[YopesInput]::Hover(5, 6)"), "{script}");
        assert!(script.contains("[YopesInput]::CursorPath = '"), "{script}");
        assert!(
            script.contains("static void Mark(int x, int y)"),
            "{script}"
        );
        assert!(script.contains("SendWait('it{'}s')"), "{script}");
        assert!(script.ends_with("exit 0;\n"), "{script}");
    }

    #[test]
    fn the_script_posts_clicks_buttons_and_scrolls_to_the_window() {
        let parsed: super::ComputerUseArgs = serde_json::from_str(
            r#"{"action":"x","actions":[
            {"type":"click","x":3,"y":4,"button":"right","count":2},
            {"type":"double_click","x":5,"y":6},
            {"type":"scroll","x":7,"y":8,"amount":-3},
            {"type":"drag","from_x":1,"from_y":2,"to_x":3,"to_y":4}
        ]}"#,
        )
        .unwrap();
        let script = windows_script(&parsed.actions, None).unwrap();
        assert!(
            script.contains("Click(3, 4, [YopesInput]::RightDown, [YopesInput]::RightUp, 2)"),
            "{script}"
        );
        assert!(
            script.contains("Click(5, 6, [YopesInput]::LeftDown, [YopesInput]::LeftUp, 2)"),
            "{script}"
        );
        assert!(script.contains("Scroll(7, 8, -360)"), "{script}");
        assert!(
            script.contains("DragBegin(1, 2, [YopesInput]::LeftDown)"),
            "{script}"
        );
        assert!(script.contains("DragEnd([YopesInput]::LeftUp)"), "{script}");
        // The drag itself glides: a press, the points between, a release.
        let drag_steps = script.matches("[YopesInput]::DragStep(").count();
        assert!(drag_steps >= 2, "{script}");
        // A point with no window under it fails the call instead of silently
        // acting nowhere.
        assert!(script.contains("nothing to click at (3, 4)"), "{script}");
    }

    #[test]
    fn the_agent_pointer_follows_the_last_mouse_step() {
        let steps = vec![
            super::Step::Move { x: 5, y: 6 },
            super::Step::Type {
                text: "x".to_owned(),
            },
            super::Step::Drag {
                from_x: 1,
                from_y: 2,
                to_x: 30,
                to_y: 40,
                button: super::Button::Left,
            },
            super::Step::Wait { ms: 10 },
        ];
        assert_eq!(pointer_target(&steps), Some((30, 40)));
        assert_eq!(pointer_target(&steps[1..2]), None);
    }

    #[test]
    fn the_agent_cursor_is_drawn_on_its_pointer() {
        let path = crate::temp::unique_path("yopes-cursor-selftest", "png");
        image::RgbaImage::from_pixel(60, 60, image::Rgba([240, 240, 240, 255]))
            .save(&path)
            .unwrap();
        // A pointer outside the captured frame has no pixel to sit on.
        assert!(!super::draw_agent_cursor(&path, (0, 0), (60, 60), (200, 10)).unwrap());
        // The frame origin is subtracted, so screen pixels land where they should.
        assert!(super::draw_agent_cursor(&path, (100, 100), (60, 60), (120, 120)).unwrap());
        let drawn = image::open(&path).unwrap().to_rgba8();
        // Sample the fill, not the anti-aliased white outline at (26, 26).
        let dark = drawn.get_pixel(24, 26).0;
        assert!(dark[..3].iter().all(|channel| *channel < 60), "{dark:?}");
        // Everything outside the icon is left exactly as captured.
        assert_eq!(drawn.get_pixel(55, 55).0, [240, 240, 240, 255]);
        let _ = std::fs::remove_file(&path);
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
        let script = windows_script(&parsed.actions, None).unwrap();
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
        const { assert!(DEFAULT_SETTLE_MS <= MAX_WAIT_MS) };
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
            target: None,
        };
        let report = format!("{}{}", shot.describe(), cursor_line(Some((640, 480)), true));
        assert!(report.contains("Frame: (100, 200) 800x600"), "{report}");
        assert!(report.contains("Desktop: (-1920, 0) 3840x1080"), "{report}");
        assert!(report.contains("Cursor: (640, 480)"), "{report}");
        // The line says whose pointer it is, and that the marker was drawn.
        assert!(report.contains("agent cursor"), "{report}");
        assert!(report.contains("system pointer back"), "{report}");
        // An unknown pointer says nothing rather than guessing (0, 0).
        assert_eq!(cursor_line(None, false), "");
        // A click target survives the report: the path stays the last line.
        assert_eq!(
            screenshot_path(&format!("{report}After: C:\\Temp\\a.png")),
            Some("C:\\Temp\\a.png".to_owned())
        );
    }
}
