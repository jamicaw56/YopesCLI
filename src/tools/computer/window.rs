//! Background mode: drive one window without taking the foreground.
//!
//! A normal call injects real input at screen coordinates, which moves the
//! system pointer and raises whatever window sits under it. With a `window`
//! argument the call instead posts its messages to that window's handle: the
//! system pointer never moves, no window is raised, and the capture comes from
//! the window itself (`PrintWindow`) rather than the screen, so the person at
//! the keyboard keeps their desktop and can watch the agent work in a window
//! off to the side.
//!
//! Coordinates are window **client** pixels — the same numbers the model picks
//! in the returned image, so nothing needs translating between calls.
//!
//! Posted input is best-effort by its nature: plain Win32 controls accept it,
//! while apps that draw themselves (Chromium, Electron, games, most GPU
//! toolkits) often ignore posted button or key messages, and a window that
//! cannot render itself is captured from the screen instead. The result says
//! which capture path was used so the model can tell the difference.

use std::path::{Path, PathBuf};

use super::{
    Button, GLIDE_STEP_MS, Region, Shot, Step, describe, draw_agent_cursor, glide, pointer_target,
    ps_quote,
};

/// One window this call drives: its handle and the client area it occupies in
/// screen pixels, so the agent's overlay arrow can sit over the real window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Target {
    pub handle: i64,
    pub title: String,
    /// Client area on the screen: x, y, width, height.
    pub client: (i32, i32, u32, u32),
}

/// The C# the PowerShell scripts share: finding a window, posting input to it,
/// drawing nothing, and the window's own capture with a screen fallback.
const HELPERS: &str = r#"
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

public static class YopesWindowMode {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct GUITHREADINFO {
    public int cbSize; public int flags; public IntPtr hwndActive; public IntPtr hwndFocus;
    public IntPtr hwndCapture; public IntPtr hwndMenuOwner; public IntPtr hwndMoveSize;
    public IntPtr hwndCaret; public RECT rcCaret;
  }

