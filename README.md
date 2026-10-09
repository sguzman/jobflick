# Jobflick

**Commands in. Results out.**

Jobflick is a small, local-first command queue and result inbox for Linux desktops. Copy a shell command, press a hotkey to submit it, and get on with something else. Jobflick runs queued commands within a configurable concurrency limit, saves their output, and lets you retrieve the result from a fast, searchable overlay.

No browser integration, cloud account, or API key is required.

## How it works

1. Copy a command (including any needed `cd`) to the clipboard.
2. Trigger `jobflick submit --clipboard` from a global hotkey. The command is submitted once; simply copying text does not execute it. Invalid Fish syntax and copied Markdown fences are rejected before queueing, with a desktop error notification.
3. Jobflick queues the command. The background manager starts jobs as execution slots open, captures a combined stdout/stderr log, and sends desktop notifications identified by short job ID (never the copied command text). Notifications are best-effort and cannot block queue execution.
4. Trigger `jobflick hud` to search jobs, inspect their states, and preview the last 8 KiB of output from a selected running or finished job.
5. Select a finished job and press **Enter** to copy its report and clear it from the active inbox, or **Shift+Enter** to copy without clearing it. If the clipboard copy succeeds but the inbox update cannot be confirmed, the HUD explicitly says the report was copied and leaves the window open.

Clearing an item from the inbox does not erase its saved log. Completed results remain recoverable.

## Interface

| Shortcut | Action |
| --- | --- |
| `Super+Enter` (suggested desktop binding) | Submit clipboard contents |
| `Super+J` (suggested desktop binding) | Open job inbox |
| `Up` / `Down` | Select job |
| `Enter` | Copy finished job report, consume from inbox, close |
| `Shift+Enter` | Copy finished job report without consuming, close |
| `Escape` | Close HUD |
| `Stop running` | Stop a running job and its normal child processes |

The global hotkeys are configured through your window manager, not installed automatically by Jobflick.

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
for the same Cargo command. Neither method installs or enables a systemd unit,
touches Hyprland or Kitty configuration, or suggests keybindings.

`jobflick submit --clipboard` and `jobflick hud` start the local background
daemon on demand when it is not already running. Concurrent launchers coordinate
through a private runtime lock and share one daemon, with up to ten seconds for
slow inbox loading. The daemon uses the same installed executable and stores
its job state under the XDG data directory. IPC requests and responses have
bounded framing and socket timeouts, so a stalled peer cannot hang the HUD indefinitely.

To connect global shortcuts, point your window-manager bindings at the actual
Cargo binary. For example, in Hyprland Lua:

```lua
hl.bind("SUPER + RETURN", hl.dsp.exec_cmd(os.getenv("HOME") .. "/.cargo/bin/jobflick submit --clipboard"))
hl.bind("SUPER + J", hl.dsp.exec_cmd(os.getenv("HOME") .. "/.cargo/bin/jobflick hud"))
```

An optional, **not automatically installed**, systemd user-service template
is provided at [packaging/jobflick.service](packaging/jobflick.service).
It runs the same Cargo-installed binary if you later choose to configure it.

### Migrating from the early installer

Earlier versions installed an executable under `~/.local/bin` and enabled a
`jobflick.service` user unit automatically. After existing jobs finish, stop
and disable the old service before switching to the Cargo-installed executable:

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

## Configuration

On first start, Jobflick writes `$XDG_CONFIG_HOME/jobflick/config.toml` (normally `~/.config/jobflick/config.toml`):

```toml
max_concurrent = 3
```

Edit this file and restart the daemon to change the number of simultaneous jobs (1–16).

## CLI

| Command | Purpose |
| --- | --- |
| `jobflick submit --clipboard` | Queue clipboard text as a Fish command |
| `jobflick submit -- 'echo hello'` | Queue an explicitly supplied command |
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

Noninteractive jobs run using `fish -c` with stdin closed. Commands requiring terminal interaction are not yet supported by the worker. Jobflick is intentionally a **local command executor**: it runs with your user permissions. Review copied commands before submitting them, and avoid submitting secrets you do not want stored in command history or logs. Clipboard reads happen only on explicit submission, never continuously. The CLI enforces the 128 KiB command limit while reading from `wl-paste`; oversized clipboard data is rejected without queuing a truncated command.

The copied report includes the command, status, exit code, timings, a bounded tail of the log, and the saved log path if output was truncated. If a finished job's saved log is missing or unreadable, both the report and HUD preview identify that failure explicitly instead of presenting it as empty output. The exception is a job cancelled before execution: it never had a log, so no missing-log warning is shown.

## Design

Jobflick has two parts in one executable: a small persistent background scheduler reachable through a user-only Unix socket, and an on-demand egui HUD. The graphical frontend can close at any time without stopping running jobs. Kitty integration for genuinely interactive jobs may be added separately; ordinary commands do not require Kitty.

## License

Original Jobflick source code is available under the [MIT License](LICENSE).
