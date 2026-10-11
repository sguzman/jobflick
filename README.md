# Jobflick

**Commands in. Results out.**

Jobflick is a small, local-first command queue and result inbox for Linux desktops. Copy a complete Execarium YAML job, press a hotkey to submit it, and get on with something else. Jobflick runs queued commands within a configurable concurrency limit, saves their output, and lets you retrieve the result from a fast, searchable overlay.

No browser integration, cloud account, or API key is required.

## How it works

1. Copy a complete **Execarium schema-1 script or schema-2 command sequence** to the clipboard. Raw Fish text is refused.
2. Trigger `jobflick submit --clipboard` from a global hotkey. The YAML is submitted once; copying text alone does not execute it. Invalid YAML, unsupported privilege/shell modes, invalid Fish syntax, and copied Markdown fences are rejected before queueing, with a desktop error notification.
3. Jobflick queues only validated Execarium jobs. The background manager starts jobs as execution slots open, captures a combined stdout/stderr log, and sends desktop notifications identified by short job ID (never the copied command text). Notifications are best-effort and cannot block queue execution. They use `notify-send -c success` for completed jobs, `-c failure` for failed or timed-out jobs and rejected clipboard submissions, and `-c warning` for cancelled or interrupted jobs. Queued and started notifications use the desktop's default appearance. Mako can style these exact category names without any Jobflick-specific desktop configuration.
4. Trigger `jobflick hud` to search jobs, inspect their states, and preview the last 8 KiB of output from a selected running or finished job.
5. Select a finished job and press **Enter** to copy its report and clear it from the active inbox, or **Shift+Enter** to copy without clearing it. If the clipboard copy succeeds but the inbox update cannot be confirmed, the HUD explicitly says the report was copied and leaves the window open.

Clearing an item from the inbox does not erase its saved log. Completed results remain recoverable.

## Interface

| Shortcut | Action |
| --- | --- |
| `Super+Enter` (suggested desktop binding) | Validate and submit clipboard Execarium YAML |
| `Super+J` (suggested desktop binding) | Open job inbox |
| `Up` / `Down` | Select job |
| `Enter` | Copy finished job report, consume from inbox, close |
| `Shift+Enter` | Copy finished job report without consuming, close |
| `Escape` | Close HUD |
| `Stop running` | Stop a running job and its normal child processes |

The global hotkeys are configured through your window manager, not installed automatically by Jobflick.

CI additionally exercises a genuine Wayland clipboard and HUD launch under a
headless Weston compositor. This checks the Wayland integration without
requiring a human tester, but does **not** certify Hyprland-specific window
rules, Mako presentation or a live systemd user session. Those remain separate
desktop release-acceptance requirements.

## Install and run

Jobflick is written in Rust and uses egui for its compact desktop HUD.
On a Wayland session it requires Fish, `wl-clipboard` (`wl-paste` and
`wl-copy`), and `libnotify` (`notify-send`).

Install or update from a checkout with Cargo:

```fish
cargo install --path . --force
```