  delegate bool EnumProc(IntPtr window, IntPtr param);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc proc, IntPtr param);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr window, StringBuilder text, int max);
  [DllImport("user32.dll")] static extern int GetWindowTextLength(IntPtr window);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
  [DllImport("user32.dll")] static extern bool IsWindow(IntPtr window);
  [DllImport("user32.dll")] static extern bool IsWindowEnabled(IntPtr window);
  [DllImport("user32.dll")] static extern bool PostMessage(IntPtr window, uint message, IntPtr wparam, IntPtr lparam);
  [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr window, ref POINT point);
  [DllImport("user32.dll")] static extern bool ScreenToClient(IntPtr window, ref POINT point);
  [DllImport("user32.dll")] static extern IntPtr ChildWindowFromPointEx(IntPtr parent, POINT point, uint flags);
  [DllImport("user32.dll")] static extern int GetWindowThreadProcessId(IntPtr window, IntPtr pid);
  [DllImport("user32.dll")] static extern bool GetGUIThreadInfo(int thread, ref GUITHREADINFO info);
  [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr window, out RECT rect);
  [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr window, IntPtr hdc, uint flags);

  const uint MouseMove = 0x0200;
  const uint LeftDown = 0x0201, LeftUp = 0x0202;
  const uint RightDown = 0x0204, RightUp = 0x0205;
  const uint MiddleDown = 0x0207, MiddleUp = 0x0208;
  const uint Wheel = 0x020A;
  const uint KeyDown = 0x0100, KeyUp = 0x0101, Char = 0x0102;
  // CWP_SKIPINVISIBLE | CWP_SKIPDISABLED | CWP_SKIPTRANSPARENT.
  const uint Skip = 0x0003 | 0x0008;

  // The overlay polls this file, so the agent's arrow sits over the window it
  // is driving while the system pointer stays with the user.
  public static string CursorPath = "";

  static int Lo(int value) { return value & 0xFFFF; }
  static IntPtr Pad(int x, int y) { return (IntPtr)((Lo(y) << 16) | Lo(x)); }
  static IntPtr Up() { return new IntPtr((long)0xC0000001); }

  public static bool Alive(IntPtr window) { return window != IntPtr.Zero && IsWindow(window); }

  public static string Title(IntPtr window) {
    int length = GetWindowTextLength(window);
    if (length <= 0) return "";
    StringBuilder text = new StringBuilder(length + 1);
    GetWindowText(window, text, text.Capacity);
    return text.ToString();
  }

  // The first visible top-level window whose title contains the needle.
  static IntPtr found;
  public static IntPtr ByTitle(string needle) {
    found = IntPtr.Zero;
    string want = needle.ToLowerInvariant();
    EnumWindows(delegate(IntPtr window, IntPtr param) {
      if (!IsWindowVisible(window)) return true;
      string title = Title(window);
      if (title.Length == 0) return true;
      if (title.ToLowerInvariant().Contains(want)) { found = window; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }

  public static int[] Client(IntPtr window) {
    RECT rect; GetClientRect(window, out rect);
    POINT at; at.X = 0; at.Y = 0; ClientToScreen(window, ref at);
    return new int[] { at.X, at.Y, rect.Right - rect.Left, rect.Bottom - rect.Top };
  }

  public static void Aim(IntPtr window, int x, int y) {
    if (CursorPath.Length == 0) return;
    POINT at; at.X = x; at.Y = y;
    ClientToScreen(window, ref at);
    try { File.WriteAllText(CursorPath, at.X + " " + at.Y); } catch { }
  }

  // Keys land in whatever control the window's own thread has focused.
  static IntPtr Focused(IntPtr window) {
    int thread = GetWindowThreadProcessId(window, IntPtr.Zero);
    GUITHREADINFO info = new GUITHREADINFO();
    info.cbSize = Marshal.SizeOf(typeof(GUITHREADINFO));
    if (thread != 0 && GetGUIThreadInfo(thread, ref info) && info.hwndFocus != IntPtr.Zero) return info.hwndFocus;
    return window;
  }

  // The deepest child at a client point, in that child's own coordinates.
  static IntPtr Locate(IntPtr window, int x, int y, out int childX, out int childY) {
    childX = x; childY = y;
    POINT point; point.X = x; point.Y = y;
    IntPtr child = ChildWindowFromPointEx(window, point, Skip);
    if (child == IntPtr.Zero || child == window) return window;
    POINT at; at.X = x; at.Y = y;
    ClientToScreen(window, ref at);
    ScreenToClient(child, ref at);
    childX = at.X; childY = at.Y;
    return child;
  }

  static uint Down(int kind) { return kind == 1 ? RightDown : (kind == 2 ? MiddleDown : LeftDown); }
  static uint Release(int kind) { return kind == 1 ? RightUp : (kind == 2 ? MiddleUp : LeftUp); }
  static uint Held(int kind) { return kind == 1 ? 2u : (kind == 2 ? 16u : 1u); }
  static int Pause(int ms) { System.Threading.Thread.Sleep(ms); return ms; }

  public static bool Move(IntPtr window, int x, int y) {
    if (!Alive(window)) return false;
    int cx, cy; IntPtr child = Locate(window, x, y, out cx, out cy);
    Aim(window, x, y);
    return PostMessage(child, MouseMove, IntPtr.Zero, Pad(cx, cy));
  }

  public static bool Click(IntPtr window, int x, int y, int kind, int count) {
    if (!Alive(window)) return false;
    int cx, cy; IntPtr child = Locate(window, x, y, out cx, out cy);
    Aim(window, x, y);
    PostMessage(child, MouseMove, IntPtr.Zero, Pad(cx, cy));
    Pause(20);
    for (int i = 0; i < count; i++) {
      PostMessage(child, Down(kind), (IntPtr)Held(kind), Pad(cx, cy));
      Pause(30);
      PostMessage(child, Release(kind), IntPtr.Zero, Pad(cx, cy));
      if (i + 1 < count) Pause(90);
    }
    return true;
  }

  // WM_MOUSEWHEEL carries screen coordinates, not client ones.
  public static bool Scroll(IntPtr window, int x, int y, int delta) {
    if (!Alive(window)) return false;
    int cx, cy; IntPtr child = Locate(window, x, y, out cx, out cy);
    Aim(window, x, y);
    POINT screen; screen.X = x; screen.Y = y;
    ClientToScreen(window, ref screen);
    return PostMessage(child, Wheel, (IntPtr)(delta << 16), Pad(screen.X, screen.Y));
  }

  static IntPtr dragChild;
  static int dragKind, dragX, dragY;
  public static bool DragBegin(IntPtr window, int x, int y, int kind) {
    if (!Alive(window)) return false;
    dragChild = Locate(window, x, y, out dragX, out dragY);
    dragKind = kind;
    Aim(window, x, y);
    PostMessage(dragChild, MouseMove, IntPtr.Zero, Pad(dragX, dragY));
    Pause(20);
    return PostMessage(dragChild, Down(kind), (IntPtr)Held(kind), Pad(dragX, dragY));
  }
  public static void DragStep(IntPtr window, int x, int y) {
    if (!Alive(window)) return;
    int cx, cy; IntPtr child = Locate(window, x, y, out cx, out cy);
    Aim(window, x, y);
    PostMessage(child, MouseMove, (IntPtr)Held(dragKind), Pad(cx, cy));
    dragChild = child; dragX = cx; dragY = cy;
  }
  public static void DragEnd(IntPtr window) {
    if (!Alive(window)) return;
    PostMessage(dragChild, Release(dragKind), IntPtr.Zero, Pad(dragX, dragY));
  }

  public static void Key(IntPtr window, int virtualKey) {
    IntPtr target = Focused(window);
    PostMessage(target, KeyDown, (IntPtr)virtualKey, (IntPtr)1);
    Pause(15);
    PostMessage(target, KeyUp, (IntPtr)virtualKey, Up());
  }

  public static void Chord(IntPtr window, int[] modifiers, int virtualKey) {
    IntPtr target = Focused(window);
    foreach (int modifier in modifiers) PostMessage(target, KeyDown, (IntPtr)modifier, (IntPtr)1);
    Pause(10);
    PostMessage(target, KeyDown, (IntPtr)virtualKey, (IntPtr)1);
    Pause(15);
    PostMessage(target, KeyUp, (IntPtr)virtualKey, Up());
    for (int i = modifiers.Length - 1; i >= 0; i--) PostMessage(target, KeyUp, (IntPtr)modifiers[i], Up());
  }

  public static void Text(IntPtr window, string text) {
    IntPtr target = Focused(window);
    foreach (char character in text) {
      if (character == '\n') {
        PostMessage(target, KeyDown, (IntPtr)13, (IntPtr)1);
        PostMessage(target, Char, (IntPtr)13, (IntPtr)1);
        PostMessage(target, KeyUp, (IntPtr)13, Up());
      } else {
        PostMessage(target, Char, (IntPtr)character, (IntPtr)1);
      }
      Pause(4);
    }
  }

  static bool Blank(Bitmap bitmap) {
    int stepX = Math.Max(1, bitmap.Width / 24);
    int stepY = Math.Max(1, bitmap.Height / 24);
    for (int y = 0; y < bitmap.Height; y += stepY) {
      for (int x = 0; x < bitmap.Width; x += stepX) {
        Color pixel = bitmap.GetPixel(x, y);
        if (pixel.R != 0 || pixel.G != 0 || pixel.B != 0) return false;
      }
    }
    return true;
  }

  // The window's own pixels, cropped to the requested client rectangle, or the
  // screen when the window cannot render itself. Returns how it was captured.
  public static string Grab(IntPtr window, string path, int x, int y, int width, int height) {
    int[] client = Client(window);
    if (client[2] <= 0 || client[3] <= 0) return "";
    Bitmap full = new Bitmap(client[2], client[3]);
    string how = "print";
    try {
      Graphics graphics = Graphics.FromImage(full);
      try {
        IntPtr hdc = graphics.GetHdc();
        bool drawn;
        try { drawn = PrintWindow(window, hdc, 2); } finally { graphics.ReleaseHdc(hdc); }
        if (!drawn || Blank(full)) {
          graphics.CopyFromScreen(client[0], client[1], 0, 0, new Size(client[2], client[3]));
          how = "screen";
        }
      } finally { graphics.Dispose(); }
      if (width > 0 && height > 0) {
        using (Bitmap crop = new Bitmap(width, height)) {
          using (Graphics cg = Graphics.FromImage(crop)) {
            cg.DrawImage(full, new Rectangle(0, 0, width, height), new Rectangle(x, y, width, height), GraphicsUnit.Pixel);
          }
          crop.Save(path, ImageFormat.Png);
        }
      } else {
        full.Save(path, ImageFormat.Png);
      }
    } finally { full.Dispose(); }
    return how;
  }
}
"#;

/// Where the running actions leave the window they last aimed at, so the
/// "after" capture follows a `focus_window` step to its new target.
pub(super) fn status_path() -> PathBuf {
    crate::temp::temp_root().join(format!("yopes-agent-window-{}", std::process::id()))
}

/// The client pixel the agent last aimed at in `handle`, if the last call aimed
/// in the same window; a glide never starts from another window's point.
fn remembered(handle: i64) -> Option<(i32, i32)> {
    let text = std::fs::read_to_string(pointer_path()).ok()?;
    let mut parts = text.split_whitespace();
    if parts.next()?.parse::<i64>().ok()? != handle {
        return None;
    }
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn pointer_path() -> PathBuf {
    crate::temp::temp_root().join(format!("yopes-agent-window-pointer-{}", std::process::id()))
}

fn remember(handle: i64, at: (i32, i32)) {
    let _ = std::fs::write(pointer_path(), format!("{handle} {} {}", at.0, at.1));
}

/// The PowerShell that finds the window: its handle, client area and title, in
/// one line plus the title on its own.
pub(super) fn resolve_script(title: &str) -> String {
    let quoted = ps_quote(title);
    format!(
        "Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'\n{HELPERS}\n'@;\n\
         $h = [YopesWindowMode]::ByTitle({quoted});\n\
         if ($h -eq [IntPtr]::Zero) {{ [Console]::Error.Write('no window title contains ' + {quoted}); exit 4 }}\n\
         $c = [YopesWindowMode]::Client($h);\n\
         Write-Output ('window ' + $h.ToInt64().ToString() + ' ' + $c[0].ToString() + ' ' + $c[1].ToString() + ' ' + $c[2].ToString() + ' ' + $c[3].ToString());\n\
         Write-Output ('title ' + [YopesWindowMode]::Title($h));\n\
         exit 0;"
    )
}

/// Read the two lines `resolve_script` prints back into a `Target`.
pub(super) fn parse_resolve(stdout: &str) -> Option<Target> {
    let mut target = None;
    let mut title = None;
    for line in stdout.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("window ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() < 5 {
                return None;
            }
            let number = |index: usize| -> Option<i64> { parts.get(index)?.parse().ok() };
            target = Some((
                number(0)?,
                (
                    i32::try_from(number(1)?).ok()?,
                    i32::try_from(number(2)?).ok()?,
                    u32::try_from(number(3)?).ok()?,
                    u32::try_from(number(4)?).ok()?,
                ),
            ));
        } else if let Some(rest) = line.strip_prefix("title ") {
            title = Some(rest.to_owned());
        }
    }
    let (handle, client) = target?;
    Some(Target {
        handle,
        title: title.unwrap_or_default(),
        client,
    })
}

/// The PowerShell that captures the window's own client area.
pub(super) fn capture_script(handle: i64, region: Option<Region>, path: &Path) -> String {
    let (x, y, width, height) = match region {
        Some(region) => (
            region.x,
            region.y,
            i64::from(region.width),
            i64::from(region.height),
        ),
        None => (0, 0, 0, 0),
    };
    format!(
        "Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'\n{HELPERS}\n'@;\n\
         $h = [IntPtr]::new({handle});\n\
         if (-not [YopesWindowMode]::Alive($h)) {{ [Console]::Error.Write('the window is gone'); exit 6 }}\n\
         $how = [YopesWindowMode]::Grab($h, {}, {x}, {y}, {width}, {height});\n\
         if ($how.Length -eq 0) {{ [Console]::Error.Write('could not capture the window'); exit 7 }}\n\
         $c = [YopesWindowMode]::Client($h);\n\
         Write-Output ('grab ' + $how);\n\
         Write-Output ('client ' + $c[0].ToString() + ' ' + $c[1].ToString() + ' ' + $c[2].ToString() + ' ' + $c[3].ToString());\n\
         exit 0;",
        ps_quote(&path.display().to_string())
    )
}

/// Read the `grab` and `client` lines of `capture_script`: how the pixels were
/// taken, and the client area in screen pixels.
pub(super) fn parse_capture(stdout: &str) -> Option<(&str, (i32, i32, u32, u32))> {
    let mut how = None;
    let mut client = None;
    for line in stdout.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("grab ") {
            how = Some(rest);
        } else if let Some(rest) = line.strip_prefix("client ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() < 4 {
                return None;
            }
            let number = |index: usize| -> Option<i64> { parts.get(index)?.parse().ok() };
            client = Some((
                i32::try_from(number(0)?).ok()?,
                i32::try_from(number(1)?).ok()?,
                u32::try_from(number(2)?).ok()?,
                u32::try_from(number(3)?).ok()?,
            ));
        }
    }
    Some((how?, client?))
}

