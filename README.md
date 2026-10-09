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
daemon on demand when it is not already running. The on-demand daemon uses the
same installed executable and stores its job state under the XDG data directory.

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
floats and centers the window, and limits it to 760×520. It is safely
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
