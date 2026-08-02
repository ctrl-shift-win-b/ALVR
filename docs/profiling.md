# ALVR pipeline profiling (Linux → Vision Pro)

Low-overhead instrumentation for the **server-side** stream path used by this Mint fork
(capture layer → encoder → TCP → App Store AVP). The headset client is a black box; we only
see what arrives in `ClientStatistics`.

## Enable

| Variable | Values | Effect |
|----------|--------|--------|
| `ALVR_PROFILE` | unset / `0` / `off` | **Disabled** (default). One atomic check per span site. |
| `ALVR_PROFILE` | `1` / `summary` | Aggregate **p50/p95/p99** every interval → ALVR log + JSONL summary |
| `ALVR_PROFILE` | `frame` | Summary + **per-span JSONL** lines |
| `ALVR_PROFILE` | `detail` | Same as `frame` (reserved for finer marks) |
| `ALVR_PROFILE_PATH` | path | Driver JSONL (default `/tmp/alvr-profile.jsonl`) |
| `ALVR_PROFILE_LOG_MS` | ms | Summary interval (default `2000`) |
| `ALVR_PROFILE_RING` | power-of-two size | In-memory ring capacity (default `65536`) |

**Capture process** (vrcompositor + `VK_LAYER_ALVR_capture`) writes a sibling file when
`ALVR_PROFILE` is `frame`/`detail`:

- default: `/tmp/alvr-profile-capture.jsonl`
- stamps `present_packet.submit_ns` always when connected (even in `summary`) so the driver
  can measure **IPC present delay**.

### One-click with profiling

```bash
export ALVR_PROFILE=summary
# optional:
# export ALVR_PROFILE=frame
# export ALVR_PROFILE_PATH=/tmp/alvr-profile.jsonl

./scripts/restart-alvr-steamvr.sh
```

Ensure the env is visible to **both** the dashboard/driver **and** SteamVR/`vrcompositor`
(wrapper inherits the environment from how you launch SteamVR). If capture JSONL is missing,
the compositor did not see `ALVR_PROFILE`.

### Tracy (optional lab build)

```bash
cargo build -p alvr_server_openvr --features alvr_server_core/trace-performance --release
# or xtask profiling flag if used in your build scripts
```

Starts a Tracy client and opens zones for spans when `ALVR_PROFILE` is also enabled.
Use the [Tracy](https://github.com/wolfpld/tracy) profiler UI on the same machine.

## Stages (frame timeline)

| ID | Name | Where |
|----|------|--------|
| 0 | `tracking_rx` | Tracking packet deserialized (`server_core`) |
| 1 | `pose_publish` | OpenVR pose push (`server_openvr` event loop) |
| 2 | `present_submit` | Vulkan layer UDS write (capture process) |
| 3 | `present_recv` | `CEncoder` UDS read |
| 15 | `ipc_present_delay` | `submit_ns` → driver recv (cross-process) |
| 4 | `stamp_pick` | `PoseHistory::PickEncodeStamp` |
| 5 | `render_gpu` | `Renderer::Render` |
| 6 | `encode_push` | `EncodePipeline::PushFrame` |
| 7 | `encode_get` | `EncodePipeline::GetEncoded` |
| 8 | `nal_parse` | `ParseFrameNals` |
| 9 | `ffi_copy` | C++ → Rust `to_vec` in `VideoSend` |
| 10 | `channel_enqueue` | `try_send` video channel |
| 11 | `channel_dequeue` | mark when network thread takes packet |
| 12 | `stream_copy` | NAL `copy_from_slice` into stream buffer |
| 13 | `tcp_send` | shard + TCP `write_all` |
| 14 | `client_stats_match` | match client residual stats |

## Output formats

### Log line (every `ALVR_PROFILE_LOG_MS`)

```text
PROFILE summary frames~... | encode_push: p50=... p95=... p99=...us n=... | tcp_send: ...
```

### JSONL

- `{"type":"header",...}` on start  
- `{"type":"span","stage":"encode_push","frame_id":...,"start_ns":...,"end_ns":...,"dur_us":...,"extra":...}`  
- `{"type":"summary","stages":[{"stage":"...","p50_us":...,"p95_us":...,"p99_us":...},...]}`

`start_ns` / `end_ns` are **CLOCK_MONOTONIC** nanoseconds (Linux), comparable across the
driver and compositor processes on the same host.

### Quick analysis

```bash
# Top stages by p95 from last summary line
tail -n 5 /tmp/alvr-profile.jsonl

# Mean encode_push duration (frame mode)
jq -r 'select(.type=="span" and .stage=="encode_push") | .dur_us' /tmp/alvr-profile.jsonl \
  | awk '{s+=$1;n++} END{print s/n, "us mean over", n}'
```

## Interpreting hotspots (typical Linux + AVP / TCP)

| High stage | Likely cause | Next optimization ideas |
|------------|--------------|-------------------------|
| `ipc_present_delay` | Encoder slower than compositor; UDS backlog | Overlap encode; drop older presents (already partially done) |
| `render_gpu` | FFR/color compute | Reduce passes; resolution |
| `encode_push` / `encode_get` | NVENC/VAAPI/SW encode + transfer | Codec settings; Vulkan→CUDA path |
| `ffi_copy` + `stream_copy` | Extra full-NAL copies | Zero-copy encode → socket buffer |
| `tcp_send` | Blocking TCP / congestion | Buffer sizes; writev; bitrate |
| `stamp_pick` | PoseHistory lock + scan | Lock-free ring / O(1) lag index |

Dashboard **Graph** stats remain the coarse motion-to-photon view; this profiler is the
**fine-grained server breakdown**.

## Overhead budget

| Mode | Target |
|------|--------|
| Off | Negligible (atomic load + branch) |
| `summary` | &lt; ~0.1 ms/frame aggregate |
| `frame` | Higher (JSONL append per span) — use short sessions |

## Code map

- Rust crate: `alvr/profiling`
- C++ driver hooks: `alvr/server_openvr/cpp/ALVR-common/alvr_profile.h`
- Capture: `alvr/vulkan_layer/util/alvr_profile_capture.h`
- Protocol stamp: `present_packet.submit_ns` in `protocol.h`
