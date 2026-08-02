//! ALVR pipeline profiling.
//!
//! ## Runtime (always compiled, zero-ish cost when off)
//! - `ALVR_PROFILE=0|off` — disabled (default)
//! - `ALVR_PROFILE=1|summary` — aggregate p50/p95/p99 every interval, no per-frame JSONL
//! - `ALVR_PROFILE=frame` — summary + per-frame span lines to JSONL
//! - `ALVR_PROFILE=detail` — frame + extra marks (mutex waits, etc.)
//! - `ALVR_PROFILE_PATH` — JSONL output path (default `/tmp/alvr-profile.jsonl`)
//! - `ALVR_PROFILE_LOG_MS` — summary log interval in ms (default `2000`)
//! - `ALVR_PROFILE_RING` — ring capacity power-of-two (default `65536`)
//!
//! ## Compile-time Tracy
//! Build with feature `tracy` (wired as `alvr_server_core/trace-performance`) for Tracy zones.
//! Env ring buffer still works independently.

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::Serialize;
use std::{
    fs::OpenOptions,
    io::Write,
    sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

// ---------------------------------------------------------------------------
// Stages (stable IDs — keep in sync with C++ alvr_profile.h)
// ---------------------------------------------------------------------------

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    TrackingRx = 0,
    PosePublish = 1,
    PresentSubmit = 2, // capture process (also written standalone by layer)
    PresentRecv = 3,
    StampPick = 4,
    RenderGpu = 5,
    EncodePush = 6,
    EncodeGet = 7,
    NalParse = 8,
    FfiCopy = 9,
    ChannelEnqueue = 10,
    ChannelDequeue = 11,
    StreamCopy = 12,
    TcpSend = 13,
    ClientStatsMatch = 14,
    IpcPresentDelay = 15, // submit_ns → present_recv
    PoseHistoryLock = 16,
    /// Catch-all / custom
    Other = 255,
}

impl Stage {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::TrackingRx,
            1 => Self::PosePublish,
            2 => Self::PresentSubmit,
            3 => Self::PresentRecv,
            4 => Self::StampPick,
            5 => Self::RenderGpu,
            6 => Self::EncodePush,
            7 => Self::EncodeGet,
            8 => Self::NalParse,
            9 => Self::FfiCopy,
            10 => Self::ChannelEnqueue,
            11 => Self::ChannelDequeue,
            12 => Self::StreamCopy,
            13 => Self::TcpSend,
            14 => Self::ClientStatsMatch,
            15 => Self::IpcPresentDelay,
            16 => Self::PoseHistoryLock,
            _ => Self::Other,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::TrackingRx => "tracking_rx",
            Self::PosePublish => "pose_publish",
            Self::PresentSubmit => "present_submit",
            Self::PresentRecv => "present_recv",
            Self::StampPick => "stamp_pick",
            Self::RenderGpu => "render_gpu",
            Self::EncodePush => "encode_push",
            Self::EncodeGet => "encode_get",
            Self::NalParse => "nal_parse",
            Self::FfiCopy => "ffi_copy",
            Self::ChannelEnqueue => "channel_enqueue",
            Self::ChannelDequeue => "channel_dequeue",
            Self::StreamCopy => "stream_copy",
            Self::TcpSend => "tcp_send",
            Self::ClientStatsMatch => "client_stats_match",
            Self::IpcPresentDelay => "ipc_present_delay",
            Self::PoseHistoryLock => "pose_history_lock",
            Self::Other => "other",
        }
    }
}

// ---------------------------------------------------------------------------
// Level
// ---------------------------------------------------------------------------

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Off = 0,
    Summary = 1,
    Frame = 2,
    Detail = 3,
}

static LEVEL: AtomicU8 = AtomicU8::new(0);
static INIT_DONE: AtomicU8 = AtomicU8::new(0);

#[inline(always)]
pub fn level() -> Level {
    match LEVEL.load(Ordering::Relaxed) {
        1 => Level::Summary,
        2 => Level::Frame,
        3 => Level::Detail,
        _ => Level::Off,
    }
}

#[inline(always)]
pub fn enabled() -> bool {
    LEVEL.load(Ordering::Relaxed) != 0
}

