use alvr_common::{
    anyhow::{bail, Context, Result},
    debug, error, info, warn,
    parking_lot::Mutex,
    ConnectionError,
};
use alvr_session::AudioBufferingConfig;
use alvr_sockets::{StreamReceiver, StreamSender};
use pipewire::{
    self as pw,
    spa::{
        self,
        param::audio::{AudioFormat, AudioInfoRaw},
        pod::{self, serialize::PodSerializer, Pod},
    },
    stream::{StreamFlags, StreamListener, StreamState},
};
use std::{
    cmp,
    collections::VecDeque,
    fs,
    io::Write,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
struct Terminate;

pub fn play_microphone_loop_pipewire(
    running: impl Fn() -> bool,
    channels_count: u16,
    sample_rate: u32,
    config: AudioBufferingConfig,
    receiver: &mut StreamReceiver<()>,
) -> Result<()> {
    let batch_frames_count = sample_rate as usize * config.batch_ms as usize / 1000;
    let average_buffer_frames_count =
        sample_rate as usize * config.average_buffering_ms as usize / 1000;

    let sample_buffer: Arc<
        alvr_common::parking_lot::lock_api::Mutex<
            alvr_common::parking_lot::RawMutex,
            VecDeque<f32>,
        >,
    > = Arc::new(Mutex::new(VecDeque::new()));

    let (pw_sender, pw_receiver) = pw::channel::channel();
    let pw_loop_buffer_arc = Arc::clone(&sample_buffer);
    let receive_samples_buffer_arc = Arc::clone(&sample_buffer);
    let pw_stream_state = Arc::new(Mutex::new(StreamState::Unconnected));
    let pw_stream_state_arc = Arc::clone(&pw_stream_state);
    let thread = thread::spawn(move || {
        match pw_microphone_loop(
            pw_stream_state,
            sample_rate,
            channels_count,
            pw_receiver,
            pw_loop_buffer_arc,
        ) {
            Ok(_) => {
                debug!("Pipewire loop exiting");
            }
            Err(e) => error!("Pipewire error: {}", e.to_string()),
        }
    });

    while running() {
        let stream_audio = {
            || {
                if let Some(stream_state) = pw_stream_state_arc.try_lock() {
                    *stream_state == StreamState::Streaming && running()
                } else {
                    false
                }
            }
        };
        let receive_samples_buffer_arc = Arc::clone(&receive_samples_buffer_arc);
        crate::receive_samples_loop(
            stream_audio,
            receiver,
            receive_samples_buffer_arc,
            channels_count as _,
            batch_frames_count,
            average_buffer_frames_count,
        )
        .ok();

        // if we end up here then no consumer is currently connected to the output,
        // so discard audio packets to not cause a buildup
        if matches!(
            receiver.recv(Duration::from_millis(500)),
            Err(ConnectionError::Other(_))
        ) {
            break;
        };
    }

    if pw_sender.send(Terminate).is_err() {
        error!(
            "Couldn't send pipewire termination signal, deinitializing forcefully.
            Restart VR app to reinitialize pipewire."
        );
        unsafe { pw::deinit() };
    }

    match thread.join() {
        Ok(_) => debug!("Pipewire thread joined"),
        Err(_) => {
            error!("Couldn't wait for pipewire thread to finish");
        }
    }
    Ok(())
}

fn pw_microphone_loop(
    pw_stream_state: Arc<Mutex<StreamState>>,
    sample_rate: u32,
    channels_count: u16,
    pw_receiver: pw::channel::Receiver<Terminate>,
    sample_buffer: Arc<Mutex<VecDeque<f32>>>,
) -> Result<(), pw::Error> {
    debug!("Starting microphone pw-thread");
    let mainloop = pw::main_loop::MainLoop::new(None)?;

    let _receiver = pw_receiver.attach(mainloop.as_ref(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });

    let context = pw::context::Context::new(&mainloop)?;
    let core = context.connect(None)?;

    let stream = pw::stream::Stream::new(
        &core,
        "alvr-mic",
        pw::properties::properties! {
            *pw::keys::NODE_NAME => "ALVR Microphone",
            *pw::keys::MEDIA_NAME => "alvr-mic",
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_CLASS => "Audio/Source",
            *pw::keys::MEDIA_ROLE => "Communication",
        },
    )?;

    let chan_size = std::mem::size_of::<f32>();
    let default_channels_count: usize = channels_count.into();
    // Amount of bytes one full processing will take
    let stride = chan_size * default_channels_count;
    let _listener: StreamListener<f32> = stream
        .add_local_listener()
        .state_changed(move |_, _, _, new_state| {
            *pw_stream_state.lock() = new_state;
        })
        .process(move |stream, _| match stream.dequeue_buffer() {
            None => {
                // Nothing is connected to stream, continue
            }
            Some(mut pw_buffer) => {
                let requested_buffer_size = pw_buffer.requested();

                let datas = pw_buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }

                let mut total_size = 0;
                let pw_data = &mut datas[0];
                if let Some(slice) = pw_data.data() {
                    // How much of slices of out data we will process
                    // Get minimum number from what pipewire suggests and maximum possible value by one stride
                    let n_frames = cmp::min(requested_buffer_size as usize, slice.len() / stride);
                    total_size = n_frames;

                    for i in 0..n_frames {
                        let start = i * stride;
                        let end = start + chan_size;
                        let channel = &mut slice[start..end];
                        match sample_buffer.try_lock() {
                            Some(mut buff) => match buff.pop_front() {
                                Some(back_buff) => {
                                    let bytes = f32::to_le_bytes(back_buff);
                                    channel.copy_from_slice(&bytes);
                                }
                                None => channel.copy_from_slice(&f32::to_le_bytes(0.0)),
                            },
                            None => channel.copy_from_slice(&f32::to_le_bytes(0.0)),
                        }
                    }
                }

                let size = (stride * total_size) as u32;

                let chunk = pw_data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = stride as i32;
                *chunk.size_mut() = size;
            }
        })
        .register()?;

    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::F32LE);
    audio_info.set_rate(sample_rate);
    audio_info.set_channels(channels_count.into());

    let values: Vec<u8> = PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pod::Value::Object(pod::Object {
            type_: libspa_sys::SPA_TYPE_OBJECT_Format,
            id: libspa_sys::SPA_PARAM_EnumFormat,
            properties: audio_info.into(),
        }),
    )
    .unwrap()
    .0
    .into_inner();

    let mut params = [Pod::from_bytes(&values).unwrap()];

    stream.connect(
        spa::utils::Direction::Output,
        None,
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
        &mut params,
    )?;
    debug!("Prepared microphone pw-thread");

    mainloop.run();
    Ok(())
}

