# hue-jack: design

> Design reference. The executable build plan is [`BUILD.md`](BUILD.md); hardware verification is [`HOME-RUNBOOK.md`](HOME-RUNBOOK.md).

A self-contained music→Hue light appliance on an Intel NUC6i3SYB driving powered speakers. You send music to it over Bluetooth, AirPlay or a YouTube Music cast; it plays the music **delayed by exactly the light-processing latency**, while it drives Hue lights in sync. Headless: no screen, no server, no cloud.

## 0. Known environment

| Thing | Value |
|---|---|
| Hue bridge | **Hue Bridge v2 (square), `modelid=BSB002`**, IP **10.0.0.166**, mDNS `ecb5faa77674.local`, `bridgeid=ecb5fafffea77674`, `apiversion 1.78.0`, `swversion 1978293000`. CLIP v2 is present (unauthenticated requests return 403). **Not yet paired.** |
| Dev machine | Fedora 44, 10.0.0.161 (`enp13s0`). Rust 1.96, Node 26, podman 5.8, PipeWire (desktop session in use), BlueZ 5.87, gh (account `axle-h`, git over SSH) |
| Appliance | Intel NUC6i3SYB: i3-6100U, Intel 8260 Wi-Fi/BT 4.2, gigabit Ethernet, front 3.5 mm jack → **powered speakers** |
| Repo | `github.com/axle-h/hue-jack` (public); OS image `ghcr.io/axle-h/hue-jack` |
| Fedora 44 packages | `shairport-sync 4.3.7` (check it was built with AirPlay 2 support), `mpv 0.41`, `bluez 5.87` (ships `mpris-proxy`), `pipewire-devel 1.6.9`. **`nqptp` is not packaged**, so build it from source. |

## 1. How it works

```
 any phone / Echo ─ Bluetooth A2DP ───────────▶ BlueZ ──────────────┐   any app: Amazon Music, YT Music, podcasts…
 iPhone / Mac ─ AirPlay 2 ────────────────────▶ shairport-sync ─────┤   any app, for friends with iPhones
 phone ─ YouTube Music cast (DIAL + Lounge) ──▶ ytcr sidecar ─▶ mpv ┤   phone stays free; NUC streams directly
                                                                    ▼
                                             PipeWire null sink "hue-jack-in" (default output)
                                                                    │ monitor
                                                                    ▼
                         ┌──────────────────────── hue-jack (Rust) ─────────────────────────┐
                         │  capture ─┬─▶ analysis (look-ahead) ─▶ effects ─▶ Hue streamer   │──DTLS/UDP──▶ Bridge ─▶ lights
                         │           └─▶ delay line (D ms) ─▶ playback                      │──▶ 3.5 mm ─▶ powered speakers
                         │  web UI (area, presets, palettes, delay calibration, status)     │
                         └──────────────────────────────────────────────────────────────────┘
```

- Every receiver plays into one virtual sink. hue-jack is the only thing that touches the real output, so whatever arrives (from any source, from any app) gets the same buffering, delay and light show.
- **Delay** `D = L_lights − L_audio_out`, set by calibration (§8). Look-ahead equal to `D` lets onset detection be non-causal.
- Latency before the sink (network, receiver buffering, Bluetooth) affects the lights and the sound equally, so it's irrelevant.
- One host and one clock.

## 2. Sources

All three are first-class, always on, and feed the same sink. Each one gives metadata (title, artist, play state) to hue-jack for the UI and for auto start and stop.

