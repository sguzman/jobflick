use crate::ipc;
use crate::paths;
use crate::protocol::{Job, Request, Response, State, STOPPING_NOTE};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const REPORT_TAIL_BYTES: u64 = 256 * 1024;
const MAX_COMMAND_BYTES: usize = 128 * 1024;
const CANCEL_POLL: Duration = Duration::from_millis(50);
const TERMINATION_GRACE: Duration = Duration::from_secs(1);

#[derive(Serialize, Deserialize)]
struct Config {
    max_concurrent: usize,
}
impl Default for Config {
    fn default() -> Self {
        Self { max_concurrent: 3 }
    }
}

struct Inner {
    jobs: HashMap<String, Job>,
    pending: VecDeque<String>,
    running: usize,
    cancellation: HashMap<String, Arc<AtomicBool>>,
}
pub struct Manager {
    inner: Mutex<Inner>,
    jobs_dir: PathBuf,
    limit: usize,
}

fn millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn load_config() -> Result<Config> {
    let dir = paths::config_dir();
    paths::private_dir(&dir)?;
    let file = dir.join("config.toml");
    if !file.exists() {
        paths::atomic_private_write(&file, b"max_concurrent = 3\n")?;
    }
    let config: Config = toml::from_str(&fs::read_to_string(file)?)?;
    if !(1..=16).contains(&config.max_concurrent) {
        bail!("max_concurrent must be in the range 1..=16");
    }
    Ok(config)
}

/// Validate Fish syntax without ever running the submitted command.
/// This intentionally does not attempt to classify syntactically valid prose.
fn validate_command(command: &str) -> Result<()> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        bail!("Clipboard command is empty");
    }
    if command.len() > MAX_COMMAND_BYTES {
        bail!("Command exceeds 128 KiB submission limit");
    }
    if trimmed.starts_with("```") || trimmed.ends_with("```") {
        bail!("Clipboard contains Markdown fences; copy only the Fish command");
    }
    let result = Command::new("fish")
        .args(["--no-config", "--no-execute", "--command", command])
        .output()
        .context("run Fish syntax check")?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        let detail = stderr.trim();
        bail!("Invalid Fish syntax: {}", if detail.is_empty() { "syntax check failed" } else { detail });
    }
    Ok(())
}
impl Manager {
    fn load(limit: usize) -> Result<Arc<Self>> {
        let root = paths::data_dir();
        let jobs_dir = root.join("jobs");
        let logs_dir = root.join("logs");
        paths::private_dir(&jobs_dir)?;
        paths::private_dir(&logs_dir)?;
        let mut jobs = HashMap::new();
        let mut queue = Vec::new();
        for entry in fs::read_dir(&jobs_dir)? {
            let entry = entry?;
            if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let contents = fs::read_to_string(entry.path())?;
            let mut job: Job = match serde_json::from_str(&contents) {
                Ok(job) => job,
                Err(error) => {
                    eprintln!("Skipped invalid job {}: {error}", entry.path().display());
                    continue;
                }
            };
            if job.state == State::Running {
                job.state = State::Interrupted;
                job.completed_at = Some(millis());
                job.note = Some("Manager restarted while this job was running; process outcome is unknown".into());
                persist(&jobs_dir, &job)?;
            }
            if job.state == State::Queued {
                queue.push((job.submitted_at, job.id.clone()));
            }
            jobs.insert(job.id.clone(), job);
        }
        queue.sort();
        let manager = Arc::new(Self {
            inner: Mutex::new(Inner {
                jobs,
                pending: queue.into_iter().map(|(_, id)| id).collect(),
                running: 0,
                cancellation: HashMap::new(),
            }),
            jobs_dir,
            limit,
        });
        Ok(manager)
    }