pub fn record_audio_blocking_pipewire(
    is_running: Arc<dyn Fn() -> bool + Send + Sync>,
    sender: StreamSender<()>,
    channels_count: u16,
    sample_rate: u32,
) -> Result<()> {
    let (pw_sender, pw_receiver) = pw::channel::channel();
    let is_running_clone_for_pw_terminate: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::clone(&is_running);
    thread::spawn(move || {
        while is_running_clone_for_pw_terminate() {
            thread::sleep(Duration::from_millis(500));
        }
        if pw_sender.send(Terminate).is_err() {
            error!(
                "Couldn't send pipewire termination signal, deinitializing forcefully.
                Restart VR app to reinitialize pipewire."
            );
            unsafe { pw::deinit() };
        }
    });
    let is_running_clone_for_pw = Arc::clone(&is_running);
    match pw_audio_loop(
        sample_rate,
        channels_count,
        pw_receiver,
        sender,
        is_running_clone_for_pw,
    ) {
        Ok(_) => {
            debug!("Pipewire loop exiting");
        }
        Err(e) => error!("Pipewire error: {}", e.to_string()),
    }
    Ok(())
}

fn pw_audio_loop(
    sample_rate: u32,
    channels_count: u16,
    pw_receiver: pw::channel::Receiver<Terminate>,
    mut sender: StreamSender<()>,
    is_running: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<(), pw::Error> {
    debug!("Starting audio pw-thread");

    let mainloop = pw::main_loop::MainLoop::new(None)?;

    let _receiver = pw_receiver.attach(mainloop.as_ref(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });

    let context = pw::context::Context::new(&mainloop)?;
    let core = context.connect(None)?;

    let stream = pw::stream::Stream::new(
        &core,
        "alvr-audio",
        pw::properties::properties! {
            *pw::keys::NODE_NAME => "ALVR Audio",
            *pw::keys::MEDIA_NAME => "alvr-audio",
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_CLASS => "Audio/Sink",
            *pw::keys::MEDIA_ROLE => "Game",
        },
    )?;

    let chan_size = std::mem::size_of::<i16>();

    let _listener: StreamListener<i16> = stream
        .add_local_listener()
        .process(move |stream, _| match stream.dequeue_buffer() {
            None => {
                // Nothing is connected to stream, continue
            }
            Some(mut pw_buffer) => {
                let datas = pw_buffer.datas_mut();
                if datas.is_empty() {
                    return;
                }

                let pw_data = &mut datas[0];
                let stride = chan_size * channels_count as usize;
                let n_frames = (pw_data.chunk().size() / stride as u32) as usize;
                let mut final_buffer: Vec<u8> = Vec::with_capacity(n_frames);
                if let Some(slice) = pw_data.data() {
                    for n_frame in 0..n_frames {
                        for n_channel in 0..channels_count {
                            let start = n_frame * stride + (n_channel as usize * chan_size);
                            let end = start + chan_size;
                            let channel = &mut slice[start..end];
                            let slice =
                                i16::from_ne_bytes(channel.try_into().unwrap()).to_ne_bytes();
                            final_buffer.extend(slice.iter());
                        }
                    }
                }
                if !final_buffer.is_empty() && is_running() {
                    let mut buffer = sender.get_buffer(&()).unwrap();
                    buffer
                        .get_range_mut(0, final_buffer.len())
                        .copy_from_slice(&final_buffer);
                    sender.send(buffer).ok();
                }
            }
        })
        .register()?;

    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::S16LE);
    audio_info.set_rate(sample_rate);
    audio_info.set_channels(channels_count.into());

    let values: Vec<u8> = PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pod::Value::Object(pod::Object {
            type_: libspa_sys::SPA_TYPE_OBJECT_Format,
            id: libspa_sys::SPA_PARAM_EnumFormat,
            properties: audio_info.into(),
        }),
    )
    .unwrap()
    .0
    .into_inner();

    let mut params = [Pod::from_bytes(&values).unwrap()];

    stream.connect(
        spa::utils::Direction::Input,
        None,
        StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
        &mut params,
    )?;
    debug!("Prepared audio pw-thread");

    mainloop.run();
    Ok(())
}