| | Bluetooth A2DP | AirPlay 2 | YouTube Music cast |
|---|---|---|---|
| Who | Any phone (Android or iPhone), the Echo, laptops | iPhone, iPad, Mac | Android or iPhone with the YT Music or YouTube app |
| Which apps | **Any**: the phone's whole audio output | **Any** app with AirPlay | YT Music and YouTube only |
| Phone role | Streams the audio, so it must stay in range and awake | Streams the audio | Remote control only; the NUC fetches the stream itself |
| Software | BlueZ + WirePlumber (PipeWire's native A2DP sink) | `shairport-sync` (AirPlay 2 build) + `nqptp` | `yt-cast-receiver` + `youtubei.js` + `mpv` (Node sidecar) |
| Metadata | AVRCP via BlueZ `org.bluez.MediaPlayer1` (D-Bus) | shairport-sync metadata pipe / MQTT | ytcr player state |
| Risk | Low. Range, and pairing UX on a headless box | Low | Medium: YouTube changes stream access from time to time |

### Bluetooth A2DP sink
- **The universal source**: whatever the phone plays (Amazon Music, YT Music, Spotify, podcasts, a browser tab) comes through the same pipeline.
- **A2DP only.** Disable the HFP/HSP profiles in WirePlumber, so phones never route calls or the mic to the NUC and it never shows up as a headset.
- **Codecs:** SBC always; AAC (better for iPhones) and LDAC or aptX where the Fedora PipeWire build includes them. Check what's in the image.
- **Pairing on a headless box:**
  - A **"Pair new device"** button in the web UI makes the NUC discoverable and auto-accepts "Just Works" pairing for 2 minutes.
  - Outside that window it isn't discoverable, but already-bonded devices can reconnect at any time.
  - The web UI lists bonded devices and can remove them.
  - BlueZ name: `hue-jack`. Device class: audio/speaker.
- **Volume:** AVRCP absolute volume, so the phone's volume buttons control the NUC's output level.
- Bluetooth adds about 150–250 ms before the sink. That's irrelevant to sync, but pause and skip feel slightly sluggish.

### AirPlay 2
- `shairport-sync` built with AirPlay 2 support + `nqptp` (which needs UDP 319/320). Output goes to the PipeWire sink. Advertised as `hue-jack` via avahi.
- AirPlay buffers about 2 s by design. It's before the sink, so it doesn't matter for sync.

### YouTube Music cast (TV/Lounge protocol, headless)
Two "TV-style" YouTube receivers exist, and they aren't the same thing:

| | [GarrettBlackmon/youtube-cast-receiver](https://github.com/GarrettBlackmon/youtube-cast-receiver) | [patrickkfkan/yt-cast-receiver](https://github.com/patrickkfkan/yt-cast-receiver) |
|---|---|---|
| How | An Electron window loads the real `youtube.com/tv` web app with a smart-TV user agent | A Node library that implements the receiver side itself (a DIAL server + the Lounge API); no browser |
| UI | Needs a screen; pairing by TV code | **None.** Headless (Volumio runs it on headless Pis) |
| Discovery | Manual "link with TV code" | Appears automatically in the cast menu on the same Wi-Fi (DIAL) |
| YouTube Music | Not mentioned | Supported since v1.0 |
| Playback | YouTube's own player | Ours: `youtubei.js` resolves the audio stream → `mpv` → PipeWire |

**Use patrickkfkan/yt-cast-receiver**, isolated in a sidecar so a YouTube-side breakage only takes out this source; Bluetooth still covers YT Music in that case. The README calls it work-in-progress, and YouTube's stream-access changes break `youtubei.js` until it's updated, so pin versions known to work and keep them upgradable.

#### Accounts, Premium and ads
- **The NUC has no account by default.** The signed-in phone is the remote. When it casts, the Lounge protocol passes the receiver a per-video **credential transfer token** (`video.context.ctt`) taken from the sender's session. The player sends it with its `/player` request (`credentialTransferTokens` in the Innertube context), which is how private videos and your YT Music uploads play. This is the same mechanism volumio-ytcr uses; there are no passwords or cookies on the box.
- **No ads, ever.** On a real TV, ads come from the TV app's own player. Our player fetches only the bare audio stream for each track, so nothing inserts ads, Premium or not.
- **Audio quality:** without Premium the best is about 128 kbps AAC (itag 140) or about 160 kbps Opus (itag 251). Premium adds **256 kbps AAC (itag 141)** and, on YT Music, 256 kbps Opus (itag 774). volumio-ytcr's docs say Premium subscribers get the 256 kbps streams through the receiver, which suggests the `ctt` carries the entitlement. **To verify in phase 3:** log the chosen itag per track and check that 141 or 774 appears.
- **Fallback if Premium formats don't come through:** `youtubei.js` cookie auth (`Innertube.create({ cookie })`) with your account's cookie, entered once in the web UI and stored in `/var/lib/hue-jack`. Downsides: the cookie expires and needs re-pasting, and Google can flag accounts used by unofficial clients. Only add it if the `ctt` path proves insufficient.
- **To check:** whether tracks played through the receiver count towards your YT Music history and recommendations. The phone owns the queue, but the receiver may play them anonymously. Both repos were maintained as of March 2026 (yt-cast-receiver v2.1.1, volumio-ytcr v2.2.1). volumio-ytcr is the reference for PO-token generation and for choosing the client per track (`YTMUSIC`, then a `TV` retry, then `ANDROID_VR`).

*If it ever dies for good:* run the real `youtube.com/tv` app in Chromium on a headless virtual display (`cage` headless backend or Xvfb, capped at 480p), pairing by TV code once through a screenshot or VNC peek. No monitor is needed.

### Several sources at once
PipeWire mixes everything in the sink, so nothing breaks, but two songs at once is a mess. Policy: **the most recently started source wins**. hue-jack pauses the others where it can (AVRCP pause for Bluetooth, the ytcr player, the shairport-sync remote control) and shows the active source in the UI.

### Level normalisation
Sources arrive at very different levels (Bluetooth volume, AirPlay volume, YT loudness normalisation). The analyser applies its own slow AGC to the analysis copy only, so the light intensity doesn't depend on how loud the phone is set. The audio copy is left untouched.

### Not in scope / not possible
- **Spotify Connect:** not needed (the Hue app syncs Spotify itself). Spotify still works over Bluetooth or AirPlay.
- **A software Google Cast receiver:** impossible. Senders verify a Google-signed device certificate, and open-source receivers (e.g. Shanocast) only work by replaying signatures, with Chrome as the sender. The only route to real Cast is certified hardware (e.g. a Cast-capable streamer → USB line or optical in), which isn't planned.

## 3. Hardware: NUC6i3SYB

- i3-6100U (2C/4T): all three receivers + the pipeline are a small fraction of it, including the headless-Chromium fallback if it's ever needed.
- Onboard Intel 8260 Bluetooth 4.2 for the A2DP sink. The NUC needs to sit within Bluetooth range of where people use their phones.
- **Use wired gigabit Ethernet and disable Wi-Fi.** The Intel 8260 shares 2.4 GHz between Wi-Fi and BT.
- Output: the onboard 3.5 mm jack to the powered speakers. Add a cheap USB DAC only if it hisses.

## 4. Appliance OS: Fedora bootc

- A `Containerfile` in this repo: a Fedora bootc base + PipeWire, WirePlumber, BlueZ, avahi, mpv, Node runtime, shairport-sync + nqptp, the hue-jack binary, systemd units and default config.
- Build it with podman, install once (`bootc install` or an ISO), then update with `bootc upgrade` / `bootc rollback`.
- A `huejack` user with lingering enabled runs the PipeWire/WirePlumber user services plus the `hue-jack`, `ytcr` and `shairport-sync` units. BlueZ runs as the system service; WirePlumber in the `huejack` session owns the A2DP endpoints.
- No host firewall. It's a home-LAN appliance, and AirPlay 2, DIAL/SSDP and mDNS need wide port ranges. Leave firewalld out of the image.
- `hue-jack.local` comes from avahi. Plain HTTP is fine (there's no mic, so no secure context is needed).
- State lives in `/var/lib/hue-jack`: Hue credentials, the selected area, calibration and presets.

## 5. Choosing lights

Everything you set up in the Hue app is visible to any paired client through CLIP v2: `room`, `zone`, `light`, `device` and `entertainment_configuration` (entertainment areas, including each light's 3D position).

Constraints that shape the design:
- Streaming only works through an **entertainment area**, and only one area streams at a time.
- **Up to 10 lights per entertainment area** (Hue's documented limit; the stream format allows 20 channels, and gradient lights use several channels).
- Only colour-capable lights can be in an area; white-only bulbs can't.
- So "all lights" works only if you have ≤10 colour lights.

Plan:
1. **MVP: pick an existing entertainment area** in the hue-jack UI (a dropdown fed by `GET /clip/v2/resource/entertainment_configuration`). You choose lights and drag positions in the Hue app, which already has a good 3D placement editor. Areas made for Hue Sync or the Spotify integration work too.
2. **Later (optional):** a "quick area" in the hue-jack UI. Tick lights, or pick a room or zone, and hue-jack creates or updates its own `hue-jack` entertainment configuration via `POST`/`PUT /clip/v2/resource/entertainment_configuration`, with auto-positions.
3. **Later (optional), more than 10 lights:** stream the ≤10 area lights, and drive the rest (including white bulbs) slowly via normal REST at ≤10 updates/s total with smooth transitions, for ambient colour and brightness. That's good enough for "the rest of the room breathes with the music".

## 6. Hue protocol

| What | API | Notes |
|---|---|---|
| Discovery | mDNS `_hue._tcp` (only one bridge on the LAN), with a static IP override | |
| Pairing | `POST https://<bridge>/api` `{"devicetype":"hue-jack#nuc","generateclientkey":true}` after pressing the link button | Returns `username` (the app key) and `clientkey` (the PSK). Done from the web UI on first run. |
| Areas | CLIP v2 `GET /clip/v2/resource/entertainment_configuration` (`hue-application-key` header) | Channels + positions |
| Start/stop | `PUT …/entertainment_configuration/{id}` `{"action":"start"}` / `"stop"` | Start when audio becomes active, stop after N s of silence so the lights go back to normal |
| Streaming | DTLS 1.2 → `<bridge>:2100/udp`, `TLS_PSK_WITH_AES_128_GCM_SHA256`; identity = `hue-application-id` (`GET /auth/v1`), PSK = hex-decoded `clientkey` | "HueStream" v2: ≤20 channels × 16-bit RGB or XY, sent at about 50–60 Hz; the bridge forwards about 25 Hz to Zigbee; the stream drops after about 10 s idle |

Effects use short fades rather than single-frame strobes, to suit the ~40 ms Zigbee steps.

## 7. Stack

| Component | Choice |
|---|---|
| hue-jack core | **Rust**, one binary: `pipewire` crate (capture + playback), ring-buffer delay line, `rustfft` analysis (band energies, spectral-flux onsets with look-ahead peak picking, tempo for effect pacing), `openssl` DTLS-PSK, `reqwest` CLIP v2, `axum` web UI and API |
| Effects | `Effect` trait rendering per-channel colours from features + channel positions; TOML presets |
| Source adapters | **Bluetooth:** `bluer` (BlueZ's official Rust crate) for the pairing window, pairing agent and bonded devices. **Bluetooth + AirPlay metadata and pause:** MPRIS on the `huejack` session bus (BlueZ's `mpris-proxy` exports AVRCP players as MPRIS; shairport-sync has an MPRIS interface). **YouTube:** Node/TS sidecar (`yt-cast-receiver` + `youtubei.js` + mpv over JSON IPC) with a small HTTP API on a unix socket for status and pause |
| Web UI | TypeScript + Vite, served by hue-jack: Hue pairing, area picker, Bluetooth "pair new device" + bonded list, active source and now playing, presets, palette, intensity, delay calibration |
| OS | Fedora bootc image built from this repo |

## 8. Delay calibration

1. **By eye:** a calibration mode plays a click track and flashes the lights; a UI slider adjusts `D` live.
2. **Measured:** film the speaker and a lamp at 240 fps and count frames.

## 9. How the light show is generated

No off-the-shelf library takes a waveform and produces Hue colours; it's our own small DSP pipeline. Its input is the raw **PCM waveform** (48 kHz stereo float samples) tapped from the virtual sink.

1. **Frames:** mix to mono; 2048-sample Hann window, hop 480 samples, so **100 feature frames/s**. FFT with `rustfft`.
2. **Band energies:** sub 20–60 Hz, bass 60–250 Hz, mid 250–2 kHz, high 2–8 kHz, air 8–16 kHz. Log-energy per band, then a slow **AGC** (peak follower: instant attack, ~10 s release) normalises each band to 0..1. The light intensity then doesn't depend on phone volume. The AGC applies to the analysis copy only; the audio is untouched.
3. **Onsets (beats and hits):** spectral flux (frame-to-frame increase in log magnitude, summed; computed overall and for the bass band separately), with an adaptive threshold (moving median + k·MAD). **Look-ahead peak picking:** a frame is an onset only if it's the maximum within ±40 ms. That's non-causal, which is fine because the audio is delayed anyway. Output: onset strength 0..1.
4. **Tempo (pacing only, never timing):** autocorrelation of the onset envelope over a 6 s window, 70–180 BPM. Used for palette rotation speed and chase step rate.
5. **Silence:** RMS below −55 dBFS for 1 s means silent (drives the stream start and stop).
6. **Effects** (Rust `Effect` trait) take `FeatureFrame { t, bands[5], onset, bass_onset, bpm, rms }` plus the area's channels (id + x/y/z position) and render **RGB per channel at 50 Hz**. Then smoothing (attack/decay envelopes, so nothing changes faster than about 40 ms, the Zigbee step), a global brightness cap, and gamma.
   - `pulse`: all lights breathe with bass; a bass onset flashes to the next palette colour with ~150 ms decay.
   - `spectrum`: channels sorted by x position (left to right) map to bands from low to high; each band's brightness = its energy.
   - `chase`: each onset advances a highlight to the next light in x order; the tail fades out.
7. **Palettes:** named lists of colours (e.g. `sunset`, `ocean`, `neon`, `fire`); effects pick by index, and the palette rotates once per N beats.
8. **Timing:** the lights are emitted as soon as a frame's look-ahead is complete; the audio plays `D` ms after capture. In effect, `D = lookahead + L_lights − L_audio_out`, and the calibration slider sets `D` directly (default 150 ms).

`hue-jack simulate` runs this exact pipeline on a WAV and writes an HTML preview (an animated light layout synced to the audio), so effects can be judged without bulbs.

## 10. Build order

See [`BUILD.md`](BUILD.md) (milestones M0–M8) and [`HOME-RUNBOOK.md`](HOME-RUNBOOK.md).
