#!/usr/bin/env bash
# Build ALVR streamer (dashboard + OpenVR driver) natively on Linux Mint / Ubuntu / Debian.
#
# Usage:
#   ./scripts/build-streamer-linux.sh              # full release streamer
#   ./scripts/build-streamer-linux.sh --deps        # only install apt packages (needs sudo)
#   ./scripts/build-streamer-linux.sh --no-nvidia   # AMD/Intel only (skip CUDA/NvEnc deps prep)
#   ./scripts/build-streamer-linux.sh --launcher    # also build ALVR Launcher
#   ./scripts/build-streamer-linux.sh --debug       # debug build (faster compile, slower runtime)
#   ./scripts/build-streamer-linux.sh --skip-deps-prep  # skip cargo xtask prepare-deps (rebuild only)
#
# Output:
#   build/alvr_streamer_linux/
#     bin/alvr_dashboard
#     lib64/driver_alvr_server.so  (etc.)
#
# Run:
#   ./build/alvr_streamer_linux/bin/alvr_dashboard

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

NO_NVIDIA=0
WITH_LAUNCHER=0
RELEASE=1
SKIP_DEPS_PREP=0
INSTALL_DEPS_ONLY=0

for arg in "$@"; do
  case "$arg" in
    --no-nvidia) NO_NVIDIA=1 ;;
    --launcher) WITH_LAUNCHER=1 ;;
    --debug) RELEASE=0 ;;
    --skip-deps-prep) SKIP_DEPS_PREP=1 ;;
    --deps) INSTALL_DEPS_ONLY=1 ;;
    -h|--help)
      sed -n '2,22p' "$0"
      exit 0
      ;;
    *)
      echo "Unknown option: $arg" >&2
      exit 1
      ;;
  esac
done

# ---------------------------------------------------------------------------
# System packages (Mint 22 / Ubuntu 24.04 and similar)
# ---------------------------------------------------------------------------
APT_PACKAGES=(
  build-essential
  pkg-config
  git
  curl
  unzip
  cmake
  clang
  libclang-dev
  nasm
  yasm
  libssl-dev
  libasound2-dev
  libjack-dev
  libgtk-3-dev
  libvulkan-dev
  # Note: package "vulkan-headers" is not always available on Mint/Ubuntu;
  # libvulkan-dev already provides Vulkan SDK headers.
  libunwind-dev
  libx264-dev
  libx265-dev
  libxcb-render0-dev
  libxcb-shape0-dev
  libxcb-xfixes0-dev
  libspeechd-dev
  libxkbcommon-dev
  libdrm-dev
  libva-dev
  libxrandr-dev
  libpipewire-0.3-dev
  libspa-0.2-dev
  pulseaudio-utils
)

install_apt_deps() {
  echo "==> Installing apt packages (sudo required)..."
  sudo apt-get update
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "${APT_PACKAGES[@]}"
  echo "==> apt packages installed."
}

if [[ "$INSTALL_DEPS_ONLY" -eq 1 ]]; then
  install_apt_deps
  exit 0
fi

# Soft check: warn if key tools missing
need_pkg=0
for bin in clang pkg-config git cmake nasm; do
  if ! command -v "$bin" >/dev/null 2>&1; then
    echo "Missing tool: $bin"
    need_pkg=1
  fi
done
if [[ ! -e /usr/include/pipewire-0.3/pipewire/pipewire.h ]] && \
   [[ ! -e /usr/include/pipewire/pipewire.h ]]; then
  echo "Missing PipeWire headers (libpipewire-0.3-dev)"
  need_pkg=1
fi
if [[ "$need_pkg" -eq 1 ]]; then
  echo
  echo "Install system dependencies first:"
  echo "  ./scripts/build-streamer-linux.sh --deps"
  echo "  # or: sudo apt install ${APT_PACKAGES[*]}"
  exit 1
fi

# ---------------------------------------------------------------------------
# Rust
# ---------------------------------------------------------------------------
if [[ -f "$HOME/.cargo/env" ]]; then
  # shellcheck source=/dev/null
  source "$HOME/.cargo/env"
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "==> Installing Rust via rustup..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  # shellcheck source=/dev/null
  source "$HOME/.cargo/env"
fi

# Workspace requires a recent rustc (see Cargo.toml rust-version)
RUST_MIN=1.92
RUST_HAVE="$(rustc --version | awk '{print $2}')"
echo "==> rustc $RUST_HAVE  cargo $(cargo --version | awk '{print $2}')"

# ---------------------------------------------------------------------------
# Submodules (OpenVR headers)
# ---------------------------------------------------------------------------
echo "==> Ensuring git submodules..."
git submodule update --init --recursive

# ---------------------------------------------------------------------------
# External deps (FFmpeg / Vulkan headers / etc. via xtask)
# ---------------------------------------------------------------------------
PREPARE_FLAGS=(--platform linux)
if [[ "$NO_NVIDIA" -eq 1 ]]; then
  PREPARE_FLAGS+=(--no-nvidia)
  echo "==> prepare-deps: NVIDIA / CUDA path disabled"
else
  echo "==> prepare-deps: NVIDIA path enabled (needs working CUDA/nvcc for NvEnc build bits)"
  echo "    If you only have AMD/Intel, re-run with --no-nvidia"
fi

if [[ "$SKIP_DEPS_PREP" -eq 0 ]]; then
  echo "==> cargo xtask prepare-deps ${PREPARE_FLAGS[*]}"
  cargo xtask prepare-deps "${PREPARE_FLAGS[@]}"
else
  echo "==> Skipping prepare-deps"
fi

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
BUILD_FLAGS=(--platform linux)
if [[ "$RELEASE" -eq 1 ]]; then
  BUILD_FLAGS+=(--release)
fi

echo "==> cargo xtask build-streamer ${BUILD_FLAGS[*]}"
cargo xtask build-streamer "${BUILD_FLAGS[@]}"

if [[ "$WITH_LAUNCHER" -eq 1 ]]; then
  echo "==> cargo xtask build-launcher ${BUILD_FLAGS[*]}"
  cargo xtask build-launcher "${BUILD_FLAGS[@]}"
fi

echo
echo "========================================================================"
echo " Build complete"
echo "========================================================================"
echo " Streamer:  $REPO_ROOT/build/alvr_streamer_linux/"
echo " Dashboard: $REPO_ROOT/build/alvr_streamer_linux/bin/alvr_dashboard"
if [[ "$WITH_LAUNCHER" -eq 1 ]]; then
  echo " Launcher:  $REPO_ROOT/build/alvr_launcher_linux/"
fi
echo
echo " Run (or use one-click helper):"
echo "   $REPO_ROOT/build/alvr_streamer_linux/bin/alvr_dashboard"
echo "   $REPO_ROOT/scripts/restart-alvr-steamvr.sh"
echo
echo " First use: Hard Config → Apply AVP defaults → Solidify & Launch SteamVR"
echo "            (or: ./scripts/restart-alvr-steamvr.sh after solidify)"
echo " README:    see Mint Linux preamble at top of README.md"
echo "========================================================================"
