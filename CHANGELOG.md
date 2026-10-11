# Release notes

## Unreleased — planned 2.0.0 (not yet released or locally accepted)

- **Breaking:** raw Fish submission is no longer supported; all CLI, clipboard/Super+Enter and IPC submissions require validated Execarium YAML.
- Add schema-2 `commands:` sequences with separate Fish steps, stop-on-first-failure, 1-based failed-step reporting, and one overall execution timeout.
- Preserve schema-1 `command:` scripts without altering their format or shared-shell semantics.
- Pin the verified, released Execarium 1.2.0 source revision (with matching Cargo.lock), and extend unit/integration tests for sequence success, failure, timeout, invalid YAML and schema mismatches.

- Require well-formed Execarium YAML for all new submissions, including clipboard via Super+Enter, direct CLI arguments, and daemon IPC. Raw Fish submissions are refused.
- Preserve completed legacy history, but fail formerly queued raw jobs on recovery without executing them.
- Add IPC and clipboard rejection regression tests and migrate integration fixtures to structured jobs.

- Classify desktop notifications for Mako: success on completion, failure on errors and timeouts, warning on cancellations or interruptions; queue/start use default styling.
- Emit a warning for a job cancelled while queued, without including its command in desktop notifications.

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