#[inline(always)]
pub fn is_frame() -> bool {
    LEVEL.load(Ordering::Relaxed) >= Level::Frame as u8
}

#[inline(always)]
pub fn is_detail() -> bool {
    LEVEL.load(Ordering::Relaxed) >= Level::Detail as u8
}

// ---------------------------------------------------------------------------
// Clock: CLOCK_MONOTONIC nanoseconds (matches C++ steady_clock epoch on Linux)
// ---------------------------------------------------------------------------

#[inline(always)]
pub fn now_ns() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let mut ts = libc_timespec();
        // SAFETY: clock_gettime with stack timespec
        unsafe {
            clock_gettime_mono(&mut ts);
        }
        (ts.tv_sec as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(ts.tv_nsec as u64)
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Fallback: Instant is not comparable across processes; fine for same-process spans.
        static START: Lazy<Instant> = Lazy::new(Instant::now);
        START.elapsed().as_nanos() as u64
    }
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[cfg(target_os = "linux")]
#[inline(always)]
fn libc_timespec() -> Timespec {
    Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    }
}

#[cfg(target_os = "linux")]
#[inline(always)]
unsafe fn clock_gettime_mono(ts: *mut Timespec) {
    // CLOCK_MONOTONIC = 1 on Linux
    extern "C" {
        fn clock_gettime(clk_id: i32, tp: *mut Timespec) -> i32;
    }
    let _ = clock_gettime(1, ts);
}

// ---------------------------------------------------------------------------
// Ring buffer
// ---------------------------------------------------------------------------

const DEFAULT_RING: usize = 65536;
const STAGE_COUNT: usize = 17;
const AGG_CAP: usize = 512;

#[derive(Clone, Copy, Debug, Default)]
#[allow(dead_code)] // ring retained for future offline dump / detail tools
struct Record {
    stage: u8,
    _pad: [u8; 3],
    thread_hint: u32,
    frame_id: u64,
    start_ns: u64,
    end_ns: u64,
    extra: u64,
}

struct Agg {
    samples: [u32; AGG_CAP], // duration microseconds, capped
    write: usize,
    count: usize,
}

impl Agg {
    const fn new() -> Self {
        Self {
            samples: [0; AGG_CAP],
            write: 0,
            count: 0,
        }
    }

    fn push_us(&mut self, us: u32) {
        self.samples[self.write % AGG_CAP] = us;
        self.write = self.write.wrapping_add(1);
        self.count = (self.count + 1).min(AGG_CAP);
    }

    fn percentiles_us(&self) -> (u32, u32, u32, u32) {
        if self.count == 0 {
            return (0, 0, 0, 0);
        }
        let mut tmp: Vec<u32> = self.samples[..self.count].to_vec();
        tmp.sort_unstable();
        let p = |q: f32| -> u32 {
            let idx = ((tmp.len() as f32 - 1.0) * q).round() as usize;
            tmp[idx.min(tmp.len() - 1)]
        };
        let sum: u64 = tmp.iter().map(|&x| x as u64).sum();
        let mean = (sum / tmp.len() as u64) as u32;
        (mean, p(0.50), p(0.95), p(0.99))
    }
}

struct State {
    ring: Vec<Record>,
    mask: usize,
    write_idx: AtomicUsize,
    dropped: AtomicU64,
    aggs: [Agg; STAGE_COUNT],
    path: String,
    log_interval: Duration,
    last_summary: Instant,
    frames_seen: u64,
}

impl State {
    fn new(ring_pow2: usize, path: String, log_interval: Duration) -> Self {
        let cap = ring_pow2.next_power_of_two().max(1024);
        Self {
            ring: vec![Record::default(); cap],
            mask: cap - 1,
            write_idx: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
            aggs: std::array::from_fn(|_| Agg::new()),
            path,
            log_interval,
            last_summary: Instant::now(),
            frames_seen: 0,
        }
    }
}

static STATE: Lazy<Mutex<Option<State>>> = Lazy::new(|| Mutex::new(None));

