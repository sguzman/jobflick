# Jobflick

**Commands in. Results out.**

Jobflick is a small, local-first command queue and result inbox for Linux desktops. Copy a shell command, press a hotkey to submit it, and get on with something else. Jobflick runs queued commands within a configurable concurrency limit, saves their output, and lets you retrieve the result from a fast, searchable overlay.

No browser integration, cloud account, or API key is required.

## How it works

1. Copy a command (including any needed `cd`) to the clipboard.
2. Trigger `jobflick submit --clipboard` from a global hotkey. The command is submitted once; simply copying text does not execute it.
3. Jobflick queues the command. The background manager starts jobs as execution slots open, captures a combined stdout/stderr log, and sends desktop notifications.
4. Trigger `jobflick hud` to search jobs and inspect queued, running, successful, failed, cancelled, or interrupted states.
5. Select a finished job and press **Enter** to copy its report and clear it from the active inbox, or **Shift+Enter** to copy without clearing it.

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

The global hotkeys are configured through your window manager, not installed automatically by Jobflick.

## Install and run

Jobflick is written in Rust and uses egui for its compact desktop HUD. It targets a Wayland session with Fish, `wl-clipboard` (`wl-paste` and `wl-copy`), and `libnotify` (`notify-send`).

```sh
cargo build --release
install -Dm755 target/release/jobflick ~/.local/bin/jobflick
jobflick daemon
```

Run `jobflick daemon` as a long-lived user service; the submit and HUD commands can also automatically start it if it is not running.

Example Hyprland binds (choose different keys if these are already assigned):

```ini
bind = SUPER, RETURN, exec, jobflick submit --clipboard
bind = SUPER, J, exec, jobflick hud
```

A `systemd --user` unit is provided at [packaging/jobflick.service](packaging/jobflick.service).

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
| `jobflick cancel <id>` | Cancel a queued job |
| `jobflick daemon` | Run the queue manager |

The queue is durable. If the manager restarts, queued jobs remain queued; jobs that were running are marked **interrupted** rather than silently rerun.

## Data and execution

Each job has a unique ID, the original command, timestamps, status, exit code when available, and a durable combined output log. Files live under `$XDG_DATA_HOME/jobflick` (normally `~/.local/share/jobflick`); runtime IPC lives under `$XDG_RUNTIME_DIR/jobflick`. Job files are private to the current user.

Noninteractive jobs run using `fish -c` with stdin closed. Commands requiring terminal interaction are not yet supported by the worker. Jobflick is intentionally a **local command executor**: it runs with your user permissions. Review copied commands before submitting them, and avoid submitting secrets you do not want stored in command history or logs. Clipboard reads happen only on explicit submission, never continuously.

The copied report includes the command, status, exit code, timings, a bounded tail of the log, and the saved log path if output was truncated.

## Design

Jobflick has two parts in one executable: a small persistent background scheduler reachable through a user-only Unix socket, and an on-demand egui HUD. The graphical frontend can close at any time without stopping running jobs. Kitty integration for genuinely interactive jobs may be added separately; ordinary commands do not require Kitty.

## License

Original Jobflick source code is available under the [MIT License](LICENSE).
