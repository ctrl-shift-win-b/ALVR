#!/usr/bin/env bash
# One-click Mint/Linux start: stop SteamVR/ALVR, wrap vrcompositor (capture layer),
# start dashboard (absolute path), launch SteamVR. No sudo. Safe basenames only for kills.
#
# Usage:
#   ./scripts/restart-alvr-steamvr.sh              # full recycle + launch
#   ./scripts/restart-alvr-steamvr.sh --no-steam   # stop + wrap + dashboard only
#   ./scripts/restart-alvr-steamvr.sh --score      # after launch, optional local score script
#
# Optional env:
#   STEAM_ROOT          Steam library root (auto-detected if unset)
#   ALVR_STREAM_NICE    renice value for vrcompositor/vrserver (default -5)
#   STEAMVR_SETTINGS    path to steamvr.vrsettings (auto-detected if unset)
#   ALVR_PROFILE        summary|frame|detail|off  (default: off)
#                       Written to ~/.config/alvr/profile.env so Steam-launched
#                       vrserver/vrcompositor see it (shell export alone is not enough).
#                       Enable for a measure session: ALVR_PROFILE=summary ./scripts/...
#   ALVR_PROFILE_PATH   JSONL path (default /tmp/alvr-profile.jsonl)
#   ALVR_PROFILE_LOG_MS summary interval ms (default 2000)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=steam-paths.sh
source "${REPO_ROOT}/scripts/steam-paths.sh"
STREAMER="${REPO_ROOT}/build/alvr_streamer_linux"
DRIVER_ROOT="${STREAMER}/lib64/alvr"
WRAPPER="${STREAMER}/libexec/alvr/vrcompositor-wrapper"
DASHBOARD="${STREAMER}/bin/alvr_dashboard"
STEAMVR_BIN="$(alvr_find_steamvr_bin64 || true)"
[[ -n "${STEAMVR_BIN:-}" ]] || { echo "SteamVR bin not found (set STEAM_ROOT)"; exit 1; }
REAL="${STEAMVR_BIN}/vrcompositor.real"
LINK="${STEAMVR_BIN}/vrcompositor"
NAMED_DIR="${STREAMER}/libexec/alvr/steamvr_vrcompositor"
NAMED="${NAMED_DIR}/vrcompositor"
OPENVR_PATHS="${HOME}/.config/openvr/openvrpaths.vrpath"
SESSION_JSON="${HOME}/.config/alvr/session.json"

LAUNCH_STEAM=1
RUN_SCORE=0
for arg in "$@"; do
  case "$arg" in
    --no-steam) LAUNCH_STEAM=0 ;;
    --score) RUN_SCORE=1 ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
  esac
done

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*"; }
die() { log "ERROR: $*"; exit 1; }

_kill_by_name() {
  local sig="${1:-TERM}"
  shift
  local name pid
  for name in "$@"; do
    while read -r pid; do
      [[ -n "$pid" ]] || continue
      kill "-${sig}" "$pid" 2>/dev/null || true
    done < <(pgrep -x "$name" 2>/dev/null || true)
  done
}

stop_all() {
  log "Stopping SteamVR + ALVR dashboard..."
  _kill_by_name TERM vrmonitor vrserver vrcompositor vrstartup vrwebhelper vrcompositor.re alvr_dashboard
  sleep 1
  _kill_by_name KILL vrmonitor vrserver vrcompositor vrstartup vrwebhelper vrcompositor.re alvr_dashboard
  sleep 1
  rm -f "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/alvr-ipc" 2>/dev/null || true
  : > /tmp/alvr-vrcompositor-wrapper.log 2>/dev/null || true
  : > /tmp/alvr-vulkan-layer.log 2>/dev/null || true
}

ensure_build() {
  [[ -x "$DASHBOARD" ]] || die "missing dashboard: $DASHBOARD (build first)"
  [[ -x "$WRAPPER" ]] || die "missing wrapper: $WRAPPER"
  [[ -d "$DRIVER_ROOT" ]] || die "missing driver root: $DRIVER_ROOT"
  [[ -f "$REAL" || -f "$LINK" ]] || die "SteamVR compositor not found under $STEAMVR_BIN"
}

