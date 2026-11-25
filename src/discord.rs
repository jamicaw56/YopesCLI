//! Discord Rich Presence over the local IPC socket, opt-in.
//!
//! Discord exposes a named pipe (Windows) or a unix socket next to its
//! runtime dir, one per running client: `discord-ipc-0` upward. The protocol is
//! a frame per message: a little-endian `u32` opcode, a little-endian `u32`
//! payload length, then JSON.
//!
//! Nothing here runs unless the user set a client id in `/set discord`, so an
//! untouched install opens no socket and talks to nothing. The connection
//! lives on its own thread: a Discord restart costs a reconnect attempt, never
//! a frame of the interface.

use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::app::App;

/// The asset key of the large image: the blue pixel Y uploaded to the Discord
/// application as `yopes_logo`.
const LARGE_IMAGE: &str = "yopes_logo";
const LARGE_TEXT: &str = "YopesCLI";
/// How long before the presence is re-sent while nothing changes, so a Discord
/// restart picks the state back up without waiting for the user to act.
const RESEND_AFTER: Duration = Duration::from_secs(30);

/// What the presence card shows. Discord caps both text fields at 128
/// characters and rejects anything shorter than two, so every field is
/// clipped here rather than at the socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presence {
    pub details: String,
    pub state: String,
}

/// The presence for the running session: which workspace and mode, on which
/// model, with the mode's own icon as the small image.
pub fn presence(app: &App) -> Presence {
    let folder = app
        .working_dir()
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| app.working_dir().display().to_string());
    let mode = app.mode.label().to_owned();
    let model = app.backend.model().trim();
    Presence {
        details: clip(&format!("{folder} · {mode} mode")),
        state: clip(&if model.is_empty() {
            mode
        } else {
            format!("{mode} · {model}")
        }),
    }
}

impl Presence {
    fn activity(&self) -> serde_json::Value {
        serde_json::json!({
            "details": self.details,
            "state": self.state,
            "assets": {
                "large_image": LARGE_IMAGE,
                "large_text": LARGE_TEXT,
            },
            "timestamps": { "start": started_ms() },
        })
    }
}

/// When this process started, in milliseconds: Discord draws the elapsed time
/// from it.
fn started_ms() -> u64 {
    static STARTED: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *STARTED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64)
    })
}

/// Two characters at least, 128 at most: Discord's window for both fields.
fn clip(text: &str) -> String {
    let mut text: String = text.trim().chars().take(128).collect();
    // Discord rejects a field shorter than two characters outright.
    while text.chars().count() < 2 {
        text.push(' ');
    }
    text
}

/// The connection as the app sees it: `sync` is called from the event loop and
/// does nothing until the user turns the presence on.
#[derive(Default)]
pub struct Runtime {
    client_id: Option<String>,
    updates: Option<Sender<Presence>>,
    last: Option<(Presence, Instant)>,
}

impl Runtime {
    /// Match the connection to `wanted`: `None` means the feature is off.
    /// Called on every loop pass, so the work is one comparison when nothing
    /// changed.
    pub fn sync(&mut self, wanted: Option<(String, Presence)>) {
        let Some((client_id, presence)) = wanted else {
            if self.client_id.take().is_some() {
                // Dropping the sender ends the worker at its next read.
                self.updates = None;
                self.last = None;
            }
            return;
        };
        if self.client_id.as_deref() != Some(client_id.as_str()) || self.updates.is_none() {
            self.client_id = Some(client_id.clone());
            self.last = None;
            self.updates = Some(spawn_worker(client_id));
        }
        let due = match &self.last {
            Some((sent, at)) => sent != &presence || at.elapsed() >= RESEND_AFTER,
            None => true,
        };
        if !due {
            return;
        }
        if let Some(updates) = &self.updates {
            // A closed channel means the worker gave up for good; the next
            // sync starts a fresh one.
            if updates.send(presence.clone()).is_err() {
                self.updates = None;
                self.client_id = None;
                self.last = None;
                return;
            }
        }
        self.last = Some((presence, Instant::now()));
    }
}

/// One worker thread per connection: handshake, then presence updates until
/// the sender is dropped. A missing or restarted Discord is retried with a
/// pause, so the feature survives Discord starting after yopes.
fn spawn_worker(client_id: String) -> Sender<Presence> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || worker(client_id, rx));
    tx
}