    fn submit(self: &Arc<Self>, command: String) -> Result<Job> {
        validate_command(&command)?;
        let id = Uuid::new_v4().to_string();
        let job = Job {
            id: id.clone(),
            command,
            state: State::Queued,
            submitted_at: millis(),
            started_at: None,
            completed_at: None,
            exit_code: None,
            consumed: false,
            log_path: paths::data_dir().join("logs").join(format!("{id}.log")).display().to_string(),
            note: None,
        };
        {
            let mut inner = self.inner.lock().unwrap();
            persist(&self.jobs_dir, &job)?;
            inner.pending.push_back(id.clone());
            inner.jobs.insert(id, job.clone());
        }
        self.pump();
        let accepted = self.find(&job.id)?;
        if accepted.state == State::Queued {
            notify("Job queued", &notification_label(&accepted));
        }
        Ok(accepted)
    }

    fn pump(self: &Arc<Self>) {
        let mut launches = Vec::new();
        {
            let mut inner = self.inner.lock().unwrap();
            while inner.running < self.limit {
                let Some(id) = inner.pending.pop_front() else { break; };
                let job_to_run = {
                    let Some(job) = inner.jobs.get_mut(&id) else { continue; };
                    if job.state != State::Queued { continue; }
                    // Never start work unless its Running transition is durable.
                    // Otherwise a restart could replay a command that already ran.
                    let mut running = job.clone();
                    running.state = State::Running;
                    running.started_at = Some(millis());
                    match persist(&self.jobs_dir, &running) {
                        Ok(()) => {
                            *job = running.clone();
                            Some(running)
                        }
                        Err(error) => {
                            eprintln!("Cannot persist running job; queue paused: {error:#}");
                            None
                        }
                    }
                };
                let Some(job_to_run) = job_to_run else {
                    // Preserve FIFO order; a future pump may retry after
                    // storage is repaired. Do not consume an execution slot.
                    inner.pending.push_front(id);
                    break;
                };
                inner.running += 1;
                let cancelled = Arc::new(AtomicBool::new(false));
                inner.cancellation.insert(id, Arc::clone(&cancelled));
                launches.push((job_to_run, cancelled));
            }
        }
        for (job, cancelled) in launches {
            let manager = Arc::clone(self);
            thread::spawn(move || {
                notify("Job started", &notification_label(&job));
                let (state, code, note) = execute(&job, &cancelled);
                manager.finish(&job.id, state, code, note);
            });
        }
    }

    fn finish(self: &Arc<Self>, id: &str, state: State, code: Option<i32>, note: Option<String>) {
        let finished = {
            let mut inner = self.inner.lock().unwrap();
            let cancelled = inner.cancellation.remove(id)
                .is_some_and(|flag| flag.load(Ordering::Acquire));
            let result = if let Some(job) = inner.jobs.get_mut(id) {
                // A cancellation may be accepted after the child exits but
                // before its final status is persisted. Keep the API coherent.
                job.state = if cancelled { State::Cancelled } else { state };
                job.exit_code = code;
                job.completed_at = Some(millis());
                job.note = if cancelled && state != State::Cancelled {
                    Some("Cancellation requested at completion; command may already have exited".into())
                } else {
                    note
                };
                if let Err(error) = persist(&self.jobs_dir, job) {
                    eprintln!("Persist completed job failed: {error:#}");
                }
                Some(job.clone())
            } else {
                None
            };
            inner.running = inner.running.saturating_sub(1);
            result
        };
        if let Some(job) = finished {
            notify(
                match job.state {
                    State::Succeeded => "Job completed",
                    State::Cancelled => "Job cancelled",
                    _ => "Job failed",
                },
                &notification_label(&job),
            );
        }
        self.pump();
    }

    fn list(&self, all: bool) -> Vec<Job> {
        let inner = self.inner.lock().unwrap();
        let mut jobs: Vec<_> = inner.jobs.values()
            .filter(|j| all || !j.consumed)
            .cloned()
            .collect();
        jobs.sort_by(|a, b| b.submitted_at.cmp(&a.submitted_at).then_with(|| b.id.cmp(&a.id)));
        jobs
    }

