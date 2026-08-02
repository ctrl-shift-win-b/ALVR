# Linux portable packaging / sharing binaries

Notes for a later packaging pass. Goal: share a working streamer build with as few files and host packages as possible—without pretending the whole stack can be one static binary.

**Status (2026-07):** Not implemented. Current share path is the built tree / `package-streamer` tarball. This document captures constraints and recommended next steps.

## What “launcher” vs “streamer” means

| Artifact | Role | Single-file realistic? |
|----------|------|------------------------|
| **`alvr_launcher`** | Small app that downloads/updates a streamer package | Closest to a small standalone binary |
| **`alvr_dashboard`** | UI + SteamVR control | One executable, but needs the driver tree next to it |
| **Streamer tree** | What actually streams (driver, Vulkan layer, wrapper, …) | **Must** remain multiple files |

What users need to **run** streaming is the streamer tree, not only the launcher crate.

Upstream packaging already exists:

```bash
cargo xtask package-streamer --platform linux   # → build/ + .tar.gz
cargo xtask package-launcher                    # → launcher archive
```

## Current streamer layout (already fairly lean)

Typical local layout after `build-streamer` / package (~12 files, ~400 MB class on a release build):

```text
bin/alvr_dashboard
lib64/alvr/bin/linux64/driver_alvr_server.so
lib64/alvr/bin/linux64/libopenvr_api.so
lib64/alvr/driver.vrdrivermanifest
lib64/libalvr_vulkan_layer.so
libexec/alvr/vrcompositor-wrapper
libexec/alvr/alvr_drm_lease_shim.so
libexec/alvr/steamvr_vrcompositor/...   # optional stub path
libexec/alvr/firewall helpers
share/vulkan/explicit_layer.d/alvr_x86_64.json
```

Dashboard and driver dominate size (~200 MB + ~150 MB class). Much of FFmpeg/encode logic is already inside the driver `.so`; remaining `ldd` deps are host libraries (PipeWire, Vulkan, X11/VA, x264, glibc, …).

## Why a true single static binary is not viable

1. **SteamVR loads an OpenVR driver plugin**  
   `driver_alvr_server.so` is `dlopen`’d via the OpenVR driver path. It cannot be fused into the dashboard process and still register as a SteamVR driver.

2. **Vulkan layer is a separate load**  
   `libalvr_vulkan_layer.so` + JSON manifest are discovered by the Vulkan loader / compositor. Separate files by design. Required for the Linux capture path used on this branch.

3. **Compositor wrapper is another process**  
   `vrcompositor-wrapper` (and related shim) is invoked as a different binary path SteamVR runs. Not optional for this capture stack.

4. **Host GPU and session stacks must stay dynamic**  
   Even with aggressive static linking, a working install still needs at runtime:
   - Vulkan ICD (NVIDIA/AMD/Intel driver)
   - PipeWire / Pulse session (game audio + mic; `pactl` for auto-switch)
   - glibc (full static glibc is a bad idea; musl is a different ABI ecosystem)
   - Often libva / DRM / X11 for encode-related paths  

   Baking those into one binary is wrong and still breaks on mismatched GPUs.

5. **Size**  
   Dashboard + driver are already huge. Statically swallowing more system libs mostly grows the download; it does not remove the multi-file SteamVR/Vulkan model.

## What *is* realistic

| Goal | Feasible? | Approach |
|------|-----------|----------|
| Share a portable package with few files | **Yes** | Current tree / `package-streamer` tarball |
| Fewer host `apt` packages | **Yes** | Private `lib/` next to the driver + `$ORIGIN` rpath (e.g. bundle `libx264` and other pure userspace deps) |
| One download the user double-clicks | **Yes** | AppImage / self-extracting archive that **contains** the multi-file tree |
| Small “get ALVR” binary | **Yes** | `package-launcher` / `alvr_launcher` downloads the streamer |
| One fully static binary: dashboard + driver + audio + NVENC + SteamVR | **No** | Plugin + layer + GPU/session APIs forbid it |

### Partial static / bundling (high effort, partial win)

| Technique | Effect |
|-----------|--------|
| Keep FFmpeg / encode bits inside `driver_alvr_server.so` | Already the main size/path; reduces some DLL hell |
| Bundle private `.so`s + `$ORIGIN` rpath | Host needs fewer matching package versions |
| `crt-static` / musl for pure Rust tools | Fine for helpers; fights OpenVR, PipeWire, Vulkan, egui for the real stack |
| Full static everything | Not viable for the streamer |

**Always leave dynamic:** Vulkan ICD, PipeWire/Pulse, libc, GPU driver stack.

## Recommended later work: `package-portable-linux` (sketch)

Not built yet. Intended direction if we want a shareable Mint-class artifact:

1. Build with **distribution** (or release) profile via existing `build_streamer` / `package_streamer`.
2. Copy selected non-GPU, non-session libraries into e.g. `lib64/alvr/bin/linux64/lib/` (or a sibling `lib/`).
3. Set **rpath** `$ORIGIN` / `$ORIGIN/lib` on `driver_alvr_server.so` and `alvr_dashboard` so those private libs resolve without host packages.
4. Ship:
   - streamer tree
   - `scripts/restart-alvr-steamvr.sh` (or a trimmed start script)
   - short README: host requirements + SteamVR launch options
5. Archive as `.tar.gz` and optionally wrap as **AppImage** for one-file distribution (runtime still expands/mounts the multi-file layout).

### Host requirements to document on the package

- Linux Mint 22 / Ubuntu 24.04-class glibc (or clearly state the build distro)
- Working **NVIDIA** (or other) **Vulkan** driver
- **SteamVR** installed
- **PipeWire** + `pactl` (`pulseaudio-utils`) for automatic audio routing
- Hybrid NVIDIA: SteamVR launch options with `vrmonitor.sh` + vendor env (see main README)

### Do not claim

- “Zero dependencies”
- “Works on any Linux without SteamVR/GPU drivers”
- “Single static binary includes the OpenVR driver and Vulkan layer as one process”

## AppImage / Flatpak notes

- Upstream already has **Flatpak-related** bits under `alvr/xtask/flatpak/` and wiki guidance for SteamVR-through-Flatpak. Portable packaging should not fight that model blindly; sandbox paths and SteamVR driver registration are the hard parts.
- **AppImage** is a good UX for “one file to pass a friend,” but SteamVR still needs a stable on-disk driver path (extract to `~/.local/share/ALVR-streamer` or similar, then register).

## Verification checklist (when implementing)

- [ ] Fresh user account / clean VM with only SteamVR + GPU driver + PipeWire
- [ ] Unpack package → start script → dashboard → SteamVR Ready
- [ ] Stream starts (video + LSW path)
- [ ] Auto audio: defaults → `ALVR Audio` / `ALVR Microphone`; restore on disconnect; re-apply after client reconnect
- [ ] `ldd driver_alvr_server.so` shows no unexpected `not found` for bundled libs; Vulkan/PipeWire still from host

## Related commands (today)

```bash
# Local streamer tree used by one-click script
cargo xtask build-streamer --platform linux --release

# Official-style archive of that tree
cargo xtask package-streamer --platform linux

# Small launcher that fetches a streamer (not a full offline streamer)
cargo xtask package-launcher

# This fork’s one-click
./scripts/restart-alvr-steamvr.sh
```

## Decision summary

**Share a directory or tarball of the streamer (~dozen files).** Optionally bundle userspace `.so`s with rpath and/or wrap in AppImage. Do **not** invest in full static linking of the SteamVR driver stack; invest in packaging, rpath, and clear host requirements instead.