The executable lives at `~/.cargo/bin/jobflick` (or `$CARGO_HOME/bin/jobflick`
if Cargo's home is customized). `fish scripts/install.fish` is a short alias
for the same Cargo command. Installation does not change Hyprland or Kitty
configuration or suggest keybindings.

`jobflick submit --clipboard` (Execarium YAML only) and `jobflick hud` use the local background
daemon. Without an enabled service, they start it on demand. When the service
is enabled, they ask the user manager to start it if needed, keeping systemd in
charge of daemon startup. Concurrent launchers coordinate through a private
runtime lock and share one daemon, with up to ten seconds for slow inbox
loading. The daemon uses the same installed executable and stores its job state
under the XDG data directory. IPC requests and responses have bounded framing
and socket timeouts, so a stalled peer cannot hang the HUD indefinitely.

To connect global shortcuts, point your window-manager bindings at the actual
Cargo binary. For example, in Hyprland Lua:

```lua
hl.bind("SUPER + RETURN", hl.dsp.exec_cmd(os.getenv("HOME") .. "/.cargo/bin/jobflick submit --clipboard"))
hl.bind("SUPER + J", hl.dsp.exec_cmd(os.getenv("HOME") .. "/.cargo/bin/jobflick hud"))
```

### systemd user service

To start Jobflick automatically with your user login, install the supplied
user unit after installing the binary:

```sh
install -Dm644 packaging/jobflick.service "$HOME/.config/systemd/user/jobflick.service"
systemctl --user daemon-reload
systemctl --user enable --now jobflick.service
```

The unit runs `~/.cargo/bin/jobflick daemon`, restarts it after an unexpected
failure, and is wanted by `default.target`. It does not require systemd
lingering. Do not enable lingering just for Jobflick; with lingering disabled,
the user manager starts the service as part of login. The unit and on-demand
CLI/HUD launchers use the same runtime socket and persistent data lock, so
normal launchers connect to the managed daemon instead of starting duplicates.
Keep the installed executable at the path configured in `ExecStart`, or update
the unit to match a custom `CARGO_HOME`.

Manage and inspect the service with:

```sh
systemctl --user status jobflick.service
systemctl --user restart jobflick.service
journalctl --user -u jobflick.service -f
systemctl --user disable --now jobflick.service
```

Before restarting or stopping the service, let running jobs finish. A daemon
restart marks running jobs **Interrupted**; it never replays them. Submitted
and completed jobs remain in the normal XDG data directory. For troubleshooting,
check `systemctl --user is-enabled jobflick.service`, confirm the `ExecStart`
path exists, then inspect `journalctl --user -u jobflick.service` for daemon
startup or storage errors. `systemctl --user daemon-reload` applies unit-file
changes.

### Migrating from the early installer

Earlier versions installed an executable under `~/.local/bin` and enabled a
`jobflick.service` user unit automatically. The current supplied unit uses the
Cargo-installed executable. After existing jobs finish, stop and disable an old
unit before replacing it with the current unit above:

```fish
systemctl --user disable --now jobflick.service
```

The old unit file is not touched by `cargo install`. It can be removed
separately when no longer wanted. Also update any old desktop shortcuts that
still point at `~/.local/bin/jobflick`; Cargo cannot rewrite them.

### Floating HUD on Hyprland

The HUD is a small, centered, **fully opaque** overlay, not a tiled workspace window. On
Hyprland's Lua configuration, `jobflick hud` registers a **named, narrowly
scoped runtime window rule** through `hyprctl eval` *before opening the HUD*.
The rule matches only the `io.github.sguzman.jobflick` application ID,
floats and centers the window, and limits it to 720×480. It is safely
re-applied when needed, including after a Hyprland reload.

This does not edit your desktop configuration, alter keybindings, or float
other applications. Outside Hyprland, an equivalent compositor-specific
window rule may be required.

## Execarium YAML integration (required for every new job)

Jobflick now consumes the independent
[Execarium schema 1](https://github.com/sguzman/execarium/blob/main/docs/SPEC-v1.md)
and [schema 2](https://github.com/sguzman/execarium/blob/main/docs/SPEC-v2.md)
through a pinned, reviewed Execarium 1.2.0 source revision. Execarium is now the **only** submission format: `submit --clipboard` reads YAML,
`submit -- '<full YAML document>'` accepts YAML directly, and `submit-spec`
reads YAML from a file or stdin. Raw Fish is rejected at the CLI and daemon IPC.

Example:

```yaml
schema: 1
shell: fish
cwd: /home/user/Projects/example
privilege: none
timeout_seconds: 600
command: |
  cargo fmt --check
  and cargo test
```

Schema 2 provides a distinct `commands:` list with ordered, fail-fast semantics:

```yaml
schema: 2
shell: fish
cwd: /home/user/Projects/example
privilege: none
timeout_seconds: 600
commands:
  - cargo fmt --check
  - cargo test
  - cargo build
```

Jobflick starts each command in a **separate Fish process**, in `cwd`, and
launches the next only after the previous exits successfully. The first failed
step stops the sequence; the report includes the 1-based failed step index.
Cancellation or a single aggregate timeout prevents later steps from starting.
Each command's temporary Fish process state is independent. Schema 1 retains
`command: |` for ordinary scripts sharing one shell state. Neither schema
silently accepts the other schema's command field.

To submit an existing YAML file, run `jobflick submit-spec job.yaml`.
For explicit stdin use `jobflick submit-spec - < job.yaml`.
The document is parsed and validated **again in the daemon** before it
can be enqueued, even if another client bypasses the CLI. Jobflick never
watches the clipboard for YAML, and merely copying YAML cannot queue a job.

### Current execution policy

* **Supported:** `schema: 1` with `command:`, or `schema: 2` with `commands:`; `shell: fish`, `privilege: none`.
* **Refused before queueing:** `bash` and `sh` (no silent conversion),
  `sudo` and `doas` (pending a future explicit approval and OS
  authentication workflow), invalid syntax, and inaccessible `cwd`.
* The `cwd` field controls the actual Fish worker's initial working
  directory, instead of requiring an embedded `cd`. The directory
  must exist at submission; changes before launch may still cause the
  worker to fail, without retrying another directory.
* `timeout_seconds` is enforced as elapsed **execution time** starting
  at process launch, not while queued. When expired, Jobflick stops its
  process group (SIGTERM then SIGKILL), records **Timed out** as a
  distinct terminal state and preserves output and metadata. Intentionally
  detached children can evade process-group termination.
* Every new job includes validated Execarium metadata and a bounded execution timeout.
  Completed legacy JSON job records without metadata remain readable, but old queued
  raw-command jobs are marked failed rather than executed after restart. A crashed Running job is still marked
  Interrupted, never automatically replayed.
* `privilege: none` is **not a sandbox**; a Fish command could itself
  invoke `sudo` or `doas`. Jobflick does not promise privilege
  confinement. Review any command before explicitly submitting it.

Submission remains explicit: Jobflick does **not** watch the clipboard,
change Hyprland bindings, or install credentials. The systemd user service is
an explicit opt-in, configured using the instructions above.
When upgrading from an older daemon, let running jobs finish and restart it
before submitting Execarium YAML so the daemon understands `submit_spec`.
See [the changelog](CHANGELOG.md) for release scope, compatibility and current limitations.

## Configuration

On first start, Jobflick writes `$XDG_CONFIG_HOME/jobflick/config.toml` (normally `~/.config/jobflick/config.toml`):

```toml
max_concurrent = 3
```

Edit this file and restart the daemon to change the number of simultaneous jobs (1–16).

## CLI

| Command | Purpose |
| --- | --- |
| `jobflick submit --clipboard` | Validate and queue Execarium schema-1 or schema-2 YAML from clipboard |
| `jobflick submit -- '<full YAML document>'` | Validate and queue an explicit Execarium document |
| `jobflick submit-spec <file.yaml\|->` | Validate Execarium schema-1 or schema-2 YAML from file or stdin |
| `jobflick hud` | Open the searchable result inbox |
| `jobflick list` | Show active jobs |
| `jobflick list --all` | Include consumed jobs |
| `jobflick show <id>` | Print a finished job's report |
| `jobflick consume <id>` | Hide a job from the active inbox |
| `jobflick restore <id>` | Restore a consumed job to the inbox |
| `jobflick cancel <id>` | Cancel queued work or request stop of a running job |
| `jobflick daemon` | Run the queue manager |

Running jobs can be stopped from the HUD or CLI. Jobflick sends SIGTERM to the job's own process group, waits one second, sends SIGKILL to any remaining members, and reaps its Fish process before freeing the slot. The HUD shows **Stopping…** while termination is in progress. Normal subprocesses share the process group; intentionally detached processes may escape it. A cancellation request concurrent with completion can be recorded as **Cancelled** even if the command already exited, with this ambiguity preserved in the report.

The queue is durable. After a restart, queued jobs retain their original FIFO order. Jobs
previously marked **Running** become **Interrupted** and are never automatically
replayed; Jobflick cannot determine whether an old process completed or is still
running after an abrupt daemon termination. If a stop request was in progress,
the interruption note preserves that fact rather than declaring cancellation
successful. Invalid, unreadable, or oversized (over 1 MiB) individual job records are logged and skipped,
without deleting or modifying the original files. A job is not launched until its **Running** state has been saved successfully; if storage fails, the scheduler keeps the job queued rather than risking a duplicate execution after a restart. While a queue slot is available, the daemon retries that pending transition every two seconds so the job can resume after storage is repaired, without another submission. Cancelling or consuming a job likewise only changes the in-memory state after its updated record has been saved. If saving a completed result fails, Jobflick marks it **Interrupted** in memory instead of showing a false durable success; the original Running record is left for conservative recovery, and the saved output log may still be inspected.

## Data and execution

Each job has a unique ID, the original command, timestamps, status, exit code when available, and a durable combined output log. Files live under `$XDG_DATA_HOME/jobflick` (normally `~/.local/share/jobflick`); runtime IPC lives under `$XDG_RUNTIME_DIR/jobflick`. Job files are private to the current user.

Before queueing, Jobflick checks the command with `fish --no-config --no-execute`.
This catches syntax mistakes without running the command. It does not prove
that syntactically valid text is meaningful or safe, and does not guess user
intent from ordinary words. Failed clipboard submissions display a generic
desktop notification without exposing clipboard text.

Noninteractive schema-1 scripts run using `fish -c` with stdin closed; schema-2 steps each launch independent `fish -c` processes. Explicit Execarium jobs
use their validated working directory and have a supervised timeout; all newly
accepted jobs must declare an Execarium timeout. Commands requiring terminal interaction are not yet supported by the worker. Jobflick is intentionally a **local command executor**: it runs with your user permissions. Review copied commands before submitting them, and avoid submitting secrets you do not want stored in command history or logs. Clipboard reads happen only on explicit submission, never continuously. The CLI enforces the 256 KiB Execarium document limit while reading from `wl-paste`; oversized clipboard data is rejected without queuing a truncated document.

The copied report includes the command, status, exit code, timings, a bounded tail of the log, and the saved log path if output was truncated. If a finished job's saved log is missing or unreadable, both the report and HUD preview identify that failure explicitly instead of presenting it as empty output. The exception is a job cancelled before execution: it never had a log, so no missing-log warning is shown.

## Releasing (post-MVP)

The development version may be `2.0.0` before its release is accepted. Do not
infer a published release from Cargo metadata. The protected **Publish Jobflick**
GitHub Actions workflow must be explicitly dispatched with the matching version
and confirmation that actual Hyprland hotkeys, HUD, daemon/service behavior,
clipboard reports, and Mako notifications have been accepted on a desktop.
It reruns the full CI suite, uses a locked dependency graph, refuses an existing
version tag, and creates an immutable tag and GitHub Release with a Linux x86-64
binary archive and SHA-256 checksums. Never claim desktop acceptance from
headless GitHub CI alone.

## Design

Jobflick has two parts in one executable: a small persistent background scheduler reachable through a user-only Unix socket, and an on-demand egui HUD. The graphical frontend can close at any time without stopping running jobs. Kitty integration for genuinely interactive jobs may be added separately; ordinary commands do not require Kitty.

## License

Original Jobflick source code is available under the [MIT License](LICENSE).
