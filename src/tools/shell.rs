use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    process::{Child as ProcessChild, Command as ProcessCommand, Stdio},
    sync::{
        Mutex, MutexGuard, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use super::*;

#[derive(Deserialize)]
struct RunCommandArgs {
    command: String,
    #[serde(default)]
    timeout_seconds: Option<u64>,
    #[serde(default)]
    shell: Option<Shell>,
}

pub(super) fn run_command(arguments: &str) -> Result<String> {
    let args: RunCommandArgs = parse(arguments)?;
    let command = args.command.trim();
    if command.is_empty() {
        bail!("command cannot be empty");
    }
    let timeout = args
        .timeout_seconds
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
        .clamp(1, MAX_TIMEOUT_SECONDS);

    let log_path = temporary_log_path();
    let file =
        fs::File::create(&log_path).with_context(|| format!("creating {}", log_path.display()))?;
    let stderr = file
        .try_clone()
        .with_context(|| format!("opening {}", log_path.display()))?;

    let mut process = shell_process(command, args.shell)?;
    process
        .current_dir(workspace())
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(stderr));

    let mut child = process
        .spawn()
        .with_context(|| format!("running `{command}`"))?;
    // Registered under this session's id, so an Esc interrupt from another
    // tab leaves it running.
    let owner = current_owner();
    spawned_pids().entry(owner).or_default().push(child.id());
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait().context("waiting for the command")? {
            break Some(status);
        }
        if started.elapsed() >= Duration::from_secs(timeout) {
            kill_process_tree(&mut child);
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(25));
    };

    // The command is over, however it ended: stop tracking its pid.
    let pid = child.id();
    if let Some(registered) = spawned_pids().get_mut(&owner) {
        registered.retain(|tracked| *tracked != pid);
    }

    let output = read_log(&log_path);
    let _ = fs::remove_file(&log_path);
    let mut result = String::new();
    if timed_out {
        result.push_str(&format!("Command timed out after {timeout}s.\n"));
    } else if let Some(status) = status {
        if !status.success() {
            result.push_str(&format!("Exit code: {}.\n", status.code().unwrap_or(-1)));
        }
    }
    if output.is_empty() {
        result.push_str("(no output)");
    } else {
        result.push_str(&output);
    }
    Ok(truncate_output(result))
}

/// One background command: where it writes, when it started, and how to stop
/// it. `exit` is set the first time its output is read after finishing.
struct BashJob {
    started: Instant,
    timeout: u64,
    log_path: PathBuf,
    child: Option<ProcessChild>,
    exit: Option<i32>,
    /// The session id that started the job: the completion notice routes to
    /// its own conversation.
    owner: u64,
}

impl Drop for BashJob {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.log_path);
    }
}

/// How long one hook may run before it is killed: long enough for `cargo fmt`
/// on a big tree, short enough that a hung hook fails the turn instead of
/// hanging the interface.
const HOOK_TIMEOUT_SECONDS: u64 = 60;

const MAX_BASH_JOBS: usize = 16;
static BASH_JOBS: OnceLock<Mutex<HashMap<u32, BashJob>>> = OnceLock::new();
static JOB_SEQ: AtomicU32 = AtomicU32::new(1);

/// The process ids of foreground commands still running, keyed by the
/// session id that owns them: a `spawn_blocking` tool call cannot be
/// cancelled from the event loop, so the Esc interrupt kills the processes
/// through this registry — and only the interrupting session's own entries,
/// so one tab's Esc never kills another tab's running command.
static SPAWNED_PIDS: OnceLock<Mutex<HashMap<u64, Vec<u32>>>> = OnceLock::new();

fn spawned_pids() -> MutexGuard<'static, HashMap<u64, Vec<u32>>> {
    SPAWNED_PIDS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Kill every foreground command still running for one session. Returns how
/// many were interrupted.
pub(crate) fn kill_commands_for(session_id: u64) -> usize {
    let pids: Vec<u32> = spawned_pids().remove(&session_id).unwrap_or_default();
    kill_pids(pids)
}

/// Kill every foreground command still running, whatever session owns it:
/// process shutdown only, so no tool leaves an orphan behind.
pub(crate) fn kill_spawned_commands() -> usize {
    let pids: Vec<u32> = std::mem::take(&mut *spawned_pids())
        .into_values()
        .flatten()
        .collect();
    kill_pids(pids)
}

