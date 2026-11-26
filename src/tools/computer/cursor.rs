//! The visible desktop half of the agent pointer.
//!
//! A tiny WPF window follows the state file written by mouse actions. It is
//! transparent, click-through and never activates, so the physical cursor and
//! keyboard focus stay with the user. The PowerShell process watches its yopes
//! owner and closes the overlay when that process exits.

#[cfg(windows)]
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

#[cfg(windows)]
use anyhow::Context;

/// Mouse actions and the overlay share a per-yopes-process state file. The
/// older, cross-run pointer file is only a starting position for a new run.
pub(super) fn state_path() -> std::path::PathBuf {
    crate::temp::temp_root().join(format!("yopes-agent-pointer-{}", std::process::id()))
}

#[cfg(windows)]
fn artifact_path(extension: &str) -> PathBuf {
    crate::temp::temp_root().join(format!(
        "yopes-agent-cursor-{}.{extension}",
        std::process::id()
    ))
}

/// Start the overlay once, or restart it if its process died. A ready file is
/// the handshake: mouse input must not run invisibly when WPF failed to start.
#[cfg(windows)]
pub(super) fn ensure(initial: Option<(i32, i32)>) -> anyhow::Result<()> {
    static PROCESS: Mutex<Option<Child>> = Mutex::new(None);

    let mut process = PROCESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(child) = process.as_mut() {
        match child.try_wait() {
            Ok(None) => return Ok(()),
            Ok(Some(_)) => {}
            Err(error) => anyhow::bail!("checking the agent cursor process: {error}"),
        }
    }
    if let Some(mut old) = process.take() {
        let _ = old.kill();
        let _ = old.wait();
    }

    let state = state_path();
    if let Some((x, y)) = initial.filter(|_| !state.is_file()) {
        fs::write(&state, format!("{x} {y}")).context("initializing the agent cursor")?;
    }
    let script = artifact_path("ps1");
    let ready = artifact_path("ready");
    let log = artifact_path("log");
    fs::write(&script, SCRIPT).context("writing the agent cursor script")?;
    let _ = fs::remove_file(&ready);
    let log_file = fs::File::create(&log).context("creating the agent cursor log")?;
    let mut child = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .arg("-StatePath")
        .arg(&state)
        .arg("-ReadyPath")
        .arg(&ready)
        .arg("-OwnerId")
        .arg(std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file))
        .spawn()
        .context("starting the agent cursor overlay")?;

    // WPF can take several seconds to initialize on a busy desktop. The script
    // writes the ready file directly, so seeing it is not proof that the whole
    // signal has been written; tolerate partial reads while waiting.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if ready.is_file() {
            let message = fs::read_to_string(&ready).unwrap_or_default();
            match startup_signal(&message) {
                StartupSignal::Ready => {
                    *process = Some(child);
                    return Ok(());
                }
                StartupSignal::Error(detail) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!("could not show the agent cursor: {detail}");
                }
                StartupSignal::Pending => {}
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let detail = startup_detail(&log);
                anyhow::bail!(
                    "agent cursor exited with {status}{}",
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(": {detail}")
                    }
                );
            }
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("checking the agent cursor startup: {error}");
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let detail = startup_detail(&log);
            anyhow::bail!(
                "agent cursor startup timed out; no mouse actions were run{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!("; PowerShell log: {detail}")
                }
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[derive(Debug, PartialEq, Eq)]
#[cfg(any(windows, test))]
enum StartupSignal {
    Ready,
    Error(String),
    Pending,
}

/// The ready file is written directly by PowerShell: tolerate a transient
/// empty/partial read, and only treat an explicit error signal as failure.
#[cfg(any(windows, test))]
fn startup_signal(message: &str) -> StartupSignal {
    let message = message.trim();
    if message == "ready" {
        StartupSignal::Ready
    } else if let Some(detail) = message.strip_prefix("error:") {
        StartupSignal::Error(detail.trim().to_owned())
    } else {
        StartupSignal::Pending
    }
}

#[cfg(windows)]
fn startup_detail(log: &Path) -> String {
    fs::read_to_string(log)
        .unwrap_or_default()
        .trim()
        .chars()
        .take(500)
        .collect()
}