// ---------------------------------------------------------------------------
// Default sink/source auto-switch for Linux (PipeWire / Pulse compatibility)
// ---------------------------------------------------------------------------

/// PipeWire node name for the game-audio capture sink (PC → HMD).
pub const ALVR_AUDIO_SINK_NAME: &str = "ALVR Audio";
/// PipeWire node name for the HMD microphone virtual source (HMD → PC).
pub const ALVR_MICROPHONE_SOURCE_NAME: &str = "ALVR Microphone";

const RESTORE_FILE_NAME: &str = "alvr-audio-defaults.restore";
/// Initial blocking wait for PipeWire nodes after Streaming starts.
const WAIT_ATTEMPTS: u32 = 60;
const WAIT_INTERVAL: Duration = Duration::from_millis(100);
/// Keep re-applying defaults while the stream is alive (reconnect / late nodes /
/// another app stealing the default).
const REASSERT_INTERVAL: Duration = Duration::from_millis(500);

/// Ensure the process can talk to the user PipeWire/Pulse session.
///
/// SteamVR (and Steam Runtime) often launch `vrserver` **without**
/// `XDG_RUNTIME_DIR`. PipeWire node creation and `pactl` then miss the host
/// session, so ALVR sinks never show up and default switching is a no-op.
/// Call this **before** spawning PipeWire audio threads.
pub fn ensure_pipewire_env() {
    let runtime = resolve_xdg_runtime_dir();
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        if let Some(ref dir) = runtime {
            warn!(
                "Audio route: XDG_RUNTIME_DIR was unset (common under SteamVR); \
                 setting to {}",
                dir.display()
            );
            // SAFETY: single-threaded at server init / connection setup; only
            // sets env when previously missing so children inherit a valid path.
            std::env::set_var("XDG_RUNTIME_DIR", dir);
        } else {
            warn!(
                "Audio route: XDG_RUNTIME_DIR unset and /run/user/<uid> missing; \
                 PipeWire/pactl will likely fail"
            );
        }
    }

    // Help Pulse clients (pactl) find the session socket when Steam strips env.
    if std::env::var_os("PULSE_SERVER").is_none() {
        if let Some(ref dir) = runtime {
            let pulse = dir.join("pulse/native");
            if pulse.exists() {
                std::env::set_var(
                    "PULSE_SERVER",
                    format!("unix:{}", pulse.display()),
                );
            }
        }
    }
}