    fn find(&self, id: &str) -> Result<Job> {
        let inner = self.inner.lock().unwrap();
        let mut matching = inner.jobs.values().filter(|job| job.id.starts_with(id));
        let job = matching.next().context("No job matches that ID")?.clone();
        if matching.next().is_some() {
            bail!("Ambiguous job ID prefix");
        }
        Ok(job)
    }

    fn consume(&self, id: &str, consumed: bool) -> Result<Job> {
        let mut inner = self.inner.lock().unwrap();
        let job_id = unique_id(&inner, id)?;
        let job = inner.jobs.get_mut(&job_id).unwrap();
        if !job.state.finished() {
            bail!("Only finished jobs can be consumed or restored");
        }
        let mut updated = job.clone();
        updated.consumed = consumed;
        persist(&self.jobs_dir, &updated)?;
        *job = updated.clone();
        Ok(updated)
    }

    fn cancel(&self, id: &str) -> Result<Job> {
        let mut inner = self.inner.lock().unwrap();
        let job_id = unique_id(&inner, id)?;
        let cancellation = inner.cancellation.get(&job_id).cloned();
        let job = inner.jobs.get_mut(&job_id).unwrap();
        match job.state {
            State::Queued => {
                let mut updated = job.clone();
                updated.state = State::Cancelled;
                updated.completed_at = Some(millis());
                updated.note = Some("Cancelled before execution".into());
                persist(&self.jobs_dir, &updated)?;
                *job = updated.clone();
                inner.pending.retain(|pending_id| pending_id != &job_id);
                Ok(updated)
            }
            State::Running => {
                let flag = cancellation.context("Running job has no cancellation control")?;
                if !flag.load(Ordering::Acquire) {
                    // Persist intent before acknowledging. Worker owns all
                    // process-group signals; the IPC thread never kills PIDs.
                    let mut updated = job.clone();
                    updated.note = Some(STOPPING_NOTE.into());
                    persist(&self.jobs_dir, &updated)?;
                    *job = updated;
                    flag.store(true, Ordering::Release);
                }
                Ok(job.clone())
            }
            State::Cancelled => Ok(job.clone()),
            _ => bail!("Job has already finished; cannot stop it"),
        }
    }
}

fn unique_id(inner: &Inner, prefix: &str) -> Result<String> {
    let mut matches = inner.jobs.keys().filter(|id| id.starts_with(prefix));
    let id = matches.next().context("No matching job")?.clone();
    if matches.next().is_some() {
        bail!("Ambiguous job ID prefix");
    }
    Ok(id)
}

fn persist(dir: &PathBuf, job: &Job) -> Result<()> {
    let data = serde_json::to_vec_pretty(job)?;
    paths::atomic_private_write(&dir.join(format!("{}.json", job.id)), &data)
}

/// Each Fish command is a process-group leader; its usual child processes
/// inherit the group. Only this worker signals its own child's process group,
/// never the daemon's group or an untracked PID.
fn signal_job_group(child: &Child, signal: libc::c_int) -> Result<()> {
    let pgid = child.id() as libc::pid_t;
    if pgid <= 0 {
        bail!("Refusing to signal invalid process group");
    }
    let result = unsafe { libc::kill(-pgid, signal) };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error).context("signal job process group");
        }
    }
    Ok(())
}

fn stop_job_group(child: &mut Child) -> Result<Option<i32>> {
    let terminate = signal_job_group(child, libc::SIGTERM);
    // Do not reap the process-group leader until after SIGKILL: retaining
    // its PID prevents us from accidentally addressing a reused group ID.
    thread::sleep(TERMINATION_GRACE);
    let force = signal_job_group(child, libc::SIGKILL);
    let status = child.wait().context("reap cancelled Fish process")?;
    terminate?;
    force?;
    Ok(status.code())
}