/// Call once at process start (safe to call repeatedly).
pub fn init_from_env() {
    if INIT_DONE.swap(1, Ordering::SeqCst) == 1 {
        // Already initialized; still allow re-read if was off? Keep first init only.
        return;
    }

    let raw = std::env::var("ALVR_PROFILE").unwrap_or_default();
    let lvl = parse_level(&raw);
    LEVEL.store(lvl as u8, Ordering::SeqCst);

    if lvl == Level::Off {
        log::info!("ALVR profiling: off (set ALVR_PROFILE=summary|frame|detail to enable)");
        return;
    }

    let path = std::env::var("ALVR_PROFILE_PATH")
        .unwrap_or_else(|_| "/tmp/alvr-profile.jsonl".to_string());
    let log_ms: u64 = std::env::var("ALVR_PROFILE_LOG_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000);
    let ring: usize = std::env::var("ALVR_PROFILE_RING")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_RING);

    *STATE.lock() = Some(State::new(
        ring,
        path.clone(),
        Duration::from_millis(log_ms),
    ));

    // Touch JSONL with a header line
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(
            f,
            r#"{{"type":"header","level":"{}","path":"{}","pid":{},"ts_ns":{}}}"#,
            level_name(lvl),
            path,
            std::process::id(),
            now_ns()
        );
    }

    log::warn!(
        "ALVR profiling: enabled level={} path={} log_every={}ms ring={}",
        level_name(lvl),
        path,
        log_ms,
        ring.next_power_of_two().max(1024)
    );

    // Background flusher for summary lines (and periodic ring drain for frame mode)
    thread::Builder::new()
        .name("alvr-profile".into())
        .spawn(|| flush_loop())
        .ok();

    #[cfg(feature = "tracy")]
    {
        // Ensure Tracy client is linked/active
        tracy_client::Client::start();
        log::warn!("ALVR profiling: Tracy client started (feature tracy)");
    }
}

fn parse_level(raw: &str) -> Level {
    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "0" | "off" | "false" | "no" => Level::Off,
        "1" | "summary" | "on" | "true" | "yes" => Level::Summary,
        "2" | "frame" => Level::Frame,
        "3" | "detail" => Level::Detail,
        _ => Level::Off,
    }
}

fn level_name(l: Level) -> &'static str {
    match l {
        Level::Off => "off",
        Level::Summary => "summary",
        Level::Frame => "frame",
        Level::Detail => "detail",
    }
}

// ---------------------------------------------------------------------------
// Record API
// ---------------------------------------------------------------------------

#[inline(always)]
pub fn record(stage: Stage, frame_id: u64, start_ns: u64, end_ns: u64, extra: u64) {
    if !enabled() {
        return;
    }
    record_inner(stage as u8, frame_id, start_ns, end_ns, extra);
}

fn record_inner(stage: u8, frame_id: u64, start_ns: u64, end_ns: u64, extra: u64) {
    let dur = end_ns.saturating_sub(start_ns);
    let tid = thread_id_hint();

    // Hold the state lock only for ring + aggregates; do file IO after unlock.
    let frame_line = {
        let mut guard = STATE.lock();
        let Some(st) = guard.as_mut() else {
            return;
        };

        let idx = st.write_idx.fetch_add(1, Ordering::Relaxed);
        let slot = idx & st.mask;
        st.ring[slot] = Record {
            stage,
            _pad: [0; 3],
            thread_hint: tid,
            frame_id,
            start_ns,
            end_ns,
            extra,
        };

        let si = stage as usize;
        if si < STAGE_COUNT {
            let us = (dur / 1000).min(u32::MAX as u64) as u32;
            st.aggs[si].push_us(us);
        }

        if stage == Stage::ClientStatsMatch as u8 || stage == Stage::EncodeGet as u8 {
            st.frames_seen = st.frames_seen.wrapping_add(1);
        }

        if is_frame() {
            Some((
                st.path.clone(),
                JsonSpan {
                    type_: "span",
                    stage: Stage::from_u8(stage).name(),
                    stage_id: stage,
                    frame_id,
                    start_ns,
                    end_ns,
                    dur_us: dur / 1000,
                    extra,
                    tid,
                },
            ))
        } else {
            None
        }
    };

    if let Some((path, line)) = frame_line {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            if let Ok(s) = serde_json::to_string(&line) {
                let _ = writeln!(f, "{s}");
            }
        }
    }
}