fn resolve_xdg_runtime_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(p);
        if path.is_dir() {
            return Some(path);
        }
    }
    let uid = current_uid()?;
    let path = PathBuf::from(format!("/run/user/{uid}"));
    if path.is_dir() {
        Some(path)
    } else {
        None
    }
}

fn current_uid() -> Option<u32> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

/// Saves the current system default sink/source, points them at ALVR's PipeWire
/// nodes for the stream duration, and restores the originals on drop.
///
/// Also writes a small restore file under `$XDG_RUNTIME_DIR` so a subsequent
/// ALVR start can heal defaults after a hard kill.
///
/// A background re-assert thread keeps defaults on the ALVR nodes for the whole
/// stream: late PipeWire registration after reconnect, or another process
/// stealing the default, will be corrected without needing a full rehandshake.
pub struct AudioRouteGuard {
    previous_sink: Option<String>,
    previous_source: Option<String>,
    restored: bool,
    stop_reassert: Arc<AtomicBool>,
    reassert_thread: Option<JoinHandle<()>>,
}

impl AudioRouteGuard {
    /// Switch system defaults to ALVR nodes for the sides that are enabled.
    ///
    /// Returns `None` when neither side should be switched. Always best-effort:
    /// missing `pactl` or nodes only produce warnings, never hard errors.
    pub fn activate(switch_sink: bool, switch_source: bool) -> Option<Self> {
        if !switch_sink && !switch_source {
            return None;
        }

        ensure_pipewire_env();

        // Read any previous restore snapshot *before* stale restore deletes it.
        // Needed when the current default is still an ALVR node (reconnect race,
        // manual set, or unclean previous exit) so we don't save ALVR as the
        // restore target and lose the user's real 5.1 / desk devices.
        let (saved_sink, saved_source) = read_restore_file();

        // Heal any leftover defaults from a previous unclean exit first.
        restore_stale_defaults();

        if !pactl_available() {
            warn!(
                "pactl not found or cannot reach the Pulse/PipeWire session; \
                 cannot auto-switch default audio devices. \
                 Route games to \"{ALVR_AUDIO_SINK_NAME}\" and apps to \
                 \"{ALVR_MICROPHONE_SOURCE_NAME}\" manually."
            );
            return None;
        }

        let previous_sink = if switch_sink {
            capture_previous_device(
                "sink",
                get_default_sink().ok(),
                saved_sink,
                ALVR_AUDIO_SINK_NAME,
            )
        } else {
            None
        };

        let previous_source = if switch_source {
            capture_previous_device(
                "source",
                get_default_source().ok(),
                saved_source,
                ALVR_MICROPHONE_SOURCE_NAME,
            )
        } else {
            None
        };

        write_restore_file(previous_sink.as_deref(), previous_source.as_deref());

        // Short initial wait so the common path switches defaults before the
        // connection pipeline blocks on disconnect_notif. Re-assert covers late
        // nodes after AVP Home → reconnect.
        if switch_sink {
            let _ = wait_for_device("sinks", ALVR_AUDIO_SINK_NAME);
        }
        if switch_source {
            let _ = wait_for_device("sources", ALVR_MICROPHONE_SOURCE_NAME);
        }
        apply_alvr_defaults(switch_sink, switch_source, /*log_missing*/ true);

        // Keep trying for the whole stream so reconnect / late nodes / stolen
        // defaults are corrected without a full server restart.
        let stop_reassert = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop_reassert);
        let reassert_thread = Some(thread::spawn(move || {
            while !stop_flag.load(Ordering::Relaxed) {
                thread::sleep(REASSERT_INTERVAL);
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                apply_alvr_defaults(switch_sink, switch_source, /*log_missing*/ false);
            }
        }));