fn kill_pids(pids: Vec<u32>) -> usize {
    let count = pids.len();
    for pid in pids {
        #[cfg(windows)]
        {
            let _ = ProcessCommand::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output();
        }
        #[cfg(not(windows))]
        {
            let _ = ProcessCommand::new("kill")
                .args(["-9", &pid.to_string()])
                .output();
        }
    }
    count
}

fn bash_jobs() -> MutexGuard<'static, HashMap<u32, BashJob>> {
    BASH_JOBS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The cached bash executable. On Windows it arrives with Git for Windows;
/// without it the shell tools refuse to run and `run_command` stays on cmd.
static BASH_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The interpreter a command runs through. The model picks one per call;
/// without a choice the default applies, so older calls keep working.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Shell {
    Bash,
    #[serde(alias = "pwsh")]
    Powershell,
    Cmd,
    Sh,
}

pub(super) fn bash_path() -> Option<&'static PathBuf> {
    BASH_PATH.get_or_init(detect_bash).as_ref()
}

fn detect_bash() -> Option<PathBuf> {
    if !cfg!(windows) {
        return Some(PathBuf::from("bash"));
    }
    let probe = ProcessCommand::new("where").arg("git").output().ok()?;
    if !probe.status.success() {
        return None;
    }
    let lossy = String::from_utf8_lossy(&probe.stdout);
    let git = lossy.lines().next()?.trim();
    // git.exe usually lives in …\cmd or …\bin; bash.exe in …\bin or …\usr\bin.
    let root = PathBuf::from(git).parent()?.to_path_buf();
    let step_up = root.parent().map(|root| root.join("bin").join("bash.exe"));
    vec![
        root.join("bash.exe"),
        root.join("bin").join("bash.exe"),
        root.join("usr").join("bin").join("bash.exe"),
    ]
    .into_iter()
    .chain(step_up)
    .find(|candidate| candidate.is_file())
}

/// The process wrapper every command runs through: the interpreter the call
/// asked for, otherwise git-bash when it is installed, otherwise the platform
/// default. That is the graceful disable.
fn shell_process(command: &str, shell: Option<Shell>) -> Result<ProcessCommand> {
    let Some(shell) = shell else {
        return Ok(default_shell(command));
    };
    let (program, flags): (&std::ffi::OsStr, &[&str]) = match shell {
        Shell::Bash => (
            bash_path()
                .context(
                    "bash is not available on this machine; install Git for Windows \
                     and restart yopes, or pass shell \"powershell\" or \"cmd\"",
                )?
                .as_os_str(),
            &["-lc"],
        ),
        Shell::Powershell => (
            std::ffi::OsStr::new("powershell"),
            &["-NoProfile", "-NonInteractive", "-Command"],
        ),
        Shell::Cmd => (std::ffi::OsStr::new("cmd"), &["/C"]),
        Shell::Sh => (std::ffi::OsStr::new("sh"), &["-c"]),
    };
    let mut process = ProcessCommand::new(program);
    process.args(flags).arg(command);
    Ok(process)
}

/// The shell a command runs through when the call names none.
fn default_shell(command: &str) -> ProcessCommand {
    if let Some(bash) = bash_path() {
        let mut process = ProcessCommand::new(bash);
        process.args(["-lc", command]);
        return process;
    }
    if cfg!(windows) {
        let mut process = ProcessCommand::new("cmd");
        process.args(["/C", command]);
        process
    } else {
        let mut process = ProcessCommand::new("sh");
        process.args(["-c", command]);
        process
    }
}

#[derive(Deserialize)]
struct BashBackgroundArgs {
    command: String,
    #[serde(default)]
    timeout_seconds: Option<u64>,
    #[serde(default)]
    shell: Option<Shell>,
}

#[derive(Deserialize)]
struct BashJobArgs {
    job_id: u32,
}

