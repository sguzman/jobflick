# Release notes

## 2.0.0 — 2026-10-13

Post-MVP major update: Execarium is now the required, versioned input contract
for all new jobs. This is intentionally incompatible with legacy raw Fish
submission; historical completed jobs remain readable.

### Breaking change

- All new clipboard, `submit`, `submit-spec` and daemon IPC submissions require
  complete validated Execarium YAML. Raw Fish commands are refused.
- Pending raw jobs from older versions are marked Failed on recovery rather
  than executed. Historical completed records are preserved.

### Execution formats and environments

- Accept Execarium schema 1 (`command:` script), schema 2 (`commands:`
  sequential independent steps), and schema 3 (either shape plus required
  `environment.session` and `environment.gpu`).
- Integrate the pinned, released Execarium 1.3.0 Rust library using the
  matching `Cargo.lock`. Schemas 1 and 2 remain compatible.
- Resolve live Wayland or Hyprland session variables and sockets when a
  schema-3 job *starts*, rather than assuming the systemd daemon inherited
  them. Missing or ambiguous requested contexts fail explicitly.
- Request integrated or NVIDIA GPU offload independently of session choice,
  including headless jobs. GPU routing is an API-dependent preference, not
  hardware isolation.
- Enforce validated Fish syntax, accessible working directories,
  unprivileged execution and a single elapsed-runtime deadline. Sequences
  stop on the first failed step and report its 1-based index.
- Preserve typed execution context and step information in durable job records
  and human-readable reports.

### Reliability, desktop integration and verification

- Keep a persistent concurrency-limited queue and existing systemd user
  service compatibility. Running jobs are never automatically replayed
  after an interrupted daemon; queued jobs retain their recovery behavior.
- Classify Mako/freedesktop notifications: success on completion; failure on
  errors and timeouts; warning on cancellation and interruption. Never include
  command text in desktop notifications.
- Require clean Rust formatting, locked dependency resolution, Clippy and
  Rust integration tests in CI. Include a real headless Sway clipboard/HUD
  smoke test, schema-3 Wayland execution checks, and scheduler safety tests.
- Validate schema 3, Wayland and Hyprland IPC, Intel/AMD integrated routing,
  NVIDIA RTX 3090 routing, actual CUDA kernel execution, concurrent
  scheduling, stop-on-first-failure, timeouts and persisted history on a
  Linux/Hyprland desktop.
- Live cancellation and queued-job restart recovery were intentionally not
  part of the desktop acceptance run; automated regression coverage remains.
  See [the acceptance record](docs/RELEASE-v2.0.0.md).
- Preserve a **manual** GitHub Release gate requiring explicit desktop
  acceptance and full CI before versioned binaries are published.

## 1.1.0 — 2026-10-11

The MVP now includes an opt-in systemd user service for reliable startup and
restart-on-failure behavior.

- Install and enable `packaging/jobflick.service` to start the daemon with the
  user session; the unit launches the Cargo-installed executable.
- Keep systemd service and on-demand CLI/HUD use on the same daemon socket and
  exclusive data lock, avoiding duplicate daemons.
- Route on-demand launches through systemd when the user service is enabled,
  avoiding races between hotkeys and session startup.
- Document service installation, management, login behavior, and troubleshooting.

The service does not enable lingering. Running work is still marked Interrupted
after a daemon restart and is never replayed.

## 1.0.0 — 2026-10-11

Jobflick's first MVP release provides an explicit, local command queue and
searchable result inbox for Linux desktops.

- Submit raw Fish commands from the clipboard or CLI; clipboard reads happen
  only on explicit submission.
- Persist queued jobs, ordering, command output, and inbox state across daemon
  restarts. Running jobs are marked interrupted and are never replayed.
- Limit concurrency, schedule queued work automatically, and cancel running
  jobs together with their normal child process groups.
- Open a floating Hyprland HUD to search jobs, preview output, copy reports,
  and consume or preserve results with Enter or Shift+Enter.
- Accept Execarium v1.1.2 schema-1 YAML only through `submit-spec`, with
  Fish, no privilege, an accessible working directory, and an enforced
  execution timeout.
- Keep desktop notifications best-effort and avoid including command text.

The MVP does not support interactive terminal jobs, privilege escalation,
or sandboxing. Intentionally detached child processes may outlive group
termination. Raw Fish commands have no implicit timeout. See the README for
execution and data handling details.
