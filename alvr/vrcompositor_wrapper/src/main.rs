#[cfg(target_os = "linux")]
fn main() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    let argv0 = std::env::args().next().unwrap();

    // File log so we can diagnose wrapper env without SteamVR swallowing stdout.
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/alvr-vrcompositor-wrapper.log")
        .ok();

    let log_line = |log: &mut Option<std::fs::File>, msg: &str| {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "[{ts}] {msg}");
            let _ = f.flush();
        }
    };

    log_line(
        &mut log,
        &format!(
            "vrcompositor-wrapper start pid={} argv0={argv0}",
            std::process::id()
        ),
    );
    log_line(
        &mut log,
        &format!(
            "DISPLAY={:?} WAYLAND_DISPLAY={:?} XDG_SESSION_TYPE={:?} XDG_RUNTIME_DIR={:?}",
            std::env::var_os("DISPLAY"),
            std::env::var_os("WAYLAND_DISPLAY"),
            std::env::var_os("XDG_SESSION_TYPE"),
            std::env::var_os("XDG_RUNTIME_DIR"),
        ),
    );

    // location of the ALVR vulkan layer manifest
    let wrapper_dir = match std::fs::read_link(&argv0) {
        Ok(path) => path.parent().unwrap().to_path_buf(),
        Err(err) => panic!("Failed to read vrcompositor symlink: {err}"),
    };
    let layer_path = wrapper_dir.join("../../share/vulkan/explicit_layer.d");
    std::env::set_var("VK_LAYER_PATH", &layer_path);
    // Vulkan < 1.3.234
    std::env::set_var("VK_INSTANCE_LAYERS", "VK_LAYER_ALVR_capture");
    std::env::set_var("DISABLE_VK_LAYER_VALVE_steam_fossilize_1", "1");
    std::env::set_var("DISABLE_MANGOHUD", "1");
    std::env::set_var("DISABLE_VKBASALT", "1");
    std::env::set_var("DISABLE_OBS_VKCAPTURE", "1");
    // Vulkan >= 1.3.234
    std::env::set_var(
        "VK_LOADER_LAYERS_ENABLE",
        "VK_LAYER_ALVR_capture,VK_LAYER_MESA_device_select",
    );
    std::env::set_var("VK_LOADER_LAYERS_DISABLE", "*");

    // Always expose session.json path (X11 and Wayland).
    let session_path = alvr_filesystem::filesystem_layout_invalid()
        .session()
        .to_string_lossy()
        .to_string();
    std::env::set_var("ALVR_SESSION_JSON", &session_path);
    if std::env::var_os("ALVR_LOG_DEBUG").is_none() {
        std::env::set_var("ALVR_LOG_DEBUG", "1");
    }

    log_line(
        &mut log,
        &format!(
            "VK_LAYER_PATH={} ALVR_SESSION_JSON={session_path} session_exists={}",
            layer_path.display(),
            Path::new(&session_path).exists()
        ),
    );

    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        let drm_lease_shim_path = wrapper_dir.join("alvr_drm_lease_shim.so");
        std::env::set_var("LD_PRELOAD", &drm_lease_shim_path);
        log_line(
            &mut log,
            &format!("Wayland: LD_PRELOAD={}", drm_lease_shim_path.display()),
        );
    } else {
        log_line(&mut log, "X11/other: no drm-lease shim preload");
    }

    // Resolve the real SteamVR compositor binary.
    // CRITICAL: Linux sets /proc/self/comm from the basename of the *executed path*
    // (TASK_COMM_LEN=16 → 15 chars). Execing "vrcompositor.real" yields
    // "vrcompositor.re", which makes SteamVR's StartVRCompositor readiness check
    // report "timeout - vrcompositor process is not running" and 303 even though
    // the process is alive. Exec a path whose basename is exactly "vrcompositor".
    let stock_real = PathBuf::from(format!("{argv0}.real"));
    let preferred = wrapper_dir.join("steamvr_vrcompositor").join("vrcompositor");

    let real_path = if preferred.is_file() {
        preferred
    } else {
        // Fallback: hardlink/copy stock .real into XDG_RUNTIME_DIR as .../vrcompositor
        let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        let dir = PathBuf::from(runtime).join("alvr-vrcompositor-bin");
        let _ = std::fs::create_dir_all(&dir);
        let target = dir.join("vrcompositor");
        let _ = std::fs::remove_file(&target);
        if !stock_real.is_file() {
            panic!(
                "Neither {} nor {} exists — re-run ALVR SteamVR launch to wrap compositor",
                preferred.display(),
                stock_real.display()
            );
        }
        match std::fs::hard_link(&stock_real, &target) {
            Ok(()) => log_line(
                &mut log,
                &format!(
                    "hardlinked {} -> {}",
                    stock_real.display(),
                    target.display()
                ),
            ),
            Err(e) => {
                log_line(
                    &mut log,
                    &format!(
                        "hardlink failed ({e}); copying {} -> {}",
                        stock_real.display(),
                        target.display()
                    ),
                );
                std::fs::copy(&stock_real, &target)
                    .unwrap_or_else(|err| panic!("copy vrcompositor failed: {err}"));
                let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
            }
        }
        target
    };

    log_line(
        &mut log,
        &format!(
            "execvp {} (basename must be 'vrcompositor' for SteamVR readiness)",
            real_path.display()
        ),
    );

    // Keep original argv (argv0 stays the SteamVR path) so SteamVR sees expected args.
    let err = exec::execvp(real_path, std::env::args());
    log_line(&mut log, &format!("Failed to run vrcompositor {err}"));
    println!("Failed to run vrcompositor {err}");
}

#[cfg(not(target_os = "linux"))]
fn main() {}
