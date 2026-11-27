//! The state machine's vocabulary: notices shown to the user, the actions
//! `main` dispatches, and the reports a turn streams back.

use crate::config::AuthKind;
use crate::diff::FileDiff;
use crate::session::{Message, ToolCall};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeKind {
    Info,
    Error,
}

/// A drag over the transcript, in cells.
///
/// Columns are screen columns inside the transcript, rows are absolute rows of
/// the whole conversation (not screen rows), so the selection survives
/// scrolling and a redraw. `anchor` is where the drag began and `head` where
/// the pointer is now: they are read in order, so a drag in either direction
/// marks the same text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSelection {
    pub anchor: (usize, u16),
    pub head: (usize, u16),
}

impl TextSelection {
    /// The selection with its ends in reading order.
    pub fn ordered(&self) -> ((usize, u16), (usize, u16)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

pub struct Notice {
    pub kind: NoticeKind,
    pub message: String,

    /// When the notice was raised: the footer dismisses it after a few
    /// seconds instead of leaving an old error sitting over new output.
    pub started: Instant,
}

pub enum Action {
    None,
    Start {
        request_id: u64,
        messages: Vec<Message>,
        tools_enabled: bool,
        /// Mode instruction appended to the system prompt for this request.
        mode_note: Option<String>,
    },
    Cancel(u64),
    /// Kill the foreground commands one session owns: the Esc interrupt is
    /// scoped to the interrupting tab, so other tabs' commands survive.
    KillCommands(u64),
    RunTool {
        call: ToolCall,
    },
    /// Start several tool calls at once. Delegate calls in the batch run as
    /// concurrent subagents (they stack in the sidebar); at most one call is
    /// a non-delegate, tracked by the sequential tool machinery.
    RunTools {
        calls: Vec<ToolCall>,
    },
    /// Run a user command (from `!` or `/run`) in the background, so a long
    /// run cannot soft-lock the event loop; the result lands as its own
    /// message.
    RunUserCommand {
        command: String,
    },
    /// Read the clipboard on Ctrl+V and attach an image when it holds one.
    PasteImage,
    /// Copy text to system clipboard.
    CopyClipboard(String),
    Quit,
    /// Fetch a custom provider's model list from its models endpoint.
    ///
    /// `base_url` is the endpoint the fetched list belongs to, so the saved
    /// model list of the right provider is reconciled against the response.
    FetchCustomModels {
        url: String,
        base_url: Option<String>,
        token: Option<String>,
        auth: AuthKind,
    },
    /// `/generate`: one image model call. The bytes come back through the
    /// image channel and are written to the workspace when they arrive.
    GenerateImage {
        base_url: String,
        auth: AuthKind,
        key: Option<String>,
        model: String,
        prompt: String,
    },
    /// Ask the provider for a short title after the first reply of a session.
    NameSession {
        task: String,
    },
}

pub struct ToolEvent {
    pub call: ToolCall,
    pub result: std::result::Result<String, String>,
}

/// A request that failed and is being tried again.
#[derive(Clone, Debug)]
pub struct RetryNotice {
    pub attempt: u32,
    pub delay_secs: u64,
    pub reason: String,
    /// When the notice was raised: the footer counts the wait down from here
    /// instead of repeating the delay it started with, which read as a
    /// request stuck on the same number.
    pub since: Instant,
}

impl RetryNotice {
    /// Seconds left of the backoff, counting down on the ticker.
    pub fn remaining_secs(&self) -> u64 {
        let waited = self.since.elapsed().as_secs();
        self.delay_secs.saturating_sub(waited)
    }
}

/// A diff shown under a tool result in the conversation.
pub struct ChangeReport {
    pub diffs: Vec<FileDiff>,
    pub side_by_side: bool,
}

#[cfg(test)]
mod tests {
    use super::RetryNotice;
    use std::time::{Duration, Instant};

    #[test]
    fn the_retry_wait_counts_down_instead_of_repeating_itself() {
        let notice = RetryNotice {
            attempt: 2,
            delay_secs: 5,
            reason: "overloaded".to_owned(),
            since: Instant::now(),
        };
        // Fresh: the whole delay is still ahead.
        assert_eq!(notice.remaining_secs(), 5);
        // Three seconds in: the footer says two, not five again.
        let aged = RetryNotice {
            since: Instant::now() - Duration::from_secs(3),
            ..notice
        };
        assert_eq!(aged.remaining_secs(), 2);
        // Past the delay it floors at zero instead of underflowing.
        let spent = RetryNotice {
            since: Instant::now() - Duration::from_secs(30),
            ..aged
        };
        assert_eq!(spent.remaining_secs(), 0);
    }
}
