//! Talking to running subagents: corrections deliver into their conversation
//! so the main agent can steer a run mid-flight, and stop flags end a run
//! cooperatively — it answers with what it has instead of being killed.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Result, bail};
use serde::Deserialize;
use tokio::sync::mpsc;

use super::parse;

/// One running subagent: where corrections land and how it is asked to stop.
struct SubagentRun {
    corrections: mpsc::UnboundedSender<String>,
    stop: Arc<AtomicBool>,
}

static SUBAGENT_RUNS: OnceLock<Mutex<HashMap<u64, SubagentRun>>> = OnceLock::new();

fn runs() -> MutexGuard<'static, HashMap<u64, SubagentRun>> {
    SUBAGENT_RUNS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Register one running subagent: returns the stop flag its task watches.
pub(crate) fn register_subagent_run(
    id: u64,
    corrections: mpsc::UnboundedSender<String>,
) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    runs().insert(
        id,
        SubagentRun {
            corrections,
            stop: stop.clone(),
        },
    );
    stop
}

/// Forget one finished subagent: corrections and stops answer so from here.
pub(crate) fn unregister_subagent_run(id: u64) {
    runs().remove(&id);
}

#[derive(Deserialize)]
struct SubagentMessageArgs {
    run: u64,
    message: String,
}

/// A correction from the main agent: delivered into the running subagent's
/// conversation as a user message, which continues the run and answers it.
pub(super) fn subagent_message(arguments: &str) -> Result<String> {
    let args: SubagentMessageArgs = parse(arguments)?;
    let message = args.message.trim();
    if message.is_empty() {
        bail!("message cannot be empty");
    }
    let Some(run) = runs().get_mut(&args.run) else {
        bail!(
            "no running subagent with id {}; it finished or was stopped, so delegate again \
             with the correction in the task",
            args.run
        );
    };
    run.corrections.send(message.to_owned()).map_err(|_| {
        anyhow::anyhow!("subagent {} stopped before the correction landed", args.run)
    })?;
    Ok(format!(
        "Correction delivered to subagent {}; it continues and answers within its run.",
        args.run
    ))
}

#[derive(Deserialize)]
struct SubagentStopArgs {
    run: u64,
}

/// Ask one running subagent to stop: it answers with what it has instead of
/// being killed mid-work.
pub(super) fn subagent_stop(arguments: &str) -> Result<String> {
    let args: SubagentStopArgs = parse(arguments)?;
    let Some(run) = runs().get_mut(&args.run) else {
        bail!("no running subagent with id {}", args.run);
    };
    run.stop.store(true, Ordering::Relaxed);
    Ok(format!(
        "Stop requested for subagent {}; it answers with what it has.",
        args.run
    ))
}
