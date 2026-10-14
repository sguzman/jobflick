# Jobflick v2.0.0 release acceptance

## Scope and version

- Source version: `2.0.0` in `Cargo.toml`.
- Major-version break: raw Fish submission is removed; all new submissions
  use validated Execarium schema 1, 2 or 3 YAML.
- Execarium dependency: Git revision
  `79e2430362956c2790926c2498c970b9328159f4`
  (Execarium crate 1.3.0), pinned alongside `Cargo.lock`.
- Platform: Rust executable for Linux desktops; Fish worker and
  egui HUD; optional systemd user service; Hyprland-focused desktop setup.

## Verified automated coverage

The main-branch Rust workflow checks formatting, `cargo check --locked`,
`cargo test --locked`, Clippy with warnings denied, a locked build,
and a headless Sway Wayland clipboard/HUD smoke test. Additional
integration gates test concurrent submissions, daemon/service resilience,
legacy persistence, schema validation, stop-on-first-failure, timeouts,
process-group termination and recoverability.

A workflow's green status applies to the exact commit it checked.
The publish workflow **reruns** this CI via `workflow_call` before packaging.

## Owner-accepted desktop behavior

The 2026-10-13 local Linux/Hyprland acceptance exercise confirmed:

- Installation of the locked Rust build and normal systemd user service.
- Schema-3 script execution and independent sequential Fish steps.
- Live Wayland socket detection and real Hyprland IPC/workspace queries.
- Integrated-GPU environment requests, NVIDIA RTX 3090 environment routing
  and combined Hyprland/NVIDIA context.
- An actual NVIDIA CUDA kernel launch and verified device-to-host result
  (value 42), rather than environment variables alone.
- Six submitted child jobs finishing successfully in two groups of three,
  consistent with a maximum concurrency of three.
- Sequence abort on failure, a 2-second execution timeout stopping step
  2 of 3, and a restart preserving historical job reports.
- Strict rejection of unsupported GPU values. Desktop notification
  behavior was observed during multi-job testing.

**Live tests explicitly deferred by the owner:**

- Manual cancellation of an active job (test 9).
- Queued-job recovery across a deliberate daemon restart (test 13),
  deferred because unrelated long-running work must not be interrupted.

These are *coverage limits*, not evidence of failures. Existing automated
regression tests continue to exercise cancellation and restart handling.
The owner explicitly accepted Jobflick's present scope for 2.0.0.

## Publish procedure

1. Review the current `main` source and confirm the Rust CI run is green.
2. Open [GitHub Actions → Publish Jobflick](https://github.com/sguzman/jobflick/actions/workflows/release.yml).
3. Select **Run workflow** on branch `main`; set `version` to `2.0.0`
   and check `desktop_acceptance_confirmed` only after explicit owner
   acceptance.
4. Wait for both `verify` and `publish` jobs to succeed.
5. Verify [the `v2.0.0` release](https://github.com/sguzman/jobflick/releases/tag/v2.0.0)
   exists, is neither draft nor prerelease, and includes
   `jobflick-linux-x86_64.tar.gz` and `SHA256SUMS`.

The workflow refuses a mismatched Cargo version or an existing tag,
builds with `--locked`, and creates an immutable versioned release.
Do not describe 2.0.0 as **published** until that workflow and release
have been verified.

**No local daemon restart is part of publication.** Running user jobs and
the installed local executable are independent of GitHub Release creation.
