#!/usr/bin/env bash
set -euo pipefail

# Test real Wayland clipboard IPC and an actual egui HUD launch without
# pretending a generic compositor validates Hyprland's window rules.
root="$(mktemp -d)"
daemon_pid=''
weston_pid=''
hud_pid=''
cleanup() {
  if [[ -n "$hud_pid" ]]; then kill "$hud_pid" 2>/dev/null || true; wait "$hud_pid" 2>/dev/null || true; fi
  if [[ -n "$daemon_pid" ]]; then kill "$daemon_pid" 2>/dev/null || true; wait "$daemon_pid" 2>/dev/null || true; fi
  if [[ -n "$weston_pid" ]]; then kill "$weston_pid" 2>/dev/null || true; wait "$weston_pid" 2>/dev/null || true; fi
  rm -rf "$root"
}
trap cleanup EXIT

export XDG_RUNTIME_DIR="$root/runtime"
export XDG_CONFIG_HOME="$root/config"
export XDG_DATA_HOME="$root/data"
export WAYLAND_DISPLAY='jobflick-ci'
export XDG_SESSION_TYPE='wayland'
export WINIT_UNIX_BACKEND='wayland'
export LIBGL_ALWAYS_SOFTWARE=1
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$root/work"
chmod 700 "$XDG_RUNTIME_DIR"

weston --backend=headless-backend.so --socket="$WAYLAND_DISPLAY" --idle-time=0 --no-config --log="$root/weston.log" &
weston_pid=$!
for i in $(seq 1 100); do
  if [[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]]; then break; fi
  if ! kill -0 "$weston_pid" 2>/dev/null; then cat "$root/weston.log"; exit 1; fi
  sleep 0.1
done
test -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"

bin='./target/debug/jobflick'
"$bin" daemon >"$root/daemon.log" 2>&1 &
daemon_pid=$!
for i in $(seq 1 100); do
  if [[ -S "$XDG_RUNTIME_DIR/jobflick/daemon.sock" ]]; then break; fi
  sleep 0.05
done
test -S "$XDG_RUNTIME_DIR/jobflick/daemon.sock"

cat > "$root/job.yaml" <<EOF
schema: 2
shell: fish
cwd: $root/work
privilege: none
timeout_seconds: 20
commands:
  - printf wayland_clipboard_ok
  - 'true'
EOF
wl-copy --type text/plain < "$root/job.yaml"
wl-paste --no-newline | grep -q 'wayland_clipboard_ok'
queued="$("$bin" submit --clipboard)"
id="$(awk '{print $2}' <<< "$queued")"
test -n "$id"
for i in $(seq 1 100); do
  if "$bin" show "$id" > "$root/report" 2>/dev/null; then break; fi
  sleep 0.1
done
grep -q 'Status: Succeeded' "$root/report"
grep -q 'wayland_clipboard_ok' "$root/report"

# An open HUD remains alive until explicitly closed. Timeout bounds failure.
timeout 8s "$bin" hud > "$root/hud.log" 2>&1 &
hud_pid=$!
sleep 3
if ! kill -0 "$hud_pid" 2>/dev/null; then
  cat "$root/hud.log" >&2
  echo 'Wayland HUD exited before completing the smoke test' >&2
  exit 1
fi
echo 'Real Wayland clipboard submission and egui HUD launch succeeded'
