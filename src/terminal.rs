//! Terminal setup: raw mode, the alternate screen, and the RAII guard that
//! restores the terminal on every exit path.

use std::{
    future::Future,
    io::{self, Stdout},
    pin::Pin,
};

use anyhow::{Context, Result};
use crossterm::{
    cursor::Show,
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
    },
};
use ratatui::{Terminal, backend::CrosstermBackend};

pub(crate) struct TerminalGuard {
    pub(crate) terminal: Terminal<CrosstermBackend<Stdout>>,
}

/// The teardown every exit path shares: mouse, paste, alternate screen, cursor.
fn restore_terminal(writer: &mut impl io::Write) -> io::Result<()> {
    execute!(
        writer,
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        Show
    )
}

pub(crate) fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = restore_terminal(&mut io::stdout());
        default_hook(info);
    }));
}

impl TerminalGuard {
    pub(crate) fn new() -> Result<Self> {
        enable_raw_mode().context("enabling terminal raw mode")?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture,
            SetTitle("yopes"),
            Show
        ) {
            let _ = disable_raw_mode();
            let _ = restore_terminal(&mut stdout);
            return Err(error).context("initializing terminal");
        }
        let backend = CrosstermBackend::new(stdout);
        match Terminal::new(backend) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = restore_terminal(&mut io::stdout());
                Err(error).context("creating terminal")
            }
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = restore_terminal(self.terminal.backend_mut());
        let _ = self.terminal.show_cursor();
    }
}

/// Resolves when the user interrupts. Works on every platform, including when
/// the key event itself is swallowed.
pub(crate) fn ctrl_c_interrupt() -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(async {
        let _ = tokio::signal::ctrl_c().await;
    })
}