fn execute(job: &Job, cancelled: &AtomicBool) -> (State, Option<i32>, Option<String>) {
    let result = (|| -> Result<(Option<i32>, bool)> {
        if cancelled.load(Ordering::Acquire) {
            return Ok((None, true));
        }
        let file = OpenOptions::new()
            .write(true).create_new(true).mode(0o600)
            .open(&job.log_path)
            .with_context(|| format!("open {}", job.log_path))?;
        let stderr = file.try_clone()?;
        let mut child = Command::new("fish")
            .arg("-c")
            .arg(&job.command)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::from(stderr))
            .spawn()
            .context("spawn fish command")?;
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Ok((stop_job_group(&mut child)?, true));
            }
            if let Some(status) = child.try_wait().context("wait for fish command")? {
                return Ok((status.code(), false));
            }
            thread::sleep(CANCEL_POLL);
        }
    })();
    match result {
        Ok((code, true)) => (State::Cancelled, code, Some("Stopped by user".into())),
        Ok((Some(0), false)) => (State::Succeeded, Some(0), None),
        Ok((code, false)) => (State::Failed, code, None),
        Err(error) => {
            let detail = format!("{error:#}");
            let _ = OpenOptions::new().append(true).open(&job.log_path)
                .and_then(|mut f| writeln!(f, "\nJobflick worker error: {detail}"));
            (State::Failed, None, Some(detail))
        }
    }
}

fn report(job: &Job) -> Result<String> {
    if !job.state.finished() {
        bail!("Job has not finished yet");
    }
    let mut body = String::new();
    let mut truncated = false;
    if let Ok(mut file) = File::open(&job.log_path) {
        let length = file.metadata()?.len();
        if length > REPORT_TAIL_BYTES {
            file.seek(SeekFrom::Start(length - REPORT_TAIL_BYTES))?;
            truncated = true;
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        body = String::from_utf8_lossy(&bytes).into_owned();
    }
    let code = job.exit_code.map(|x| x.to_string()).unwrap_or_else(|| "n/a".into());
    let note = job.note.as_deref().unwrap_or("none");
    Ok(format!(
        "Jobflick execution report\nJob: {}\nStatus: {}\nExit code: {}\nSubmitted (unix ms): {}\nStarted (unix ms): {}\nCompleted (unix ms): {}\nNote: {}\n\nCommand:\n{}\n\n{}Output:\n{}\n\nSaved log: {}\n",
        job.id, job.state.label(), code, job.submitted_at,
        job.started_at.map(|x| x.to_string()).unwrap_or_else(|| "n/a".into()),
        job.completed_at.map(|x| x.to_string()).unwrap_or_else(|| "n/a".into()),
        note, job.command,
        if truncated { "[Output truncated to last 256 KiB; see saved log]\n" } else { "" },
        body, job.log_path
    ))
}

// Notifications may appear in desktop history or on a locked screen.
// Do not put command text, arguments, or job output on that surface.
fn notification_label(job: &Job) -> String {
    format!("Job {}", job.short_id())
}

fn notify(title: &str, message: &str) {
    let _ = Command::new("notify-send")
        .arg("-a").arg("Jobflick")
        .arg(title).arg(message)
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status();
}

fn handle(manager: &Arc<Manager>, request: Request) -> Response {
    let operation = || -> Result<Response> {
        match request.action.as_str() {
            "submit" => {
                let job = manager.submit(request.command.context("Missing command")?)?;
                Ok(Response { job: Some(job), ..Response::success("Job queued") })
            }
            "list" => Ok(Response {
                jobs: manager.list(request.all),
                ..Response::success("Job list")
            }),
            "show" => {
                let job = manager.find(&request.id.context("Missing job ID")?)?;
                let contents = report(&job)?;
                Ok(Response { job: Some(job), report: Some(contents), ..Response::success("Report ready") })
            }
            "consume" | "restore" => {
                let job = manager.consume(&request.id.context("Missing job ID")?, request.action == "consume")?;
                Ok(Response { job: Some(job), ..Response::success("Updated inbox") })
            }
            "cancel" => {
                let job = manager.cancel(&request.id.context("Missing job ID")?)?;
                let message = match job.state {
                    State::Running => "Stop requested; waiting for process termination",
                    State::Cancelled => "Job cancelled",
                    _ => "Cancellation request accepted",
                };
                Ok(Response { job: Some(job), ..Response::success(message) })
            }
            _ => bail!("Unknown action"),
        }
    };
    match operation() {
        Ok(response) => response,
        Err(error) => Response::failure(format!("{error:#}")),
    }
}

fn serve_client(mut stream: UnixStream, manager: Arc<Manager>) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;
    let reply = match ipc::read_frame(
        BufReader::new(stream.try_clone()?),
        ipc::MAX_REQUEST_FRAME,
    ) {
        Ok(line) => match serde_json::from_slice::<Request>(&line) {
            Ok(request) => handle(&manager, request),
            Err(error) => Response::failure(format!("Invalid request: {error}")),
        },
        Err(error) => Response::failure(format!("Invalid IPC request: {error:#}")),
    };
    serde_json::to_writer(&mut stream, &reply)?;
    stream.write_all(b"\n")?;
    let _ = stream.shutdown(Shutdown::Write);
    Ok(())
}