fn worker(client_id: String, updates: Receiver<Presence>) {
    loop {
        let Some(mut connection) = connect() else {
            // No Discord running: wait, but give up as soon as the feature is
            // switched off.
            std::thread::sleep(Duration::from_secs(5));
            if receiver_closed(&updates) {
                return;
            }
            continue;
        };
        if write_frame(
            &mut connection,
            OP_HANDSHAKE,
            &serde_json::json!({ "v": 1, "client_id": client_id }),
        )
        .is_err()
        {
            std::thread::sleep(Duration::from_secs(5));
            continue;
        }
        // One connection: send every update until it fails or the feature is
        // switched off, then reconnect (a failed write means Discord went
        // away, and the next attempt waits for it).
        loop {
            match updates.recv_timeout(Duration::from_secs(2)) {
                Ok(presence) => {
                    if send_activity(&mut connection, &presence).is_err() {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // A ping Discord sent must be answered or it drops us.
                    if !drain(&mut connection) {
                        break;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        std::thread::sleep(Duration::from_secs(5));
        if receiver_closed(&updates) {
            return;
        }
    }
}

fn receiver_closed(updates: &Receiver<Presence>) -> bool {
    matches!(updates.try_recv(), Err(TryRecvError::Disconnected))
}

fn send_activity(connection: &mut Connection, presence: &Presence) -> std::io::Result<()> {
    let frame = serde_json::json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": std::process::id(), "activity": presence.activity() },
        "nonce": format!("{}", started_ms()),
    });
    write_frame(connection, OP_FRAME, &frame)
}

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

fn write_frame(
    connection: &mut Connection,
    opcode: u32,
    payload: &serde_json::Value,
) -> std::io::Result<()> {
    let body = payload.to_string();
    let mut frame = Vec::with_capacity(8 + body.len());
    frame.extend_from_slice(&opcode.to_le_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body.as_bytes());
    connection.write_all(&frame)?;
    connection.flush()
}

/// Read whatever Discord sent and answer its pings. `false` when the
/// connection is gone, which is the signal to reconnect.
fn drain(connection: &mut Connection) -> bool {
    loop {
        let mut header = [0u8; 8];
        if connection.read_exact(&mut header).is_err() {
            return false;
        }
        let opcode = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let mut body = vec![0u8; length];
        if connection.read_exact(&mut body).is_err() {
            return false;
        }
        match opcode {
            OP_PING => {
                let mut frame = Vec::with_capacity(8 + body.len());
                frame.extend_from_slice(&OP_PONG.to_le_bytes());
                frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
                frame.extend_from_slice(&body);
                if connection.write_all(&frame).is_err() || connection.flush().is_err() {
                    return false;
                }
            }
            OP_CLOSE => return false,
            _ => {}
        }
        // `read_exact` blocks for the next frame: the caller polls this on a
        // timeout, so one readable frame is enough per call.
        return true;
    }
}

/// The IPC socket: the first `discord-ipc-N` that accepts a connection.
fn connect() -> Option<Connection> {
    (0..10).find_map(|index| open(index))
}

#[cfg(windows)]
type Connection = std::fs::File;

#[cfg(windows)]
fn open(index: u32) -> Option<Connection> {
    let path = format!(r"\\.\pipe\discord-ipc-{index}");
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .ok()
}

#[cfg(unix)]
type Connection = std::os::unix::net::UnixStream;

#[cfg(unix)]
fn open(index: u32) -> Option<Connection> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    for dir in [base, std::path::PathBuf::from("/tmp")] {
        let path = dir.join(format!("discord-ipc-{index}"));
        if let Ok(stream) = std::os::unix::net::UnixStream::connect(path) {
            return Some(stream);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{Presence, clip};

    #[test]
    fn the_activity_card_carries_the_logo_and_the_mode() {
        let presence = Presence {
            details: "YopesCLI · build mode".to_owned(),
            state: "build · gpt-4o".to_owned(),
        };
        let activity = presence.activity();
        assert_eq!(activity["assets"]["large_image"], "yopes_logo");
        assert_eq!(activity["assets"]["large_text"], "YopesCLI");
        assert_eq!(activity["details"], "YopesCLI · build mode");
        assert_eq!(activity["state"], "build · gpt-4o");
        // Only the one asset the README asks the user to upload: a missing
        // asset key makes Discord refuse the whole card.
        assert!(activity["assets"].get("small_image").is_none());
        assert!(activity["timestamps"]["start"].as_u64().unwrap() > 0);
    }

    #[test]
    fn a_field_is_never_shorter_than_two_or_longer_than_128_chars() {
        assert_eq!(clip("build"), "build");
        // Discord rejects a one-character or empty field outright.
        assert_eq!(clip("x").chars().count(), 2);
        assert_eq!(clip("").chars().count(), 2);
        assert_eq!(clip(&"m".repeat(400)).chars().count(), 128);
    }
}