pub(super) fn bash_background(arguments: &str) -> Result<String> {
    let args: BashBackgroundArgs = parse(arguments)?;
    let command = args.command.trim();
    if command.is_empty() {
        bail!("command cannot be empty");
    }
    let timeout = args
        .timeout_seconds
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
        .clamp(1, MAX_TIMEOUT_SECONDS);
    let mut jobs = bash_jobs();
    // Finished jobs forget themselves on read, but those the model
    // never reads again stay; drop them here so the cap cannot be
    // exhausted by finished work.
    jobs.retain(|_, job| job.child.is_some());
    if jobs.len() >= MAX_BASH_JOBS {
        bail!(
            "too many bash jobs are waiting or running ({}, the limit is \
             {MAX_BASH_JOBS}); read their output with bash_output first",
            jobs.len()
        );
    }
    let id = JOB_SEQ.fetch_add(1, Ordering::Relaxed);
    let log_path = temporary_log_path();
    let file =
        fs::File::create(&log_path).with_context(|| format!("creating {}", log_path.display()))?;
    let stderr = file
        .try_clone()
        .with_context(|| format!("opening {}", log_path.display()))?;
    let mut process = shell_process(command, Some(args.shell.unwrap_or(Shell::Bash)))?;
    let child = process
        .current_dir(workspace())
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("running `{command}`"))?;
    jobs.insert(
        id,
        BashJob {
            started: Instant::now(),
            timeout,
            log_path,
            child: Some(child),
            exit: None,
            owner: current_owner(),
        },
    );
    Ok(format!(
        "Started bash job {id} (timeout {timeout}s). Call bash_output with job id {id} \
         to read it, and bash_kill to stop it."
    ))
}

pub(super) fn bash_output(arguments: &str) -> Result<String> {
    let args: BashJobArgs = parse(arguments)?;
    let mut jobs = bash_jobs();
    let Some(job) = jobs.get_mut(&args.job_id) else {
        bail!(
            "no bash job with id {}; it was never started or was already read",
            args.job_id
        );
    };
    // Ownership of the child moves through this block so the job can finish
    // between calls; on kill and exit the child is consumed and dropped.
    let mut held = job.child.take();
    if let Some(child) = held.as_mut() {
        match child.try_wait().context("waiting for the bash job")? {
            Some(status) => {
                job.exit = Some(status.code().unwrap_or(-1));
                held = None;
            }
            None if job.started.elapsed() >= Duration::from_secs(job.timeout) => {
                kill_process_tree(child);
                job.exit = Some(-1);
                held = None;
            }
            None => {}
        }
    }
    if job.exit.is_none() {
        job.child = held;
    }
    let mut result = String::new();
    match job.exit {
        Some(exit) => result.push_str(&format!("Exit code: {exit}.\n")),
        None => result.push_str("Still running.\n"),
    }
    let output = read_log(&job.log_path);
    if output.is_empty() {
        if job.child.is_some() {
            result.push_str("(no output yet)");
        } else {
            result.push_str("(no output)");
        }
    } else {
        result.push_str(&output);
    }
    // A job whose child is gone and whose exit was read frees its slot.
    if job.exit.is_some() && job.child.is_none() {
        jobs.remove(&args.job_id);
    }
    Ok(truncate_output(result))
}

pub(super) fn bash_kill(arguments: &str) -> Result<String> {
    let args: BashJobArgs = parse(arguments)?;
    let mut jobs = bash_jobs();
    let Some(job) = jobs.get_mut(&args.job_id) else {
        bail!(
            "no bash job with id {}; it was never started or was already read",
            args.job_id
        );
    };
    let Some(mut child) = job.child.take() else {
        return Ok(format!("Bash job {} already finished.", args.job_id));
    };
    kill_process_tree(&mut child);
    job.child = None;
    job.exit = Some(-1);
    Ok(format!(
        "Killed bash job {}. It reported no exit code.",
        args.job_id
    ))
}

/// Run one hook command in `workspace`, reporting whether it succeeded and
/// what it printed. A hook that hangs is killed at the timeout, so a bad hook
/// cannot wedge the turn it runs inside.
pub(crate) fn run_hook(command: &str, workspace: &Path) -> (bool, String) {
    if command.trim().is_empty() {
        return (true, String::new());
    }
    let log_path = temporary_log_path();
    let Ok(file) = fs::File::create(&log_path) else {
        return (false, "hooks: could not create a log file".to_owned());
    };
    let Ok(stderr) = file.try_clone() else {
        return (false, "hooks: could not open a log file".to_owned());
    };
    let mut process = match shell_process(command, None) {
        Ok(process) => process,
        Err(error) => {
            let _ = fs::remove_file(&log_path);
            return (false, format!("hooks: {error}"));
        }
    };
    process
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(stderr));
    let Ok(mut child) = process.spawn() else {
        let _ = fs::remove_file(&log_path);
        return (
            false,
            format!("hooks: could not start `{}`", command.trim()),
        );
    };
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if started.elapsed() >= Duration::from_secs(HOOK_TIMEOUT_SECONDS) {
            kill_process_tree(&mut child);
            timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(25));
    };
    let mut output = read_log(&log_path);
    let _ = fs::remove_file(&log_path);
    if timed_out {
        output.push_str(&format!("\nhook timed out after {HOOK_TIMEOUT_SECONDS}s"));
    }
    let ok = status.is_some_and(|status| status.success()) && !timed_out;
    (ok, truncate_output(output))
}