/// PowerShell and WPF are part of the Windows desktop, so the single-binary
/// build needs no native library or new crate for the second pointer.
pub(super) const SCRIPT: &str = r##"param(
  [Parameter(Mandatory=$true)][string]$StatePath,
  [Parameter(Mandatory=$true)][string]$ReadyPath,
  [Parameter(Mandatory=$true)][int]$OwnerId
)
$ErrorActionPreference = 'Stop'
try {
  Add-Type -AssemblyName PresentationFramework, PresentationCore, WindowsBase
  Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class YopesCursorWindow {
  [DllImport("user32.dll")] static extern int GetWindowLong(IntPtr window, int index);
  [DllImport("user32.dll")] static extern int SetWindowLong(IntPtr window, int index, int value);
  [DllImport("user32.dll")] static extern bool SetWindowDisplayAffinity(IntPtr window, uint affinity);
  [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
  [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr window);
  [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr window, IntPtr insertAfter, int x, int y, int width, int height, uint flags);
  public static void UsePixelCoordinates() {
    // PER_MONITOR_AWARE_V2: tool coordinates and SetWindowPos both use pixels.
    SetThreadDpiAwarenessContext(new IntPtr(-4));
  }
  public static void Prepare(IntPtr handle) {
    // Click-through, no taskbar button, and no activation on show or move.
    int style = GetWindowLong(handle, -20);
    SetWindowLong(handle, -20, style | 0x00000020 | 0x00000080 | 0x08000000);
    // The model gets its own drawn marker, so do not double it in captures.
    SetWindowDisplayAffinity(handle, 0x00000011);
  }
  public static void Move(IntPtr handle, int x, int y) {
    uint dpi = GetDpiForWindow(handle);
    if (dpi == 0) dpi = 96;
    int offset = (int)Math.Round(12.0 * dpi / 96.0);
    // SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE.
    SetWindowPos(handle, IntPtr.Zero, x - offset, y - offset, 0, 0, 0x0001 | 0x0004 | 0x0010);
  }
}
'@

  [YopesCursorWindow]::UsePixelCoordinates()
  $window = New-Object System.Windows.Window
  $window.Title = 'yopes agent cursor'
  $window.WindowStyle = [System.Windows.WindowStyle]::None
  $window.AllowsTransparency = $true
  $window.Background = [System.Windows.Media.Brushes]::Transparent
  $window.ResizeMode = [System.Windows.ResizeMode]::NoResize
  $window.WindowStartupLocation = [System.Windows.WindowStartupLocation]::Manual
  $window.Width = 36
  $window.Height = 40
  $window.Topmost = $true
  $window.ShowInTaskbar = $false
  $window.ShowActivated = $false
  $window.IsHitTestVisible = $false
  $window.Focusable = $false
  $window.Left = -10000
  $window.Top = -10000

  $canvas = New-Object System.Windows.Controls.Canvas
  $canvas.Width = 36
  $canvas.Height = 40
  $brush = New-Object System.Windows.Media.BrushConverter
  $halo = New-Object System.Windows.Shapes.Ellipse
  $halo.Width = 22
  $halo.Height = 22
  $halo.Fill = $brush.ConvertFromString('#60FFFFFF')
  [System.Windows.Controls.Canvas]::SetLeft($halo, 1)
  [System.Windows.Controls.Canvas]::SetTop($halo, 1)
  [void]$canvas.Children.Add($halo)

  $arrow = New-Object System.Windows.Shapes.Path
  $arrow.Data = [System.Windows.Media.Geometry]::Parse('M 12,12 L 12,28.5 L 16.2,24.4 L 19,30.6 L 21.7,29.4 L 18.9,23.4 L 24.2,23.4 Z')
  $arrow.Fill = $brush.ConvertFromString('#FF0C0C0C')
  $arrow.Stroke = $brush.ConvertFromString('#EBFFFFFF')
  $arrow.StrokeThickness = 1.5
  $arrow.StrokeLineJoin = [System.Windows.Media.PenLineJoin]::Round
  [void]$canvas.Children.Add($arrow)
  $window.Content = $canvas

  $window.add_SourceInitialized({
    $source = New-Object System.Windows.Interop.WindowInteropHelper($window)
    [YopesCursorWindow]::Prepare($source.Handle)
    $script:cursorWindow = $source.Handle
  })

  $script:lastCursorX = [int]::MinValue
  $script:lastCursorY = [int]::MinValue
  function Move-AgentCursor {
    if (-not (Test-Path -LiteralPath $StatePath)) { return }
    $stream = New-Object System.IO.FileStream($StatePath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    $reader = New-Object System.IO.StreamReader($stream)
    try {
      $parts = $reader.ReadToEnd().Trim() -split '\s+'
    } finally {
      $reader.Dispose()
    }
    if ($parts.Count -lt 2) { return }
    $x = 0
    $y = 0
    if ([int]::TryParse($parts[0], [ref]$x) -and [int]::TryParse($parts[1], [ref]$y)) {
      if ($x -ne $script:lastCursorX -or $y -ne $script:lastCursorY) {
        [YopesCursorWindow]::Move($script:cursorWindow, $x, $y)
        $script:lastCursorX = $x
        $script:lastCursorY = $y
      }
    }
  }

  $timer = New-Object System.Windows.Threading.DispatcherTimer
  $timer.Interval = [TimeSpan]::FromMilliseconds(40)
  $script:ownerCheckTicks = 0
  $timer.add_Tick({
    try {
      $script:ownerCheckTicks += 1
      if ($script:ownerCheckTicks -ge 12) {
        $script:ownerCheckTicks = 0
        if ($null -eq (Get-Process -Id $OwnerId -ErrorAction SilentlyContinue)) {
          $timer.Stop()
          $window.Close()
          [System.Windows.Threading.Dispatcher]::CurrentDispatcher.InvokeShutdown()
          return
        }
      }
      Move-AgentCursor
    } catch {
      # A state write may overlap this read; the next tick gets the whole point.
    }
  })
  $window.Show()
  Move-AgentCursor
  $timer.Start()
  [System.IO.File]::WriteAllText($ReadyPath, 'ready')
  [System.Windows.Threading.Dispatcher]::Run()
} catch {
  [System.IO.File]::WriteAllText($ReadyPath, 'error: ' + $_.Exception.Message)
  [Console]::Error.WriteLine($_)
  exit 1
} finally {
  Remove-Item -LiteralPath $ReadyPath -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $StatePath -Force -ErrorAction SilentlyContinue
}
"##;

#[cfg(test)]
#[path = "cursor_tests.rs"]
mod extracted_tests;
