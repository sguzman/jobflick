use crate::paths;
use crate::protocol::{Job, Request, Response, State};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const REPORT_TAIL_BYTES: u64 = 256 * 1024;
const MAX_COMMAND_BYTES: usize = 128 * 1024;

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
            }),
            jobs_dir,
            limit,
        });
        Ok(manager)
    }

    fn submit(self: &Arc<Self>, command: String) -> Result<Job> {
        if command.trim().is_empty() {
            bail!("Clipboard command is empty");
        }
        if command.len() > MAX_COMMAND_BYTES {
            bail!("Command exceeds 128 KiB submission limit");
        }
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
        notify("Job queued", &job.summary());
        self.pump();
        Ok(job)
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
                    job.state = State::Running;
                    job.started_at = Some(millis());
                    if let Err(error) = persist(&self.jobs_dir, job) {
                        eprintln!("Persist running job failed: {error:#}");
                    }
                    job.clone()
                };
                inner.running += 1;
                launches.push(job_to_run);
            }
        }
        for job in launches {
            let manager = Arc::clone(self);
            thread::spawn(move || {
                notify("Job started", &job.summary());
                let (state, code, note) = execute(&job);
                manager.finish(&job.id, state, code, note);
            });
        }
    }

    fn finish(self: &Arc<Self>, id: &str, state: State, code: Option<i32>, note: Option<String>) {
        let finished = {
            let mut inner = self.inner.lock().unwrap();
            let result = if let Some(job) = inner.jobs.get_mut(id) {
                job.state = state;
                job.exit_code = code;
                job.completed_at = Some(millis());
                job.note = note;
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
                if job.state == State::Succeeded { "Job completed" } else { "Job failed" },
                &job.summary(),
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
        job.consumed = consumed;
        persist(&self.jobs_dir, job)?;
        Ok(job.clone())
    }

    fn cancel(&self, id: &str) -> Result<Job> {
        let mut inner = self.inner.lock().unwrap();
        let job_id = unique_id(&inner, id)?;
        let job = inner.jobs.get_mut(&job_id).unwrap();
        if job.state != State::Queued {
            bail!("Only queued jobs can currently be cancelled");
        }
        job.state = State::Cancelled;
        job.completed_at = Some(millis());
        persist(&self.jobs_dir, job)?;
        Ok(job.clone())
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

fn execute(job: &Job) -> (State, Option<i32>, Option<String>) {
    let result = (|| -> Result<Option<i32>> {
        let file = OpenOptions::new()
            .write(true).create_new(true).mode(0o600)
            .open(&job.log_path)
            .with_context(|| format!("open {}", job.log_path))?;
        let stderr = file.try_clone()?;
        let status = Command::new("fish")
            .arg("-c")
            .arg(&job.command)
            .stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::from(stderr))
            .status()
            .context("run fish command")?;
        Ok(status.code())
    })();
    match result {
        Ok(Some(0)) => (State::Succeeded, Some(0), None),
        Ok(code) => (State::Failed, code, None),
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
                Ok(Response { job: Some(job), ..Response::success("Queued job cancelled") })
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
    let mut line = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut line)?;
    if line.len() > MAX_COMMAND_BYTES + 1024 {
        bail!("Request too large");
    }
    let reply = match serde_json::from_str::<Request>(&line) {
        Ok(request) => handle(&manager, request),
        Err(error) => Response::failure(format!("Invalid request: {error}")),
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
    fn state_limit_validity() {
        assert!(State::Succeeded.finished());
        assert!(State::Interrupted.finished());
        assert!(!State::Running.finished());
        assert!(!State::Queued.finished());
    }
}
