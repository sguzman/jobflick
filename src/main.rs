mod backend;
mod hud;
mod ipc;
mod paths;
mod protocol;

use crate::protocol::{Request, Response};
use anyhow::{anyhow, bail, Context, Result};
use std::fs::OpenOptions;
use std::io::{BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

fn help() {
    println!(
        "Jobflick - commands in, results out.\n\
         \n\
         Usage:\n\
           jobflick submit --clipboard\n\
           jobflick submit -- '<fish command>'\n\
           jobflick hud\n\
           jobflick list [--all]\n\
           jobflick show <job-id>\n\
           jobflick consume <job-id>\n\
           jobflick restore <job-id>\n\
           jobflick cancel <job-id>  (queued or running)\n\
           jobflick daemon"
    );
}

fn send_on_stream(mut stream: UnixStream, request: &Request) -> Result<Response> {
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    let line = ipc::read_frame(BufReader::new(stream), ipc::MAX_RESPONSE_FRAME)
        .context("read bounded daemon response")?;
    serde_json::from_slice(&line).context("decode daemon response")
}

/// Only one client at a time may spawn the on-demand daemon. Waiting
/// launchers connect to the same socket instead of spawning competitors.
fn connect_or_start_daemon() -> Result<UnixStream> {
    if let Ok(stream) = UnixStream::connect(paths::socket()) {
        return Ok(stream);
    }

    let runtime = paths::runtime_dir();
    paths::private_dir(&runtime)?;
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .mode(0o600)
        .open(runtime.join("startup.lock"))
        .context("open Jobflick startup lock")?;

    let mut acquired = false;
    for _ in 0..100 {
        let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            acquired = true;
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(error).context("lock Jobflick daemon startup");
        }
        if let Ok(stream) = UnixStream::connect(paths::socket()) {
            return Ok(stream);
        }
        thread::sleep(Duration::from_millis(100));
    }
    if !acquired {
        bail!("Timed out waiting for another Jobflick launcher");
    }

    // A different launcher may have finished startup while we waited.
    if let Ok(stream) = UnixStream::connect(paths::socket()) {
        return Ok(stream);
    }

    let exe = std::env::current_exe()?;
    Command::new(exe)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("launch Jobflick daemon")?;

    // A large durable inbox may take longer than the old three seconds
    // to load, especially from slow storage.
    for _ in 0..100 {
        if let Ok(stream) = UnixStream::connect(paths::socket()) {
            return Ok(stream);
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!("Daemon did not become available after 10 seconds; run 'jobflick daemon' to diagnose")
}

pub(crate) fn send(request: Request) -> Result<Response> {
    // Never retry a request after writing any bytes: a disconnected response
    // does not prove that the command was not queued.
    let response = send_on_stream(connect_or_start_daemon()?, &request)?;
    if !response.ok {
        bail!("{}", response.message);
    }
    Ok(response)
}

fn clipboard_text() -> Result<String> {
    let output = Command::new("wl-paste")
        .arg("--no-newline")
        .output()
        .context("read Wayland clipboard (install wl-clipboard)")?;
    if !output.status.success() {
        bail!("wl-paste failed with status {}", output.status);
    }
    String::from_utf8(output.stdout).context("clipboard is not valid UTF-8")
}

pub(crate) fn copy_clipboard(text: &str) -> Result<()> {
    let mut child = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("open wl-copy (install wl-clipboard)")?;
    let write_result = child.stdin.take().context("wl-copy stdin missing")?
        .write_all(text.as_bytes());
    let status = child.wait().context("wait for wl-copy")?;
    write_result?;
    if !status.success() {
        bail!("wl-copy exited with status {status}");
    }
    Ok(())
}

fn request(action: &str, id: Option<String>, all: bool) -> Request {
    Request {
        action: action.into(),
        id,
        all,
        ..Request::default()
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(subcommand) = args.first().map(String::as_str) else {
        help();
        return Ok(());
    };
    match subcommand {
        "daemon" => backend::daemon(),
        "hud" => hud::open(),
        "submit" => {
            let command = match args.get(1).map(String::as_str) {
                Some("--clipboard") | None => clipboard_text()?,
                Some("--") => args.iter().skip(2).cloned().collect::<Vec<_>>().join(" "),
                Some(_) => bail!("Use 'submit --clipboard' or 'submit -- <command>'"),
            };
            let reply = send(Request {
                action: "submit".into(),
                command: Some(command),
                ..Request::default()
            })?;
            let job = reply.job.context("Daemon did not return submitted job")?;
            println!("Queued {}  {}", job.short_id(), job.summary());
            Ok(())
        }
        "list" => {
            let all = args.iter().any(|a| a == "--all");
            let reply = send(request("list", None, all))?;
            if reply.jobs.is_empty() {
                println!("No jobs in the inbox");
            }
            for job in reply.jobs {
                println!("{:<9} {:<12} {}", job.short_id(), job.state.label(), job.summary());
            }
            Ok(())
        }
        "show" | "consume" | "restore" | "cancel" => {
            let id = args.get(1).context("Missing job ID")?.clone();
            let reply = send(request(subcommand, Some(id), false))?;
            if subcommand == "show" {
                print!("{}", reply.report.context("Missing report")?);
            } else {
                println!("{}", reply.message);
            }
            Ok(())
        }
        "-h" | "--help" | "help" => {
            help();
            Ok(())
        }
        "-V" | "--version" => {
            println!("jobflick {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(anyhow!("Unknown command: {other}")),
    }
}

fn main() {
    if let Err(error) = run() {
        // Desktop bindings have no terminal for displaying errors.
        // Never include copied command text in a notification.
        let clipboard_submission = std::env::args().nth(1).as_deref() == Some("submit")
            && matches!(std::env::args().nth(2).as_deref(), None | Some("--clipboard"));
        if clipboard_submission {
            let _ = Command::new("notify-send")
                .args([
                    "-a",
                    "Jobflick",
                    "Submission error",
                    "Check the Jobflick inbox before retrying.",
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
        eprintln!("jobflick: {error:#}");
        std::process::exit(1);
    }
}