        Some(Self {
            previous_sink,
            previous_source,
            restored: false,
            stop_reassert,
            reassert_thread,
        })
    }

    /// Explicit restore (also called from Drop).
    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;

        self.stop_reassert.store(true, Ordering::Relaxed);
        if let Some(handle) = self.reassert_thread.take() {
            // Don't block forever on a stuck pactl; reassert sleeps ≤ REASSERT_INTERVAL.
            let _ = handle.join();
        }

        ensure_pipewire_env();

        if let Some(ref sink) = self.previous_sink {
            // Avoid re-pointing at a dead ALVR node if something else already changed defaults.
            if !names_match(sink, ALVR_AUDIO_SINK_NAME) {
                match set_default_sink(sink) {
                    Ok(()) => info!("Audio route: restored default sink → \"{sink}\""),
                    Err(e) => warn!("Audio route: restore default sink \"{sink}\" failed: {e:#}"),
                }
            }
        }

        if let Some(ref source) = self.previous_source {
            if !names_match(source, ALVR_MICROPHONE_SOURCE_NAME) {
                match set_default_source(source) {
                    Ok(()) => info!("Audio route: restored default source → \"{source}\""),
                    Err(e) => {
                        warn!("Audio route: restore default source \"{source}\" failed: {e:#}")
                    }
                }
            }
        }

        clear_restore_file();
    }
}

impl Drop for AudioRouteGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Choose a non-ALVR device name to restore later.
///
/// Prefer the live system default when it is a real desk device. If the live
/// default is already an ALVR node (reconnect / manual set), fall back to a
/// previously saved restore-file value.
fn capture_previous_device(
    kind: &str,
    current: Option<String>,
    saved: Option<String>,
    alvr_name: &str,
) -> Option<String> {
    match current {
        Some(name) if !names_match(&name, alvr_name) => {
            info!("Audio route: saving default {kind} \"{name}\"");
            Some(name)
        }
        Some(name) => {
            if let Some(saved) = saved.filter(|s| !names_match(s, alvr_name)) {
                info!(
                    "Audio route: current default {kind} is ALVR (\"{name}\"); \
                     keeping previously saved \"{saved}\" for restore"
                );
                Some(saved)
            } else {
                warn!(
                    "Audio route: current default {kind} is ALVR (\"{name}\") and no \
                     non-ALVR previous is known; will not restore this side on disconnect"
                );
                None
            }
        }
        None => {
            if let Some(saved) = saved.filter(|s| !names_match(s, alvr_name)) {
                info!(
                    "Audio route: could not read default {kind}; \
                     using previously saved \"{saved}\" for restore"
                );
                Some(saved)
            } else {
                warn!("Audio route: failed to read default {kind}");
                None
            }
        }
    }
}

/// Point system defaults at ALVR nodes when present. Idempotent.
///
/// When `log_missing` is true, warn once if nodes are not found yet (initial
/// activate). The re-assert loop passes false to avoid log spam.
fn apply_alvr_defaults(switch_sink: bool, switch_source: bool, log_missing: bool) {
    if switch_sink {
        match find_named_device("sinks", ALVR_AUDIO_SINK_NAME) {
            Some(resolved) => {
                let already = get_default_sink()
                    .ok()
                    .is_some_and(|c| names_match(&c, &resolved) || names_match(&c, ALVR_AUDIO_SINK_NAME));
                if !already {
                    match set_default_sink(&resolved) {
                        Ok(()) => {
                            info!("Audio route: default sink → \"{resolved}\"");
                            move_all_sink_inputs_to(&resolved);
                        }
                        Err(e) => warn!(
                            "Audio route: set default sink to \"{resolved}\" failed: {e:#}"
                        ),
                    }
                }
            }
            None if log_missing => {
                let known = list_short_names("sinks").join(", ");
                warn!(
                    "Audio route: sink matching \"{ALVR_AUDIO_SINK_NAME}\" not found yet \
                     (known sinks: [{known}]); will keep retrying while stream is active"
                );
            }
            None => {}
        }
    }

    if switch_source {
        match find_named_device("sources", ALVR_MICROPHONE_SOURCE_NAME) {
            Some(resolved) => {
                let already = get_default_source().ok().is_some_and(|c| {
                    names_match(&c, &resolved) || names_match(&c, ALVR_MICROPHONE_SOURCE_NAME)
                });
                if !already {
                    match set_default_source(&resolved) {
                        Ok(()) => info!("Audio route: default source → \"{resolved}\""),
                        Err(e) => warn!(
                            "Audio route: set default source to \"{resolved}\" failed: {e:#}"
                        ),
                    }
                }
            }
            None if log_missing => {
                let known = list_short_names("sources").join(", ");
                warn!(
                    "Audio route: source matching \"{ALVR_MICROPHONE_SOURCE_NAME}\" not found yet \
                     (known sources: [{known}]); will keep retrying while stream is active"
                );
            }
            None => {}
        }
    }
}