/// Read the status file the action script leaves behind: the last window it
/// aimed at, as `handle`, `x y w h`, and the title.
pub(super) fn parse_status(text: &str) -> Option<Target> {
    let mut lines = text.lines().map(str::trim);
    let handle = lines.next()?.parse::<i64>().ok()?;
    let client: Vec<i32> = lines
        .next()?
        .split_whitespace()
        .map(|part| part.parse::<i32>().ok())
        .collect::<Option<Vec<i32>>>()?;
    if client.len() < 4 {
        return None;
    }
    Some(Target {
        handle,
        title: lines.next().unwrap_or_default().to_owned(),
        client: (
            client[0],
            client[1],
            u32::try_from(client[2]).ok()?,
            u32::try_from(client[3]).ok()?,
        ),
    })
}

/// A virtual key code for the named keys a `key` step accepts.
pub(super) fn virtual_key(name: &str) -> anyhow::Result<i32> {
    let upper = name.trim().to_ascii_uppercase();
    let code = match upper.as_str() {
        "ENTER" | "RETURN" => 0x0D,
        "TAB" => 0x09,
        "ESC" | "ESCAPE" => 0x1B,
        "SPACE" => 0x20,
        "BACKSPACE" => 0x08,
        "DELETE" | "DEL" => 0x2E,
        "INSERT" => 0x2D,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        "UP" => 0x26,
        "DOWN" => 0x28,
        "LEFT" => 0x25,
        "RIGHT" => 0x27,
        "F1" => 0x70,
        "F2" => 0x71,
        "F3" => 0x72,
        "F4" => 0x73,
        "F5" => 0x74,
        "F6" => 0x75,
        "F7" => 0x76,
        "F8" => 0x77,
        "F9" => 0x78,
        "F10" => 0x79,
        "F11" => 0x7A,
        "F12" => 0x7B,
        other if other.chars().count() == 1 => {
            // A single character keys as its uppercase ASCII code.
            u32::from(upper.chars().next().unwrap_or(' ')) as i32 & 0x7F
        }
        other => anyhow::bail!(
            "unknown key `{other}`; use ENTER, TAB, ESC, SPACE, BACKSPACE, DELETE, INSERT, \
             HOME, END, PAGEUP, PAGEDOWN, UP, DOWN, LEFT, RIGHT, F1..F12 or one character"
        ),
    };
    Ok(code)
}

