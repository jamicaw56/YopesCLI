//! Talking between sessions: `check_session` lists the open tabs a session
//! can address, and `talk_to` delivers a message into one tab's conversation.
//!
//! The tool layer holds no session state — every tab belongs to the app loop,
//! so both calls send a request over a channel and wait for the loop's
//! answer. The answer is the tool result the model sees: whether the message
//! landed at once (the receiving tab was idle and its model woke) or waits
//! for that tab's current turn to end, or why nothing was sent. The loop
//! answers within a tick; the wait only guards against a loop that stopped.

use std::sync::{OnceLock, mpsc};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;
use tokio::sync::mpsc::UnboundedSender;

use super::parse;

/// One request from a talk tool call to the app loop, which owns the tabs.
pub(crate) enum TalkRequest {
    /// `check_session`: the reply carries the directory of open tabs.
    List {
        /// Session id of the caller, so its own row is marked in the list.
        from: u64,
        reply: mpsc::Sender<String>,
    },
    /// `talk_to`: deliver `message` into tab `tab` (1-based, from
    /// `check_session`).
    Send {
        /// Session id of the caller: the delivery names the sender's tab.
        from: u64,
        tab: usize,
        message: String,
        reply: mpsc::Sender<String>,
    },
}

static TALK: OnceLock<UnboundedSender<TalkRequest>> = OnceLock::new();

/// Install the channel the talk tools send through. Called once from `main`,
/// before any session can run a tool.
pub(crate) fn install(sender: UnboundedSender<TalkRequest>) {
    let _ = TALK.set(sender);
}

/// How long a talk tool waits for the app loop's answer. The loop answers
/// within a tick; the timeout only catches a loop that stopped running.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Send one request to the app loop and wait for its answer.
fn request(make: impl FnOnce(mpsc::Sender<String>) -> TalkRequest) -> Result<String> {
    let sender = TALK
        .get()
        .ok_or_else(|| anyhow!("the talk channel is not installed"))?;
    let (reply, receiver) = mpsc::channel();
    sender
        .send(make(reply))
        .map_err(|_| anyhow!("the app loop is not running; the talk request went unanswered"))?;
    receiver
        .recv_timeout(REPLY_TIMEOUT)
        .map_err(|_| anyhow!("the app loop did not answer the talk request; try again"))
}

#[derive(Deserialize)]
struct TalkToArgs {
    tab: usize,
    message: String,
}

/// `check_session`: the directory of open tabs, answered by the app loop
/// from a live snapshot.
pub(super) fn check_session() -> Result<String> {
    request(|reply| TalkRequest::List {
        from: super::current_owner(),
        reply,
    })
}

/// `talk_to`: one message into another tab's conversation.
pub(super) fn talk_to(arguments: &str) -> Result<String> {
    let args: TalkToArgs = parse(arguments)?;
    let message = args.message.trim();
    if message.is_empty() {
        bail!("message cannot be empty");
    }
    if args.tab == 0 {
        bail!("tab numbers start at 1; check_session lists the open tabs");
    }
    request(|reply| TalkRequest::Send {
        from: super::current_owner(),
        tab: args.tab,
        message: message.to_owned(),
        reply,
    })
}

#[cfg(test)]
mod tests {
    use super::talk_to;

    #[test]
    fn talk_to_rejects_an_empty_message_and_a_zero_tab_before_any_delivery() {
        // These bail before the channel is consulted, so they hold even
        // without the app loop running.
        let empty = talk_to(r#"{"tab":1,"message":"   "}"#)
            .unwrap_err()
            .to_string();
        assert!(empty.contains("message cannot be empty"), "{empty}");
        let zero = talk_to(r#"{"tab":0,"message":"hi"}"#)
            .unwrap_err()
            .to_string();
        assert!(zero.contains("tab numbers start at 1"), "{zero}");
        // A missing message never parses, so it is rejected before the check.
        let missing = talk_to(r#"{"tab":1}"#).unwrap_err().to_string();
        assert!(missing.contains("invalid tool arguments"), "{missing}");
    }
}