/// If a previous ALVR process died without restoring defaults, apply the
/// restore file (if present) and clear it. Safe to call at server start.
pub fn restore_stale_defaults() {
    ensure_pipewire_env();

    let (previous_sink, previous_source) = read_restore_file();
    let Some(path) = restore_file_path() else {
        return;
    };

    if previous_sink.is_none() && previous_source.is_none() {
        let _ = fs::remove_file(&path);
        return;
    }

    if !pactl_available() {
        warn!("Audio route: stale restore file present but pactl missing; leaving as-is");
        return;
    }

    info!("Audio route: restoring defaults from previous unclean session");

    if let Some(sink) = previous_sink {
        // Only restore if the current default is still ALVR (or missing) —
        // if the user already fixed routing, leave it alone.
        let current = get_default_sink().ok();
        let should_restore = match current.as_deref() {
            None => true,
            Some(c) if names_match(c, ALVR_AUDIO_SINK_NAME) || !device_exists("sinks", c) => true,
            Some(_) => false,
        };
        if should_restore && !names_match(&sink, ALVR_AUDIO_SINK_NAME) {
            match set_default_sink(&sink) {
                Ok(()) => info!("Audio route: stale restore sink → \"{sink}\""),
                Err(e) => warn!("Audio route: stale restore sink failed: {e:#}"),
            }
        }
    }

    if let Some(source) = previous_source {
        let current = get_default_source().ok();
        let should_restore = match current.as_deref() {
            None => true,
            Some(c)
                if names_match(c, ALVR_MICROPHONE_SOURCE_NAME) || !device_exists("sources", c) =>
            {
                true
            }
            Some(_) => false,
        };
        if should_restore && !names_match(&source, ALVR_MICROPHONE_SOURCE_NAME) {
            match set_default_source(&source) {
                Ok(()) => info!("Audio route: stale restore source → \"{source}\""),
                Err(e) => warn!("Audio route: stale restore source failed: {e:#}"),
            }
        }
    }

    let _ = fs::remove_file(&path);
}

fn names_match(a: &str, b: &str) -> bool {
    a == b || a.eq_ignore_ascii_case(b)
}

/// Match exact name, or a Pulse/PipeWire variant that still contains the label.
fn find_named_device(kind: &str, want: &str) -> Option<String> {
    let names = list_short_names(kind);
    if let Some(exact) = names.iter().find(|n| names_match(n, want)) {
        return Some(exact.clone());
    }
    // e.g. "ALVR_Audio" or application-suffixed names containing "ALVR Audio"
    let want_compact: String = want.chars().filter(|c| !c.is_whitespace()).collect();
    names.into_iter().find(|n| {
        let compact: String = n.chars().filter(|c| !c.is_whitespace()).collect();
        compact.eq_ignore_ascii_case(&want_compact)
            || n.to_ascii_lowercase()
                .contains(&want.to_ascii_lowercase())
    })
}

fn device_exists(kind: &str, name: &str) -> bool {
    find_named_device(kind, name).is_some()
}

fn pactl_available() -> bool {
    // Require a real session round-trip, not just that the binary exists
    // (`pactl --version` succeeds even with a broken XDG_RUNTIME_DIR).
    match pactl_output(&["info"]) {
        Ok(info) => {
            let ok = info.contains("Server Name:") || info.contains("Default Sink:");
            if !ok {
                warn!("Audio route: pactl info returned unexpected output");
            }
            ok
        }
        Err(e) => {
            warn!("Audio route: cannot talk to Pulse/PipeWire via pactl: {e:#}");
            false
        }
    }
}

fn pactl_command() -> Command {
    let mut cmd = Command::new("pactl");
    if let Some(dir) = resolve_xdg_runtime_dir() {
        cmd.env("XDG_RUNTIME_DIR", &dir);
        let pulse = dir.join("pulse/native");
        if pulse.exists() {
            cmd.env("PULSE_SERVER", format!("unix:{}", pulse.display()));
        }
    }
    cmd
}

