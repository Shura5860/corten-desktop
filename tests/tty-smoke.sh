#!/usr/bin/env bash
set -euo pipefail

if [[ ! -d /dev/dri ]]; then
  echo "SKIP: no DRM device (/dev/dri)"
  exit 0
fi

if [[ ! -x target/debug/corten-compositor ]]; then
  echo "Build first with: cargo build -p corten-compositor"
  exit 2
fi

if ldd target/debug/corten-compositor 2>/dev/null | grep -q "not found"; then
  echo "SKIP: compositor runtime dependencies are not installed"
  ldd target/debug/corten-compositor 2>/dev/null | grep "not found" || true
  exit 0
fi

log_file="$(mktemp)"
cleanup() {
  if [[ -n "${compositor_pid:-}" ]] && kill -0 "$compositor_pid" 2>/dev/null; then
    kill -TERM "$compositor_pid" 2>/dev/null || true
    wait "$compositor_pid" 2>/dev/null || true
  fi
  rm -f "$log_file"
}
trap cleanup EXIT

RUST_LOG=corten_compositor=info timeout --signal=TERM 35s target/debug/corten-compositor --drm >"$log_file" 2>&1 &
compositor_pid=$!

sleep 2
if ! kill -0 "$compositor_pid" 2>/dev/null; then
  cat "$log_file"
  echo "FAIL: compositor exited before smoke window"
  exit 1
fi

if grep -Eqi "panic|permission denied|failed to initialize drm output" "$log_file"; then
  cat "$log_file"
  echo "FAIL: DRM smoke test found a fatal startup/shutdown error"
  exit 1
fi

kill -TERM "$compositor_pid" 2>/dev/null || true
wait "$compositor_pid" 2>/dev/null || true
if grep -Eqi "panic|permission denied" "$log_file"; then
  cat "$log_file"
  echo "FAIL: DRM shutdown reported a fatal error"
  exit 1
fi

echo "PASS: DRM compositor startup/shutdown smoke test"