pub fn daemon() -> Result<()> {
    let config = load_config()?;
    let data = paths::data_dir();
    paths::private_dir(&data)?;
    let lock_path = data.join("daemon.lock");
    let lock = OpenOptions::new().write(true).create(true).mode(0o600).open(lock_path)?;
    // Keep this descriptor open for the entire daemon lifetime.
    let locked = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if locked != 0 {
        bail!("Jobflick daemon is already running");
    }
    paths::private_dir(&paths::runtime_dir())?;
    let socket_path = paths::socket();
    if socket_path.exists() {
        if UnixStream::connect(&socket_path).is_ok() {
            bail!("Jobflick daemon already serves this socket");
        }
        fs::remove_file(&socket_path)?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
    let manager = Manager::load(config.max_concurrent)?;
    manager.pump();
    println!("Jobflick daemon listening at {}", socket_path.display());
    for client in listener.incoming() {
        match client {
            Ok(stream) => {
                let manager = Arc::clone(&manager);
                thread::spawn(move || {
                    if let Err(error) = serve_client(stream, manager) {
                        eprintln!("Client handling failed: {error:#}");
                    }
                });
            }
            Err(error) => eprintln!("Socket accept failed: {error}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_is_rejected_for_unfinished_jobs() {
        let job = Job {
            id: "test".into(),
            command: "echo hi".into(),
            state: State::Queued,
            submitted_at: 0,
            started_at: None,
            completed_at: None,
            exit_code: None,
            consumed: false,
            log_path: "/dev/null".into(),
            note: None,
        };
        assert!(report(&job).is_err());
    }

    #[test]
    fn reject_empty_and_fenced_clipboard() {
        assert!(validate_command("   ").is_err());
        assert!(validate_command("```fish\necho ok\n```").is_err());
        assert!(validate_command(&"x".repeat(MAX_COMMAND_BYTES + 1)).is_err());
    }

    #[test]
    fn fish_syntax_preflight_does_not_execute() {
        assert!(validate_command("cd /tmp; and echo ready").is_ok());
        assert!(validate_command("echo 'unterminated").is_err());
    }
    #[test]
    fn cancelling_before_spawn_never_runs_command() {
        let path = std::env::temp_dir().join(format!("jobflick-cancel-{}.log", Uuid::new_v4()));
        let job = Job {
            id: Uuid::new_v4().to_string(),
            command: "printf SHOULD_NOT_RUN".into(),
            state: State::Running,
            submitted_at: 0,
            started_at: Some(1),
            completed_at: None,
            exit_code: None,
            consumed: false,
            log_path: path.display().to_string(),
            note: None,
        };
        let cancellation = AtomicBool::new(true);
        let (state, exit_code, note) = execute(&job, &cancellation);
        assert_eq!(state, State::Cancelled);
        assert_eq!(exit_code, None);
        assert!(note.is_some());
        assert!(!path.exists(), "pre-spawn cancellation must not create a log");
    }

    fn manager_with_unwritable_job_directory(state: State) -> (Arc<Manager>, PathBuf, String) {
        // A regular file is deterministically not a directory, even in a
        // privileged test runner; permission-bit tests would be unreliable.
        let blocked_path = std::env::temp_dir()
            .join(format!("jobflick-storage-blocked-{}", Uuid::new_v4()));
        fs::write(&blocked_path, b"not a directory").unwrap();
        let id = Uuid::new_v4().to_string();
        let job = Job {
            id: id.clone(),
            command: "echo no-side-effect".into(),
            state,
            submitted_at: 1,
            started_at: None,
            completed_at: None,
            exit_code: None,
            consumed: false,
            log_path: "/dev/null".into(),
            note: None,
        };
        let manager = Arc::new(Manager {
            inner: Mutex::new(Inner {
                jobs: HashMap::from([(id.clone(), job)]),
                pending: if state == State::Queued {
                    VecDeque::from([id.clone()])
                } else {
                    VecDeque::new()
                },
                running: 0,
                cancellation: HashMap::new(),
            }),
            jobs_dir: blocked_path.clone(),
            limit: 1,
        });
        (manager, blocked_path, id)
    }

    #[test]
    fn failed_persist_does_not_launch_or_dequeue_work() {
        let (manager, blocked_path, id) = manager_with_unwritable_job_directory(State::Queued);
        manager.pump();
        {
            let inner = manager.inner.lock().unwrap();
            assert_eq!(inner.running, 0);
            assert_eq!(inner.pending.front(), Some(&id));
            assert_eq!(inner.jobs[&id].state, State::Queued);
            assert!(inner.cancellation.is_empty());
        }
        fs::remove_file(blocked_path).unwrap();
    }

    #[test]
    fn failed_cancellation_persist_does_not_mutate_queue() {
        let (manager, blocked_path, id) = manager_with_unwritable_job_directory(State::Queued);
        assert!(manager.cancel(&id).is_err());
        {
            let inner = manager.inner.lock().unwrap();
            assert_eq!(inner.jobs[&id].state, State::Queued);
            assert_eq!(inner.pending.front(), Some(&id));
        }
        fs::remove_file(blocked_path).unwrap();
    }

    #[test]
    fn failed_consume_persist_leaves_result_visible() {
        let (manager, blocked_path, id) = manager_with_unwritable_job_directory(State::Succeeded);
        assert!(manager.consume(&id, true).is_err());
        assert!(!manager.inner.lock().unwrap().jobs[&id].consumed);
        fs::remove_file(blocked_path).unwrap();
    }

    #[test]
    fn desktop_notifications_never_include_clipboard_commands() {
        let job = Job {
            id: "abcdef12-3456-7890".into(),
            command: "do-sensitive-work --password=top-secret".into(),
            state: State::Running,
            submitted_at: 1,
            started_at: Some(2),
            completed_at: None,
            exit_code: None,
            consumed: false,
            log_path: "/dev/null".into(),
            note: None,
        };
        let label = notification_label(&job);
        assert_eq!(label, "Job abcdef12");
        assert!(!label.contains("password"));
        assert!(!label.contains("top-secret"));
    }

    #[test]
    fn state_limit_validity() {
        assert!(State::Succeeded.finished());
        assert!(State::Interrupted.finished());
        assert!(!State::Running.finished());
        assert!(!State::Queued.finished());
    }
}