wrap_compositor() {
  mkdir -p "$STEAMVR_BIN" "$NAMED_DIR"
  if [[ -L "$LINK" ]]; then
    rm -f "$LINK"
  elif [[ -f "$LINK" && ! -f "$REAL" ]]; then
    mv "$LINK" "$REAL"
  fi
  [[ -f "$REAL" ]] || die "vrcompositor.real missing — run SteamVR once or re-wrap from dashboard"
  # Basename MUST be "vrcompositor" (not .real) or SteamVR 303 / readiness fails.
  rm -f "$NAMED"
  ln "$REAL" "$NAMED" 2>/dev/null || cp -a "$REAL" "$NAMED"
  chmod +x "$NAMED"
  ln -sfn "$WRAPPER" "$LINK"
  log "wrap: $LINK -> $(readlink "$LINK")"
  log "named: $NAMED (comm will be vrcompositor)"
}

register_driver() {
  python3 - <<PY
import json
from pathlib import Path
p = Path("${OPENVR_PATHS}")
d = json.loads(p.read_text())
driver = Path("${DRIVER_ROOT}").resolve()
ext = [Path(x).resolve() for x in d.get("external_drivers") or []]
if driver not in ext:
    ext.append(driver)
d["external_drivers"] = [str(x) for x in ext]
p.write_text(json.dumps(d, indent=2) + "\n")
print("external_drivers:", d["external_drivers"])
PY
}

touch_session() {
  [[ -f "$SESSION_JSON" ]] || die "missing $SESSION_JSON"
  python3 - <<'PY'
import json
from pathlib import Path
p = Path.home() / ".config/alvr/session.json"
d = json.loads(p.read_text())
d["hard_config_solidified"] = True
o = d.setdefault("openvr_config", {})
o.setdefault("eye_resolution_width", 2144)
o.setdefault("eye_resolution_height", 2144)
o.setdefault("refresh_rate", 90)
if int(o.get("nvenc_quality_preset") or 0) < 1:
    o["nvenc_quality_preset"] = 1
if int(o.get("nvenc_tuning_preset") or 0) < 1:
    o["nvenc_tuning_preset"] = 2
# Prefer stable SS
import os
cfg_s = os.environ.get("STEAMVR_SETTINGS") or ""
if not cfg_s:
    # try common roots
    for root in [
        Path.home() / ".steam/steam",
        Path.home() / ".steam/debian-installation",
        Path.home() / ".local/share/Steam",
    ]:
        c = root / "config/steamvr.vrsettings"
        if c.is_file():
            cfg_s = str(c)
            break
cfg = Path(cfg_s) if cfg_s else Path.home() / ".steam/steam/config/steamvr.vrsettings"
if cfg.is_file():
    s = json.loads(cfg.read_text())
    sv = s.setdefault("steamvr", {})
    if float(sv.get("supersampleScale") or 1) > 1.5:
        sv["supersampleScale"] = 1.5
    sv["disableAsync"] = True
    cfg.write_text(json.dumps(s, indent=2) + "\n")
p.write_text(json.dumps(d, indent=2) + "\n")
print("session ok, solidified=True")
PY
}

# Steam children do not inherit this shell's exports. Write a file the driver and
# vrcompositor-wrapper both load on startup.
ensure_profile_env() {
  local cfg_dir="${XDG_CONFIG_HOME:-$HOME/.config}/alvr"
  local f="${cfg_dir}/profile.env"
  mkdir -p "$cfg_dir"
  local level="${ALVR_PROFILE:-off}"
  local path="${ALVR_PROFILE_PATH:-/tmp/alvr-profile.jsonl}"
  local logms="${ALVR_PROFILE_LOG_MS:-2000}"
  cat >"$f" <<EOF
# Written by restart-alvr-steamvr.sh — read by driver + vrcompositor-wrapper
# (Steam does not inherit shell ALVR_PROFILE.) Profiling is OFF by default.
#   ALVR_PROFILE=summary ./scripts/restart-alvr-steamvr.sh
#   ALVR_PROFILE=frame ./scripts/restart-alvr-steamvr.sh
ALVR_PROFILE=${level}
ALVR_PROFILE_PATH=${path}
ALVR_PROFILE_LOG_MS=${logms}
EOF
  # Fresh JSONL per launch so sessions are easy to read
  if [[ "${level}" != "off" && "${level}" != "0" && "${level}" != "false" ]]; then
    : >"${path}" 2>/dev/null || true
    # capture sibling
    if [[ "${path}" == *.jsonl ]]; then
      : >"${path%.jsonl}-capture.jsonl" 2>/dev/null || true
    else
      : >"${path}-capture.jsonl" 2>/dev/null || true
    fi
  fi
  log "profile.env: ALVR_PROFILE=${level} path=${path} (cfg=${f})"
}