#[inline(always)]
fn thread_id_hint() -> u32 {
    // Cheap stable-ish id; not pthread_self exact but fine for correlation
    let id = thread::current().id();
    // Hash Debug format is heavy; use pointer of parked thread as hint via as_u64 if available
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish() as u32
}

#[derive(Serialize)]
struct JsonSpan {
    #[serde(rename = "type")]
    type_: &'static str,
    stage: &'static str,
    stage_id: u8,
    frame_id: u64,
    start_ns: u64,
    end_ns: u64,
    dur_us: u64,
    extra: u64,
    tid: u32,
}

#[derive(Serialize)]
struct JsonSummaryStage {
    stage: &'static str,
    n: usize,
    mean_us: u32,
    p50_us: u32,
    p95_us: u32,
    p99_us: u32,
}

/// RAII span. Cheap no-op when profiling is off.
pub struct Span {
    stage: Stage,
    frame_id: u64,
    start_ns: u64,
    extra: u64,
    active: bool,
    #[cfg(feature = "tracy")]
    _tracy: Option<tracy_client::Span>,
}

#[cfg(feature = "tracy")]
fn tracy_span_for(stage: Stage) -> Option<tracy_client::Span> {
    tracy_client::Client::running().map(|c| {
        // Dynamic name — allocated once per enter; OK for lab builds only.
        c.span_alloc(Some(stage.name()), "alvr", file!(), line!(), 0)
    })
}

impl Span {
    #[inline(always)]
    pub fn new(stage: Stage, frame_id: u64) -> Self {
        Self::with_extra(stage, frame_id, 0)
    }

    #[inline(always)]
    pub fn with_extra(stage: Stage, frame_id: u64, extra: u64) -> Self {
        if !enabled() {
            return Self {
                stage,
                frame_id,
                start_ns: 0,
                extra,
                active: false,
                #[cfg(feature = "tracy")]
                _tracy: None,
            };
        }
        Self {
            stage,
            frame_id,
            start_ns: now_ns(),
            extra,
            active: true,
            #[cfg(feature = "tracy")]
            _tracy: tracy_span_for(stage),
        }
    }

    #[inline(always)]
    pub fn set_extra(&mut self, extra: u64) {
        self.extra = extra;
    }

    #[inline(always)]
    pub fn end(self) {
        drop(self);
    }
}

impl Drop for Span {
    #[inline(always)]
    fn drop(&mut self) {
        if self.active {
            let end = now_ns();
            record(self.stage, self.frame_id, self.start_ns, end, self.extra);
        }
    }
}

/// Instantaneous mark (zero duration) — useful for ordering events.
#[inline(always)]
pub fn mark(stage: Stage, frame_id: u64, extra: u64) {
    if !enabled() {
        return;
    }
    let t = now_ns();
    record(stage, frame_id, t, t, extra);
}

// ---------------------------------------------------------------------------
// Flush / summary
// ---------------------------------------------------------------------------

fn flush_loop() {
    loop {
        thread::sleep(Duration::from_millis(200));
        maybe_emit_summary();
    }
}

fn maybe_emit_summary() {
    if !enabled() {
        return;
    }
    let mut guard = STATE.lock();
    let Some(st) = guard.as_mut() else {
        return;
    };
    if st.last_summary.elapsed() < st.log_interval {
        return;
    }
    st.last_summary = Instant::now();

    let mut stages = Vec::new();
    for i in 0..STAGE_COUNT {
        let (mean, p50, p95, p99) = st.aggs[i].percentiles_us();
        let n = st.aggs[i].count;
        if n == 0 {
            continue;
        }
        stages.push(JsonSummaryStage {
            stage: Stage::from_u8(i as u8).name(),
            n,
            mean_us: mean,
            p50_us: p50,
            p95_us: p95,
            p99_us: p99,
        });
    }

    if stages.is_empty() {
        return;
    }

    // Human-readable log line for ALVR session logs
    let mut parts: Vec<String> = Vec::with_capacity(stages.len());
    for s in &stages {
        parts.push(format!(
            "{}: p50={} p95={} p99={}us n={}",
            s.stage, s.p50_us, s.p95_us, s.p99_us, s.n
        ));
    }
    log::warn!(
        "PROFILE summary frames~{} dropped_ring={} | {}",
        st.frames_seen,
        st.dropped.load(Ordering::Relaxed),
        parts.join(" | ")
    );

    let summary = serde_json::json!({
        "type": "summary",
        "ts_ns": now_ns(),
        "frames_seen": st.frames_seen,
        "dropped": st.dropped.load(Ordering::Relaxed),
        "stages": stages,
    });
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&st.path)
    {
        let _ = writeln!(f, "{summary}");
    }
}