fn kill_process_tree(child: &mut ProcessChild) {
    #[cfg(windows)]
    {
        let pid = child.id();
        let _ = ProcessCommand::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// One finished background job, delivered to the event loop: the exit code
/// and the tail of its log, so the owning conversation learns the outcome
/// without ever polling `bash_output`.
pub(crate) struct BashNotice {
    pub(crate) job_id: u32,
    pub(crate) owner: u64,
    pub(crate) output: String,
}

/// How much of a finished job's log enters the notice: a huge log must not
/// flood the conversation; the tail carries the outcome.
const NOTICE_TAIL_BYTES: usize = 8192;

/// Scan the running bash jobs once: finish the ones that exited (or timed
/// out) and return their notices. Called from a watcher task every 500ms —
/// no model turn is ever spent polling.
pub(crate) fn poll_bash_jobs() -> Vec<BashNotice> {
    let mut jobs = bash_jobs();
    let ids: Vec<u32> = jobs.keys().copied().collect();
    let mut notices = Vec::new();
    for id in ids {
        let Some(job) = jobs.get_mut(&id) else {
            continue;
        };
        let mut held = job.child.take();
        let Some(child) = held.as_mut() else {
            continue;
        };
        let Ok(status) = child.try_wait() else {
            job.child = held;
            continue;
        };
        let (exit, timed_out) = match status {
            Some(status) => (status.code().unwrap_or(-1), false),
            None => {
                // Still running: enforce the timeout here, so a hung job
                // cannot hold a child forever when nobody polls.
                if job.started.elapsed() < Duration::from_secs(job.timeout) {
                    job.child = held;
                    continue;
                }
                kill_process_tree(child);
                (-1, true)
            }
        };
        job.exit = Some(exit);
        let owner = job.owner;
        let timeout = job.timeout;
        let log = read_log(&job.log_path);
        let tail = if log.len() > NOTICE_TAIL_BYTES {
            let cut = log.len() - NOTICE_TAIL_BYTES;
            let cut = (0..=cut)
                .rev()
                .find(|index| log.is_char_boundary(*index))
                .unwrap_or(cut);
            format!("… earlier output omitted\n{}", &log[cut..])
        } else {
            log
        };
        let output = if timed_out {
            format!("bash job {id} timed out after {timeout}s.\n{tail}")
        } else {
            format!("bash job {id} finished (exit {exit}):\n{tail}")
        };
        // The job forgets itself: its Drop removes the log file.
        jobs.remove(&id);
        notices.push(BashNotice {
            job_id: id,
            owner,
            output,
        });
    }
    notices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_argument_picks_the_interpreter() {
        let pwsh: RunCommandArgs = parse(r#"{"command":"echo hi","shell":"pwsh"}"#).unwrap();
        assert_eq!(pwsh.shell, Some(Shell::Powershell));
        let build = shell_process("echo hi", pwsh.shell).unwrap();
        assert_eq!(build.get_program(), std::ffi::OsStr::new("powershell"));

        let cmd: RunCommandArgs = parse(r#"{"command":"dir","shell":"cmd"}"#).unwrap();
        let build = shell_process("dir", cmd.shell).unwrap();
        assert_eq!(build.get_program(), std::ffi::OsStr::new("cmd"));
        let argv: Vec<_> = build.get_args().collect();
        assert_eq!(argv, ["/C", "dir"]);
    }

    #[cfg(windows)]
    #[test]
    fn run_command_runs_through_the_named_shell() {
        let out = run_command(r#"{"command":"echo cmd-here","shell":"cmd"}"#).unwrap();
        assert!(out.contains("cmd-here"), "{out}");
        let out =
            run_command(r#"{"command":"Write-Output ps-here","shell":"powershell"}"#).unwrap();
        assert!(out.contains("ps-here"), "{out}");
    }
}
