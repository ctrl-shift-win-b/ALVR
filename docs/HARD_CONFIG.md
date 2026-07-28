# Hard Config (ALVR 20.14.1 + App Store Vision Pro)

This branch targets **streamer 20.14.1** to match the **App Store ALVR client 20.14.1**.

## Why this branch

Protocol IDs are major-version based. Client **20.14.1** does not connect to streamer **21.x**.

## Usage

1. Build streamer on this branch (`hard-config/v20.14.1`).
2. Open dashboard → **Hard Config**.
3. **Apply AVP defaults** (TCP, Valve Index controllers, hands off).
4. Set absolute per-eye resolution + FPS.
5. **Solidify & Launch SteamVR**.
6. Open App Store ALVR on Vision Pro → trust if needed.

## Notes

- **No PSVR2 Sense OpenVR profile** in 20.14.1 streamer. Use Valve Index (or another listed profile). PSVR2 still pairs on AVP; buttons are remapped to the chosen SteamVR profile.
- Connect path **never** auto-restarts SteamVR when solidified.
- SteamVR launch options (Linux, hybrid NVIDIA) must still include `vrmonitor.sh` + NVIDIA env (see Linux troubleshooting).
