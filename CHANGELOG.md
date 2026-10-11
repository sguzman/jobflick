# Release notes

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