/// `ctrl+shift+t` into the modifier codes and the final key code.
pub(super) fn chord_keys(chord: &str) -> anyhow::Result<(Vec<i32>, i32)> {
    let parts: Vec<&str> = chord
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        anyhow::bail!("a key action needs keys or a chord");
    }
    let (modifiers, final_key) = parts.split_at(parts.len().saturating_sub(1));
    let mut codes = Vec::with_capacity(modifiers.len());
    for modifier in modifiers {
        codes.push(match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => 0x11,
            "alt" => 0x12,
            "shift" => 0x10,
            "win" | "windows" => 0x5B,
            other => {
                anyhow::bail!(
                    "unknown modifier `{other}` in `{chord}`; use ctrl, alt, shift or win"
                )
            }
        });
    }
    Ok((codes, virtual_key(final_key[0])?))
}

/// The button kind the shared C# understands: 0 left, 1 right, 2 middle.
fn button_kind(button: Button) -> i32 {
    match button {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => 2,
    }
}

/// The hover posts that walk the agent's arrow from `from` to `to` inside the
/// window. Every point but the last may be dropped by the window; the point the
/// step acts on must be accepted.
fn hover(root: &str, from: Option<(i32, i32)>, to: (i32, i32)) -> String {
    let path = glide(from, to);
    let last = path.len().saturating_sub(1);
    let mut out = String::new();
    for (index, (x, y)) in path.iter().enumerate() {
        if index == last {
            out.push_str(&format!(
                "if (-not [YopesWindowMode]::Move({root}, {x}, {y})) {{ [Console]::Error.Write('the window could not take a point at ({x}, {y})'); exit 5 }};\n"
            ));
        } else {
            out.push_str(&format!(
                "[void][YopesWindowMode]::Move({root}, {x}, {y});\nStart-Sleep -Milliseconds {GLIDE_STEP_MS};\n"
            ));
        }
    }
    out
}