/// Force a summary flush (e.g. on stream stop).
pub fn flush_summary_now() {
    maybe_emit_summary();
}

// ---------------------------------------------------------------------------
// C ABI (for CEncoder / server_openvr FFI)
// ---------------------------------------------------------------------------

/// Stage IDs for C must match [`Stage`].
#[no_mangle]
pub extern "C" fn alvr_profile_init() {
    init_from_env();
}

#[no_mangle]
pub extern "C" fn alvr_profile_enabled() -> u8 {
    enabled() as u8
}

#[no_mangle]
pub extern "C" fn alvr_profile_now_ns() -> u64 {
    now_ns()
}

#[no_mangle]
pub extern "C" fn alvr_profile_record(
    stage: u8,
    frame_id: u64,
    start_ns: u64,
    end_ns: u64,
    extra: u64,
) {
    if !enabled() {
        return;
    }
    record_inner(stage, frame_id, start_ns, end_ns, extra);
}

#[no_mangle]
pub extern "C" fn alvr_profile_mark(stage: u8, frame_id: u64, extra: u64) {
    if !enabled() {
        return;
    }
    let t = now_ns();
    record_inner(stage, frame_id, t, t, extra);
}

// Thread-local open spans for C begin/end pairing (single nesting per stage per thread)
thread_local! {
    static C_SPANS: std::cell::RefCell<[Option<(u64, u64, u64)>; 32]> =
        std::cell::RefCell::new([None; 32]);
}

#[no_mangle]
pub extern "C" fn alvr_profile_span_begin(stage: u8, frame_id: u64) {
    if !enabled() {
        return;
    }
    let start = now_ns();
    let idx = (stage as usize) % 32;
    C_SPANS.with(|s| {
        s.borrow_mut()[idx] = Some((frame_id, start, 0));
    });
}

#[no_mangle]
pub extern "C" fn alvr_profile_span_begin_extra(stage: u8, frame_id: u64, extra: u64) {
    if !enabled() {
        return;
    }
    let start = now_ns();
    let idx = (stage as usize) % 32;
    C_SPANS.with(|s| {
        s.borrow_mut()[idx] = Some((frame_id, start, extra));
    });
}

#[no_mangle]
pub extern "C" fn alvr_profile_span_end(stage: u8) {
    if !enabled() {
        return;
    }
    let idx = (stage as usize) % 32;
    let end = now_ns();
    let data = C_SPANS.with(|s| s.borrow_mut()[idx].take());
    if let Some((frame_id, start, extra)) = data {
        record_inner(stage, frame_id, start, end, extra);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn records_spans_to_jsonl() {
        let path = std::env::temp_dir().join(format!(
            "alvr-profile-test-{}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        // Force re-init path: INIT_DONE may already be set in process — test only
        // when profiling was off at first init. Set env before first init.
        std::env::set_var("ALVR_PROFILE", "frame");
        std::env::set_var("ALVR_PROFILE_PATH", &path);
        std::env::set_var("ALVR_PROFILE_LOG_MS", "50");
        // Reset init gate for test process
        INIT_DONE.store(0, Ordering::SeqCst);
        LEVEL.store(0, Ordering::SeqCst);
        *STATE.lock() = None;
        init_from_env();
        assert!(enabled());

        {
            let s = Span::new(Stage::EncodePush, 42);
            std::thread::sleep(Duration::from_micros(200));
            drop(s);
        }
        flush_summary_now();
        std::thread::sleep(Duration::from_millis(30));

        let data = std::fs::read_to_string(&path).expect("jsonl written");
        assert!(data.contains("header"), "missing header: {data}");
        assert!(data.contains("encode_push"), "missing span: {data}");
        assert!(data.contains("summary") || data.contains("span"), "content: {data}");
        let _ = std::fs::remove_file(PathBuf::from(&path));
    }
}
