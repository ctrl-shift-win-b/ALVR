# Hard Config (ALVR 20.14.1 + App Store Vision Pro / Quest 3)

This branch targets **streamer 20.14.1** to match the **App Store ALVR client 20.14.1**.

## Why this branch

Protocol IDs are major-version based. Client **20.14.1** does not connect to streamer **21.x**.

SteamVR snapshots HMD projection (`GetProjectionRaw`) when the driver activates — before any client connects. Hard Config therefore bakes a **headset-specific frustum** at solidify time.

## Usage

1. Build streamer on this branch.
2. Open dashboard → **Hard Config**.
3. Select **Apple Vision Pro** or **Quest 3**.
4. Optionally **Apply AVP defaults** or **Apply Quest 3 defaults** (controllers / protocol / FOV). Resolution is independent; 5000×5000 per eye is fine for both.
5. **Solidify & Launch SteamVR**.
6. Open the matching ALVR client → trust if needed.

Switching headsets: change the selector, solidify, and relaunch SteamVR. Do not connect Quest 3 to a session solidified for AVP (or vice versa) without re-solidifying.

## FOV calibration

Built-in frustums:

- **AVP** — App Store client ViewsConfig used on this host.
- **Quest 3** — Quest 3 ALVR client ViewsConfig captured on this host (matches published Quest 3 HMD geometry).

While streaming, the host stores the last valid client ViewsConfig. Use **Use last client FOV for SteamVR start**, then solidify and relaunch, to refine the baked frustum from a real session.

## Notes

- **No PSVR2 Sense OpenVR profile** in 20.14.1 streamer. Use Valve Index (or another listed profile). PSVR2 still pairs on AVP; buttons are remapped to the chosen SteamVR profile.
- Connect path **never** auto-restarts SteamVR when solidified.
- SteamVR launch options (Linux, hybrid NVIDIA) must still include `vrmonitor.sh` + NVIDIA env (see Linux troubleshooting).
