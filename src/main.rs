mod backend;
mod hud;
mod ipc;
mod paths;
mod protocol;

use crate::protocol::{Request, Response};
use execarium::MAX_DOCUMENT_BYTES as MAX_SPEC_BYTES;
use std::fs::File;
use anyhow::{anyhow, bail, Context, Result};
use std::fs::OpenOptions;
use std::io::{BufReader, Read, Write};
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
           jobflick submit -- '<full Execarium YAML>'\n\
           jobflick submit-spec <file.yaml|->\n\
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

/// If the user enabled the packaged service, let systemd own daemon startup.
/// This avoids a startup race where an on-demand launcher wins the socket lock
/// just before the unit starts and leaves systemd repeatedly retrying a second
/// daemon that correctly refuses the exclusive data lock.
fn start_enabled_user_service() -> Result<bool> {
    let enabled = Command::new("systemctl")
        .args(["--user", "is-enabled", "jobflick.service"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !matches!(enabled, Ok(status) if status.success()) {
        return Ok(false);
    }

    let status = Command::new("systemctl")
        .args(["--user", "start", "jobflick.service"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("ask systemd to start the enabled Jobflick user service")?;
    if !status.success() {
        bail!("systemd could not start the enabled Jobflick user service");
    }
    Ok(true)
}

/// Only one client at a time may spawn the on-demand daemon. Waiting
/// launchers connect to the same socket instead of spawning competitors. If
/// the user service is enabled, systemd remains the owner of daemon startup.
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

    if !start_enabled_user_service()? {
        let exe = std::env::current_exe()?;
        Command::new(exe)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("launch Jobflick daemon")?;
    }

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

/// Bound clipboard reads to the maximum Execarium document size.
fn read_clipboard_bytes(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_SPEC_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_SPEC_BYTES {
        bail!("Clipboard Execarium YAML exceeds the 256 KiB document limit");
    }
    Ok(bytes)
}

/// Both clipboard and file submissions require an untruncated Execarium document.
fn read_spec(reader: impl Read) -> Result<String> {
    let mut bytes = Vec::new();
    reader.take((MAX_SPEC_BYTES + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_SPEC_BYTES {
        bail!("Execarium document exceeds the 256 KiB limit");
    }
    String::from_utf8(bytes).context("Execarium YAML is not valid UTF-8")
}

fn spec_source(argument: &str) -> Result<String> {
    if argument == "-" {
        read_spec(std::io::stdin().lock())
    } else {
        let file = File::open(argument).with_context(|| format!("open Execarium YAML file {argument}"))?;
        read_spec(file)
    }
}

fn clipboard_text() -> Result<String> {
    let mut child = Command::new("wl-paste")
        .arg("--no-newline")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("read Wayland clipboard (install wl-clipboard)")?;

    let bytes = (|| {
        let stdout = child.stdout.take().context("wl-paste stdout unavailable")?;
        read_clipboard_bytes(stdout)
    })();
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(error) => {
            // wl-paste can still be writing beyond the limit; stop it before
            // waiting, and never send a truncated command to the daemon.
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let status = child.wait().context("wait for wl-paste")?;
    if !status.success() {
        bail!("wl-paste failed with status {status}");
    }
    String::from_utf8(bytes).context("Clipboard Execarium YAML is not valid UTF-8")
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

/// Every transport sends one validated Execarium schema-1 or schema-2 document.
/// The daemon enforces the same parser and policy independently.
fn submit_document(document: String) -> Result<()> {
    let (shell, privilege) = match execarium::parse_document(&document)
        .context("Invalid Execarium YAML (schema 1 or 2)")? {
        execarium::Document::V1(spec) => (spec.shell(), spec.privilege()),
        execarium::Document::V2(spec) => (spec.shell(), spec.privilege()),
    };
    if shell != execarium::Shell::Fish {
        bail!("Jobflick currently supports only shell: fish; refusing silent conversion");
    }
    if privilege != execarium::Privilege::None {
        bail!("Privileged Execarium requests require an approval workflow; refusing to queue");
    }
    let reply = send(Request {
        action: "submit_spec".into(),
        spec_yaml: Some(document),
        ..Request::default()
    })?;
    let job = reply.job.context("Daemon did not return submitted job")?;
    println!("Queued {}  {}", job.short_id(), job.summary());
    Ok(())
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
            let document = match args.get(1).map(String::as_str) {
                Some("--clipboard") | None if args.len() <= 2 => clipboard_text()?,
                Some("--") if args.len() == 3 => args[2].clone(),
                _ => bail!("Only Execarium schema-1 or schema-2 YAML can be submitted; use 'submit --clipboard' or 'submit -- <full YAML document>'"),
            };
            submit_document(document)
        }
        "submit-spec" => {
            if args.len() != 2 {
                bail!("Usage: jobflick submit-spec <file.yaml|->");
            }
            submit_document(spec_source(&args[1])?)
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
                    "-c",
                    "failure",
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

#[cfg(test)]
mod clipboard_tests {
    use super::*;

    #[test]
    fn reads_clipboard_within_execarium_limit() {
        assert_eq!(read_clipboard_bytes(b"schema: 1\nshell: fish".as_slice()).unwrap(),
            b"schema: 1\nshell: fish");
        assert_eq!(
            read_clipboard_bytes(vec![b'a'; MAX_SPEC_BYTES].as_slice())
                .unwrap().len(),
            MAX_SPEC_BYTES
        );
    }

    #[test]
    fn reads_explicit_yaml_without_truncating_or_submitting() {
        let yaml = b"schema: 1\nshell: fish\n";
        assert_eq!(read_spec(yaml.as_slice()).unwrap(), std::str::from_utf8(yaml).unwrap());
        assert!(read_spec(vec![b'a'; MAX_SPEC_BYTES + 1].as_slice()).is_err());
        assert!(read_spec([0xff].as_slice()).is_err());
    }

    #[test]
    fn rejects_large_clipboards_without_returning_truncated_commands() {
        let oversized = vec![b'x'; MAX_SPEC_BYTES + 16384];
        assert!(read_clipboard_bytes(oversized.as_slice()).is_err());
    }
}