/// The script that posts every step to the window in order.
pub(super) fn window_script(
    steps: &[Step],
    start: Option<(i32, i32)>,
    handle: i64,
    status: &Path,
) -> anyhow::Result<String> {
    let mut script = format!(
        "Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'\n{HELPERS}\n'@;\n\
         $ErrorActionPreference = 'Stop';\n\
         [YopesWindowMode]::CursorPath = {};\n\
         $h = [IntPtr]::new({handle});\n\
         if (-not [YopesWindowMode]::Alive($h)) {{ [Console]::Error.Write('the window is gone'); exit 6 }}\n",
        ps_quote(&super::cursor::state_path().display().to_string())
    );
    let root = "$h";
    let mut last = start;
    for step in steps {
        match step {
            Step::Move { x, y } => {
                script.push_str(&hover(root, last, (*x, *y)));
                script.push_str("Start-Sleep -Milliseconds 60;\n");
                last = Some((*x, *y));
            }
            Step::Click {
                x,
                y,
                button,
                count,
            } => {
                let count = (*count).clamp(1, 3);
                script.push_str(&hover(root, last, (*x, *y)));
                script.push_str(&format!(
                    "if (-not [YopesWindowMode]::Click({root}, {x}, {y}, {}, {count})) {{ [Console]::Error.Write('the window could not take a click at ({x}, {y})'); exit 5 }};\nStart-Sleep -Milliseconds 60;\n",
                    button_kind(*button)
                ));
                last = Some((*x, *y));
            }
            Step::DoubleClick { x, y } => {
                script.push_str(&hover(root, last, (*x, *y)));
                script.push_str(&format!(
                    "if (-not [YopesWindowMode]::Click({root}, {x}, {y}, 0, 2)) {{ [Console]::Error.Write('the window could not take a click at ({x}, {y})'); exit 5 }};\nStart-Sleep -Milliseconds 60;\n"
                ));
                last = Some((*x, *y));
            }
            Step::Scroll { x, y, amount } => {
                let delta = (*amount).clamp(-10, 10) * 120;
                script.push_str(&hover(root, last, (*x, *y)));
                script.push_str(&format!(
                    "if (-not [YopesWindowMode]::Scroll({root}, {x}, {y}, {delta})) {{ [Console]::Error.Write('the window could not take a scroll at ({x}, {y})'); exit 5 }};\nStart-Sleep -Milliseconds 120;\n"
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
                script.push_str(&hover(root, last, (*from_x, *from_y)));
                script.push_str(&format!(
                    "if (-not [YopesWindowMode]::DragBegin({root}, {from_x}, {from_y}, {})) {{ [Console]::Error.Write('the window could not take a drag at ({from_x}, {from_y})'); exit 5 }};\n",
                    button_kind(*button)
                ));
                for (x, y) in glide(Some((*from_x, *from_y)), (*to_x, *to_y)) {
                    script.push_str(&format!(
                        "[YopesWindowMode]::DragStep({root}, {x}, {y});\nStart-Sleep -Milliseconds {GLIDE_STEP_MS};\n"
                    ));
                }
                script.push_str("Start-Sleep -Milliseconds 40;\n[YopesWindowMode]::DragEnd($h);\nStart-Sleep -Milliseconds 120;\n");
                last = Some((*to_x, *to_y));
            }
            Step::Type { text } => {
                script.push_str(&format!(
                    "[YopesWindowMode]::Text({root}, {});\nStart-Sleep -Milliseconds 120;\n",
                    ps_quote(text)
                ));
            }
            Step::Key { keys, chord } => {
                if !chord.trim().is_empty() {
                    let (modifiers, key) = chord_keys(chord)?;
                    let list = modifiers
                        .iter()
                        .map(i32::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    script.push_str(&format!(
                        "[YopesWindowMode]::Chord({root}, @({list}), {key});\nStart-Sleep -Milliseconds 120;\n"
                    ));
                } else {
                    for key in keys {
                        script.push_str(&format!(
                            "[YopesWindowMode]::Key({root}, {});\nStart-Sleep -Milliseconds 80;\n",
                            virtual_key(key)?
                        ));
                    }
                }
            }
            Step::Wait { ms } => {
                script.push_str(&format!(
                    "Start-Sleep -Milliseconds {};\n",
                    (*ms).min(super::MAX_WAIT_MS)
                ));
            }
            Step::FocusWindow { title } => {
                // Background mode retargets instead of raising a window: the
                // steps that follow act on the new one, and it never comes to
                // the front.
                script.push_str(&format!(
                    "$h = [YopesWindowMode]::ByTitle({});\n\
                     if ($h -eq [IntPtr]::Zero) {{ [Console]::Error.Write('no window title contains ' + {}); exit 4 }}\n\
                     Start-Sleep -Milliseconds 150;\n",
                    ps_quote(title),
                    ps_quote(title)
                ));
                last = None;
            }
        }
    }
    // Leave the last target behind, so the "after" capture follows a
    // focus_window step to the window it switched to.
    script.push_str(&format!(
        "$c = [YopesWindowMode]::Client($h);\n\
         [System.IO.File]::WriteAllLines({}, @([string]$h.ToInt64(), ($c[0].ToString() + ' ' + $c[1].ToString() + ' ' + $c[2].ToString() + ' ' + $c[3].ToString()), [YopesWindowMode]::Title($h)));\n\
         exit 0;\n",
        ps_quote(&status.display().to_string())
    ));
    Ok(script)
}

/// Capture a window into a PNG plus the geometry the model needs to aim.
pub(super) fn capture(handle: i64, region: Option<Region>) -> anyhow::Result<(Shot, String)> {
    let path = crate::temp::unique_path("yopes-window", "png");
    let stdout = super::run_script(&capture_script(handle, region, &path))?;
    if !path.is_file() {
        anyhow::bail!("could not capture the window: no image was written");
    }
    let (how, client) = parse_capture(&stdout).ok_or_else(|| {
        anyhow::anyhow!(
            "could not read the window geometry from `{}`",
            stdout.trim()
        )
    })?;
    let (origin, size) = match region {
        Some(region) => ((region.x, region.y), (region.width, region.height)),
        None => ((0, 0), (client.2, client.3)),
    };
    Ok((
        Shot {
            path,
            origin,
            size,
            desktop: client,
            cursor: None,
            target: None,
        },
        how.to_owned(),
    ))
}

/// Everything one background call does: resolve the window, capture it, post
/// the steps, capture again, and report in the same shape a foreground call
/// does, so the model reads both the same way.
pub(super) fn run(
    intent: &str,
    title: &str,
    steps: &[Step],
    region: Option<Region>,
    settle: u64,
) -> anyhow::Result<String> {
    let target = resolve(title)?;
    let (mut before, _) = capture(target.handle, region)?;
    before.target = Some(target.title.clone());
    super::cursor::ensure(
        remembered(target.handle).map(|(x, y)| (x + target.client.0, y + target.client.1)),
    )?;
    let status = status_path();
    let _ = std::fs::remove_file(&status);
    let script = window_script(steps, remembered(target.handle), target.handle, &status)?;
    super::run_script(&script)?;
    if !steps.is_empty() && settle > 0 {
        std::thread::sleep(std::time::Duration::from_millis(settle));
    }
    let ended = std::fs::read_to_string(&status)
        .ok()
        .and_then(|text| parse_status(&text))
        .unwrap_or(target.clone());
    let (mut after, how) = capture(ended.handle, region)?;
    after.target = Some(if ended.title.is_empty() {
        title.to_owned()
    } else {
        ended.title.clone()
    });

    // The agent's arrow sits over the window it drove: the overlay wants
    // screen pixels, so the client point is offset by the window's origin.
    let pointer = pointer_target(steps).or_else(|| remembered(ended.handle));
    let marked = pointer.is_some_and(|at| {
        remember(ended.handle, at);
        draw_agent_cursor(&after.path, after.origin, after.size, (at.0, at.1)).unwrap_or(false)
    });

    let mut report = String::new();
    report.push_str("Action: ");
    report.push_str(if intent.is_empty() {
        "(no description)"
    } else {
        intent
    });
    report.push('\n');
    report.push_str("Actions run: ");
    report.push_str(&if steps.is_empty() {
        "none".to_owned()
    } else {
        steps.iter().map(describe).collect::<Vec<_>>().join("; ")
    });
    report.push('\n');
    report.push_str("Mode: background — input was posted to the window, the system pointer did not move and no window was raised\n");
    report.push_str(&before.describe());
    report.push_str(&format!(
        "Capture: {}\n",
        if how == "print" {
            "the window rendered itself"
        } else {
            "the screen was copied over the window, so an occluded window shows whatever covers it"
        }
    ));
    if let Some((x, y)) = pointer {
        report.push_str(&format!(
            "Pointer: client ({x}, {y}){}\n",
            if marked { ", drawn in the capture" } else { "" }
        ));
    }
    report.push_str("After: ");
    report.push_str(&after.path.display().to_string());
    Ok(super::truncate_output(report))
}

/// Find the window whose title contains `title`, case-insensitively.
fn resolve(title: &str) -> anyhow::Result<Target> {
    let stdout = super::run_script(&resolve_script(title))?;
    parse_resolve(&stdout).ok_or_else(|| {
        anyhow::anyhow!(
            "could not read the window geometry from `{}`",
            stdout.trim()
        )
    })
}

#[cfg(test)]
#[path = "window_tests.rs"]
mod extracted_tests;