start_dashboard() {
  # Absolute path required so driver HmdDriverFactory path check passes.
  # Also pass ALVR_PROFILE* for any code that runs in the dashboard process.
  # shellcheck disable=SC1090
  set -a
  # shellcheck source=/dev/null
  [[ -f "${XDG_CONFIG_HOME:-$HOME/.config}/alvr/profile.env" ]] && . "${XDG_CONFIG_HOME:-$HOME/.config}/alvr/profile.env"
  set +a
  nohup "$DASHBOARD" >/tmp/alvr-dashboard.log 2>&1 &
  echo $! > /tmp/alvr-dashboard.pid
  sleep 1
  log "dashboard pid=$(cat /tmp/alvr-dashboard.pid) path=$DASHBOARD"
}

launch_steamvr() {
  local nv_icd="/usr/share/vulkan/icd.d/nvidia_icd.json"
  [[ -f "$nv_icd" ]] || nv_icd="/usr/share/vulkan/icd.d/nvidia_icd.x86_64.json"
  export __GLX_VENDOR_LIBRARY_NAME=nvidia
  export __NV_PRIME_RENDER_OFFLOAD=1
  [[ -f "$nv_icd" ]] && export VK_DRIVER_FILES="$nv_icd"
  export LD_LIBRARY_PATH="/lib/x86_64-linux-gnu:${DRIVER_ROOT}/bin/linux64:${LD_LIBRARY_PATH:-}"
  if command -v steam >/dev/null 2>&1; then
    steam steam://rungameid/250820 >/tmp/alvr-steamvr-launch.log 2>&1 &
    log "launched steam://rungameid/250820"
  else
    die "steam not in PATH"
  fi
}

# ---- main ----
log "repo=$REPO_ROOT"
ensure_build
stop_all
touch_session
ensure_profile_env
register_driver
wrap_compositor
start_dashboard

# Best-effort CPU scheduling: prefer SteamVR capture path over background noise.
# Negative nice may fail without CAP_SYS_NICE; we try and log. Not a GPU priority API.
raise_stream_priorities() {
  local nice_val="${ALVR_STREAM_NICE:--5}"
  local names=(vrcompositor vrserver vrmonitor alvr_dashboard)
  local name pids pid
  for name in "${names[@]}"; do
    pids="$(pgrep -x "$name" 2>/dev/null || true)"
    [[ -n "$pids" ]] || continue
    for pid in $pids; do
      if renice -n "$nice_val" -p "$pid" >/dev/null 2>&1; then
        log "renice $nice_val pid=$pid ($name)"
      else
        # Fallback: only de-nice toward more favorable if we own the process
        if renice -n -1 -p "$pid" >/dev/null 2>&1; then
          log "renice -1 pid=$pid ($name) [fallback]"
        else
          log "renice skipped pid=$pid ($name) — need CAP_SYS_NICE or run as same user with rights"
        fi
      fi
    done
  done
}

if (( LAUNCH_STEAM )); then
  launch_steamvr
  log "Waiting for compositor..."
  for i in $(seq 1 40); do
    if pgrep -x vrcompositor >/dev/null 2>&1; then
      pid=$(pgrep -x vrcompositor | head -1)
      log "vrcompositor pid=$pid comm=$(cat /proc/$pid/comm 2>/dev/null)"
      break
    fi
    sleep 0.5
  done
  # Give vrserver a moment to appear, then raise priorities.
  sleep 1
  raise_stream_priorities
fi

log "Done. Connect headset client when SteamVR shows Ready."
log "Logs: /tmp/alvr-dashboard.log  /tmp/alvr-vrcompositor-wrapper.log"
log "      SteamVR logs: $(alvr_find_steamvr_logs 2>/dev/null || echo unset)"
log "Profile: ~/.config/alvr/profile.env → /tmp/alvr-profile.jsonl (driver)"
log "         look for 'ALVR profiling: enabled' in SteamVR logs/vrserver.txt"

if (( RUN_SCORE )); then
  if [[ -x "${REPO_ROOT}/scripts/alvr-auto-rca.sh" ]]; then
    log "Running optional auto score in 22s..."
    sleep 22
    "${REPO_ROOT}/scripts/alvr-auto-rca.sh" --no-launch --keep || true
  else
    log "--score requested but scripts/alvr-auto-rca.sh not present; skipped"
  fi
fi