fn pactl_output(args: &[&str]) -> Result<String> {
    let output = pactl_command()
        .args(args)
        .output()
        .with_context(|| format!("failed to run pactl {}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "pactl {} failed ({}): {}",
            args.join(" "),
            output.status,
            stderr.trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn get_default_sink() -> Result<String> {
    let name = pactl_output(&["get-default-sink"])?;
    if name.is_empty() {
        bail!("empty default sink");
    }
    Ok(name)
}

fn get_default_source() -> Result<String> {
    let name = pactl_output(&["get-default-source"])?;
    if name.is_empty() {
        bail!("empty default source");
    }
    Ok(name)
}

fn set_default_sink(name: &str) -> Result<()> {
    pactl_output(&["set-default-sink", name]).map(|_| ())
}

fn set_default_source(name: &str) -> Result<()> {
    pactl_output(&["set-default-source", name]).map(|_| ())
}

fn list_short_names(kind: &str) -> Vec<String> {
    // `pactl list short sinks|sources` is TAB-separated:
    //   index \t name \t driver \t sample-spec \t state
    // Name may contain spaces ("ALVR Audio"). Splitting on all whitespace
    // wrongly yields "ALVR" and breaks default switching / reconnect.
    match pactl_output(&["list", "short", kind]) {
        Ok(out) => out
            .lines()
            .filter_map(|line| {
                let mut cols = line.split('\t');
                let _index = cols.next()?;
                let name = cols.next()?.trim();
                if name.is_empty() {
                    None
                } else {
                    Some(name.to_owned())
                }
            })
            .collect(),
        Err(e) => {
            warn!("Audio route: list short {kind} failed: {e:#}");
            Vec::new()
        }
    }
}

fn wait_for_device(kind: &str, want: &str) -> Option<String> {
    for _ in 0..WAIT_ATTEMPTS {
        if let Some(name) = find_named_device(kind, want) {
            return Some(name);
        }
        thread::sleep(WAIT_INTERVAL);
    }
    None
}

fn move_all_sink_inputs_to(sink_name: &str) {
    let Ok(out) = pactl_output(&["list", "short", "sink-inputs"]) else {
        return;
    };
    for line in out.lines() {
        // Short sink-input lines are also tab-separated; index is column 0.
        let id = line
            .split('\t')
            .next()
            .unwrap_or_else(|| line.split_whitespace().next().unwrap_or(""))
            .trim();
        if id.is_empty() {
            continue;
        }
        match pactl_output(&["move-sink-input", id, sink_name]) {
            Ok(_) => debug!("Audio route: moved sink-input {id} → \"{sink_name}\""),
            Err(e) => debug!("Audio route: move-sink-input {id} failed: {e:#}"),
        }
    }
}

fn restore_file_path() -> Option<PathBuf> {
    resolve_xdg_runtime_dir().map(|d| d.join(RESTORE_FILE_NAME))
}

fn read_restore_file() -> (Option<String>, Option<String>) {
    let Some(path) = restore_file_path() else {
        return (None, None);
    };
    let Ok(contents) = fs::read_to_string(&path) else {
        return (None, None);
    };

    let mut previous_sink: Option<String> = None;
    let mut previous_source: Option<String> = None;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("sink=") {
            let s = rest.trim();
            if !s.is_empty() {
                previous_sink = Some(s.to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("source=") {
            let s = rest.trim();
            if !s.is_empty() {
                previous_source = Some(s.to_owned());
            }
        }
    }
    (previous_sink, previous_source)
}

fn write_restore_file(sink: Option<&str>, source: Option<&str>) {
    let Some(path) = restore_file_path() else {
        return;
    };
    // Never persist ALVR node names as restore targets (dead after stream ends).
    let sink = sink.filter(|s| !names_match(s, ALVR_AUDIO_SINK_NAME));
    let source = source.filter(|s| !names_match(s, ALVR_MICROPHONE_SOURCE_NAME));
    if sink.is_none() && source.is_none() {
        clear_restore_file();
        return;
    }
    let mut body = String::new();
    if let Some(s) = sink {
        body.push_str(&format!("sink={s}\n"));
    }
    if let Some(s) = source {
        body.push_str(&format!("source={s}\n"));
    }
    match fs::File::create(&path).and_then(|mut f| f.write_all(body.as_bytes())) {
        Ok(()) => info!("Audio route: wrote restore file {}", path.display()),
        Err(e) => warn!("Audio route: failed to write restore file: {e}"),
    }
}

fn clear_restore_file() {
    if let Some(path) = restore_file_path() {
        let _ = fs::remove_file(path);
    }
}
