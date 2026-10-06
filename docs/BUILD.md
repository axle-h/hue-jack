# hue-jack: build plan (unattended)

This is the executable plan for building hue-jack **in one unattended session, with no human checkpoints**. Read [`PLAN.md`](PLAN.md) first for the design. When finished, the repo should contain working, tested code, green CI, a published OS image and an installer ISO. Alex then follows [`HOME-RUNBOOK.md`](HOME-RUNBOOK.md) with the real bulbs and the NUC.

## Ground rules

1. **Authorisation:** Alex has explicitly authorised committing and pushing to `main` of `github.com/axle-h/hue-jack` (git over SSH) for this build, and using `gh` against this repo (Actions, runs, releases, packages). This overrides the global "git is read-only" rule **for this repo only**. Don't touch any other repo.
2. **Commit small and often**, with a clear message per logical step, and push after each milestone. Keep CI green on `main`; if a push breaks CI, fixing it is the next task.
3. **No human checkpoints.** Where something can't be verified without hardware (real bulbs, NUC, phone, Bluetooth pairing, casting, AirPlay), build it against the stated assumptions, cover it with fakes or tests, and list it in `docs/STATUS.md` under "Untested on hardware". Never block waiting for Alex.
4. **Record deviations:** if you change any design decision, add a line to `docs/DECISIONS.md` (date, decision, why).
5. **Protect the dev machine (Alex's desktop, 10.0.0.161):**
   - Never change the default PipeWire sink or source, and never touch Alex's audio devices. Local audio tests use **dedicated null sinks named `hue-jack-dev-*`**, loaded and unloaded by the test script itself.
   - Never make the dev machine's Bluetooth discoverable or pairable, and never pair or unpair anything.
   - Don't run the YouTube receiver locally for longer than a test needs. While it runs, it shows up in cast menus on the LAN.
   - Don't `sudo`. If something needs root, do it in CI or in the image.
6. **Hue bridge (10.0.0.166):** you may only make *unauthenticated* reads (`GET https://10.0.0.166/api/0/config`, mDNS). Pairing needs a physical button press, so it happens in the runbook. All Hue logic is tested against `fake-bridge`.
7. **No secrets in git.** Hue keys live in the state dir. The SSH *public* key in the installer config is fine.
8. **Licence:** MIT. If you port code from `patrickkfkan/volumio-ytcr` or `yt-cast-receiver` (both MIT), keep the attribution in a header comment.
9. **Priority if time runs short:** M0 → M1 → M2 → M3 → M4 → M5 → M7 → M6-Bluetooth → M6-YouTube → M6-AirPlay → M8. Always leave time for M8. A working image with Bluetooth + lights beats a half-working everything.

## Repo layout (target)

```
hue-jack/
├─ Cargo.toml                 # workspace
├─ crates/
│  ├─ hue-jack/               # lib + main binary (all core logic)
│  │  └─ src/{main.rs, cli.rs, config.rs, state.rs,
│  │          hue/{mod,discovery,clip,pairing,stream,huestream}.rs,
│  │          audio/{mod,pipewire,wav,delay}.rs,
│  │          analysis/{mod,bands,onset,tempo,agc}.rs,
│  │          effects/{mod,pulse,spectrum,chase,palette,smoothing}.rs,
│  │          engine.rs, simulate.rs,
│  │          sources/{mod,bluetooth,mpris,ytcr}.rs,
│  │          web/{mod,api,ws}.rs}
│  └─ fake-bridge/            # test double: CLIP v2 HTTPS + DTLS-PSK HueStream receiver
├─ web/                       # Vite + TypeScript (+ Preact) UI; build output web/dist embedded via rust-embed
├─ ytcr/                      # Node/TS sidecar: yt-cast-receiver + youtubei.js + mpv
├─ os/
│  ├─ Containerfile           # bootc image
│  ├─ rootfs/                 # files copied into the image (/usr/lib/systemd/..., /usr/share/pipewire/..., /etc/...)
│  └─ bib/config.toml         # bootc-image-builder config (kickstart + user)
├─ tools/                     # dev scripts (local PipeWire passthrough test, etc.)
├─ test-audio/                # generated test WAVs (gitignored), made by `hue-jack gen-test-audio`
├─ docs/{PLAN,BUILD,HOME-RUNBOOK,STATUS,DECISIONS}.md
├─ .github/workflows/{ci.yml,image.yml,iso.yml}
├─ CLAUDE.md
└─ README.md
```

## Contracts and assumptions

### Hue (assumed correct per the Hue Entertainment API v2; verified at home)
- **Discovery:** mDNS `_hue._tcp` (`mdns-sd` crate). Return IP + `bridgeid` from the TXT record. Static `--bridge <ip>` override.
- **HTTPS:** the bridge's certificate is self-signed with CN = bridgeid. Accept it if the CN matches the expected bridge id (pinned at pairing, TOFU). Use a custom rustls verifier, or `danger_accept_invalid_certs` plus a CN check.
- **Pairing:** `POST /api` `{"devicetype":"hue-jack#<hostname>","generateclientkey":true}`. Error type 101 ("link button not pressed") means poll every 1 s for up to 30 s. Success returns `username` and `clientkey`. Then `GET /auth/v1` with header `hue-application-key: <username>`; the response header `hue-application-id` gives the DTLS PSK identity.
- **Areas:** `GET /clip/v2/resource/entertainment_configuration` returns `data[]` with `id`, `metadata.name`, `status`, `channels[] {channel_id, position{x,y,z}, members[]}`.
- **Start/stop:** `PUT /clip/v2/resource/entertainment_configuration/{id}` with `{"action":"start"|"stop"}`.
- **DTLS:** `openssl` crate, DTLS 1.2 client over a *connected* `UdpSocket` (write a `Read + Write` adapter: one write = one datagram). Cipher list `PSK-AES128-GCM-SHA256`; PSK client callback: identity = application id, key = hex-decoded clientkey. Port 2100. Handshake within 5 s of `start`. If Fedora's crypto policy rejects PSK, set the cipher list and security level explicitly on the context and note it in `DECISIONS.md`.
- **HueStream v2 packet:** `"HueStream"` (9 bytes ASCII), `0x02 0x00` (version), seq (u8), `0x00 0x00`, colour space (`0x00` RGB), `0x00`, entertainment configuration id (36 ASCII bytes), then per channel `channel_id (u8)` + `R,G,B (u16 BE each)`. That's 52 header bytes + 7 per channel, at most 20 channels.
- **Stream loop:** a dedicated thread with a 50 Hz tick. It always sends the latest frame (and resends during quiet passages, so the bridge's ~10 s timeout never trips). On error: `stop` → backoff 1/2/5 s → `start` + re-handshake.
- **Lifecycle:** audio active (RMS > −55 dBFS for 0.5 s) → start; silent for `idle_stop_secs` (default 20) → stop. *Assumption:* the bridge restores the lights' previous state after stop (verify at home; if not, snapshot the lights' state via CLIP v2 before start and restore it after).

### Audio
- Format: f32 interleaved stereo at 48 kHz throughout.
- `AudioInput` / `AudioOutput` traits with implementations: **PipeWire** (`pipewire` crate; needs `pipewire-devel`, `clang-devel`) and **WAV/file** (`hound`), plus `Null` output for tests.
- PipeWire capture stream: `stream.capture.sink = true`, `target.object = <input sink name>` (default `hue-jack-in`), so it reads the sink's monitor.
- PipeWire playback stream: `target.object = <output>` and `node.dont-reconnect = true`. `output = "auto"` resolves to the first `Audio/Sink` whose `node.name` starts with `alsa_output.`. **It must never be the input sink: refuse to start if it is (feedback loop).**
- **Delay line:** a ring buffer of up to 1000 ms. `D` can change at runtime, ramped over 20 ms to avoid clicks.
- **Drift guard:** if the fill level drifts more than 5 ms from target, drop or duplicate one frame per 10 ms until it's back. Log a counter. Expose fill level and the counter in `/api/status`.
- Analysis gets a copy of every captured block through a lock-free SPSC ring (e.g. `rtrb`). Never allocate or lock in the PipeWire callbacks.

### Config and state
- State dir: `$HUEJACK_STATE_DIR`, else `$XDG_STATE_HOME/hue-jack`, else `~/.local/state/hue-jack`. The image sets `HUEJACK_STATE_DIR=/var/lib/hue-jack`.
- `state.json`: `{ bridge: {ip, bridge_id, app_key, client_key, app_id}, area_id, delay_ms, effect, palette, brightness_max, intensity, idle_stop_secs, output, input_sink }`. Write atomically (temp file + rename).
- Web listen address: `--listen` (default `0.0.0.0:8080`; the image uses `0.0.0.0:80` via the sysctl in M7).

### CLI (`clap`)
| Command | Purpose |
|---|---|
| `hue-jack serve [--listen] [--virtual]` | Main daemon. `--virtual` runs without a bridge; the lights only show in the web UI. |
| `hue-jack discover` | Print bridges found via mDNS |
| `hue-jack pair [--bridge ip]` | Prompt "press the link button", poll, save credentials |
| `hue-jack areas` | List entertainment areas: id, name, channel count and positions |
| `hue-jack test-pattern [--area id] [--pattern chase\|strobe\|rainbow\|identify] [--secs 20]` | Stream a pattern. `identify` lights each channel in turn and prints its id. |
| `hue-jack gen-test-audio <dir>` | Write synthetic WAVs (below) |
| `hue-jack analyze <wav> [--csv out]` | Print or emit feature frames |
| `hue-jack simulate <wav> --out preview.html [--effect] [--palette] [--layout fake6]` | Self-contained HTML: audio + an animated light layout from the real pipeline |

### Web API (axum) and UI
- REST under `/api`:
  - `GET /status`: sources, now playing, stream state, levels, bpm, delay, drift stats, version.
  - `GET /bridges/discover`, `POST /bridge/pair` (async; poll `GET /bridge/pair`), `GET /areas`, `PUT /settings` (area, effect, palette, intensity, brightness, delay, idle).
  - `POST /calibration {on}`, `POST /test-pattern`.
  - `POST /bluetooth/pairing {seconds}`, `GET /bluetooth/devices`, `DELETE /bluetooth/devices/{addr}`.
- WebSocket `/ws`: at 20 Hz pushes `{levels, bands, onset, bpm, channels:[{id,x,y,rgb}]}` for the meters and the **virtual lights** view.
- UI pages (mobile-first, one page with sections is fine): Now playing and levels; Lights (virtual layout, pair bridge, area picker, test pattern); Effect (effect, palette, intensity, brightness); Calibration (toggle click mode, `D` slider at 0–600 ms, step 5); Bluetooth (pair button with countdown, bonded list); System (version, image digest from `bootc status --json` if available).
- Embed `web/dist` with `rust-embed`. Commit a placeholder `web/dist/index.html` so `cargo build` works before the web build.

### Calibration mode
Injects a click (1 kHz, 10 ms) every 750 ms *into the delay line input*. That way it goes through the same delay as music, and the lights flash full white on each click. Music input is muted while calibration is on.

### Test audio (`gen-test-audio`, generated, never committed)
- `clicks_120.wav`: clicks at exactly 120 BPM, 30 s.
- `drums_128.wav`: synthesised kick, snare and hi-hat at 128 BPM, plus a sine bass line, 60 s. Ground-truth kick times go in `drums_128.onsets.json`.
- `sweep.wav`: log sine sweep 20 Hz to 16 kHz, 20 s.
- `silence_gap.wav`: 10 s drums, 30 s silence, 10 s drums.

## Milestones

Each milestone ends with its acceptance checks passing locally *and* in CI (where CI can run them), then a commit and push.

### M0: scaffold and CI
- Cargo workspace, crates, `web/` (Vite TS + Preact), `ytcr/` (TS, `tsc` build, `vitest`), README, CLAUDE.md is already present, `.gitignore`.
- `.github/workflows/ci.yml` (push + PR):
  - **rust** job in `container: fedora:44`: `dnf install -y rust cargo clippy rustfmt pipewire-devel clang-devel openssl-devel dbus-devel pkgconf-pkg-config`, then `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`.
  - **node** (pnpm workspace `web` + `ytcr`, one lockfile): `pnpm install --frozen-lockfile && pnpm -r run build && pnpm -r test`.
- **Accept:** CI green on `main`.

### M1: Hue client + fake bridge
- `hue/huestream.rs` encoder with **golden-byte tests**.
- `hue/clip.rs`, `hue/pairing.rs`, `hue/discovery.rs`, `hue/stream.rs`.
- `fake-bridge`, a library + binary:
  - HTTPS server on a configurable port (self-signed cert generated with `rcgen`, CN = fake bridge id) implementing `/api` (with a `press_link_button()` hook), `/auth/v1`, entertainment_configuration GET/PUT.
  - A DTLS-PSK server on configurable UDP (`openssl` `SslAcceptor` for DTLS, PSK server callback) that decodes HueStream packets and records `(timestamp, seq, channels)`.
  - A default area of 6 channels at plausible positions; fixed area id.
- hue-jack accepts `--bridge host:port` and `--dtls-port` overrides for tests.
- CLI commands `discover`, `pair`, `areas`, `test-pattern`.
- **Accept:**
  - The integration test pairs (including a failed attempt before the button press), lists areas, starts, streams `chase` for 3 s, and the fake bridge records ≥ 140 packets with the right config id and channel ids. Then it stops.
  - `hue-jack discover` on the dev machine prints `10.0.0.166 ecb5fafffea77674` (local only, not CI).

### M2: audio I/O + delay line
- File and PipeWire backends, the delay line, the drift guard.
- `tools/passthrough-test.sh`:
  1. `pactl load-module module-null-sink sink_name=hue-jack-dev-in` and `…-dev-out` (record the module ids; unload in `trap`).
  2. Run `hue-jack serve --virtual --input-sink hue-jack-dev-in --output hue-jack-dev-out` (the `--output` override bypasses the `alsa_output.` auto-pick).
  3. Play `clicks_120.wav` into `hue-jack-dev-in` with `pw-play --target hue-jack-dev-in`; record `hue-jack-dev-out.monitor` with `pw-record`.
  4. Cross-correlate the input and output (a small `hue-jack` dev subcommand or a test helper) and assert the measured delay = `D` ± 2 ms for D = 150 and D = 300.
- **Accept:** unit tests for exact sample delay, runtime `D` change without discontinuity (max sample-to-sample jump bounded), and drift-guard behaviour. The passthrough script passes locally. A 10-minute local soak shows the drift counter stable and no xruns logged.

### M3: analysis
- Bands, AGC, onsets with look-ahead, tempo, silence, as in PLAN §9.
- `gen-test-audio`, `analyze`.
- **Accept (CI):**
  - `clicks_120`: ≥ 95 % of clicks detected, timing error ≤ 10 ms, BPM 120 ± 1.
  - `drums_128`: kick recall ≥ 90 % against ground truth, false positives ≤ 10 %, BPM 128 ± 1.
  - `sweep`: the band with the most energy moves monotonically from sub to air.
  - `silence_gap`: silent is flagged within 1.2 s of the gap starting and cleared within 0.2 s of it ending.
  - Analysis of a 60 s file runs in < 1 s in release mode.

### M4: effects + engine + simulate
- `Effect` trait; `pulse`, `spectrum`, `chase`; palettes; smoothing; gamma; brightness cap.
- `engine.rs` ties features → effect → `LightFrame { channels: Vec<(u8, [u16;3])> }` at 50 Hz, consumed by both the Hue streamer and the WS virtual view.
- `simulate` writes `preview.html`: the WAV embedded base64 (≤ 30 s clip), frames as JSON, and a canvas drawing channels at their x/y, synced to `audio.currentTime`.
- **Accept:**
  - Deterministic snapshot tests (fixed input → identical frames).
  - No channel changes more than its envelope allows between consecutive frames.
  - The brightness cap is respected.
  - `simulate` produces a valid HTML file for `drums_128` with every effect (checked headless: file exists, JSON parses, frame count = duration × 50 ± 1).

### M5: daemon, web UI, calibration
- `serve`: wires audio → analysis → engine → streamer; state persistence; auto start and stop; calibration mode; full REST + WS; UI built and embedded.
- **Accept:**
  - The end-to-end test (CI, file audio backend + fake-bridge) plays `silence_gap.wav` through `serve`. The fake bridge sees `start`, packets at ≥ 45/s during drums, `stop` after the idle period, then `start` again.
  - API tests for every endpoint.
  - Locally: `serve --virtual` with the passthrough sinks shows moving virtual lights in the UI. Take a screenshot with the Chrome tools if available, or check the WS payloads with a script.

### M6: sources
**Bluetooth** (`bluer`):
- Register a `NoInputNoOutput` agent that auto-accepts only while the pairing window is open.
- The pairing window sets the adapter `Discoverable` + `Pairable` with a timeout. List bonded devices; remove a device; read and pause via MPRIS.
- Config files for the image:
  - WirePlumber `wireplumber.conf.d/50-hue-jack-bluetooth.conf`:
    - `monitor.bluez.properties = { bluez5.roles = [ a2dp_sink ], bluez5.codecs = [ sbc sbc_xq aac ldac aptx aptx_hd ] }`
    - **`monitor.bluez.seat-monitoring = disabled`** (headless, no active logind seat).
    - HFP/HSP off.
  - `/etc/bluetooth/main.conf`: `Name = hue-jack`, `Class = 0x240414`, `DiscoverableTimeout = 0`, `AlwaysPairable = false`.
  - A D-Bus policy `/usr/share/dbus-1/system.d/hue-jack.conf` allowing user `huejack` to talk to `org.bluez` (Agent registration, Adapter1 properties, Device1).
- Unit-test the agent/window state machine with a mock. **Don't run it against the dev machine's adapter**, apart from read-only adapter enumeration.

**MPRIS watcher:** watches the `huejack` session bus for `org.mpris.MediaPlayer2.*` (sources: `mpris-proxy` for Bluetooth AVRCP, shairport-sync). It maps players to sources, reads metadata and PlaybackStatus, and can call `Pause`.

**YouTube sidecar (`ytcr/`):**
- `yt-cast-receiver` (^2.1.1 from npm; check `volumio-ytcr/dep/` for a patched tgz and whether those patches are needed), `youtubei.js` matching its peer version, PO-token generation the way `volumio-ytcr`'s `InnertubeLoader` does it.
- Port `VideoLoader` (MIT, with attribution), **including the `ctt` credential-transfer handling** and the client fallback order: YTMUSIC → TV for music; ANDROID_VR for video; WEB for live.
- Format preference: itag 774 > 141 > 251 > 140. **Log the chosen itag and bitrate per track** at info level (needed to check Premium quality at home).
- Player: `mpv --idle=yes --no-video --no-terminal --input-ipc-server=$XDG_RUNTIME_DIR/hue-jack/mpv.sock --ao=pipewire`, playing into the default sink, which is `hue-jack-in` on the appliance. Implement the `Player` class methods over the mpv JSON IPC (play, pause, resume, stop, seek, volume, position, duration).
- Device name `hue-jack`, port 8098.
- Control API: HTTP on unix socket `$XDG_RUNTIME_DIR/hue-jack/ytcr.sock`: `GET /status` → `{state, title, artist, album, thumbnail, itag, bitrate}`, `POST /pause`.
- **Tests:** unit-test the mpv IPC client against a fake socket. **Local smoke test (once, then stop):**
  1. Resolve a well-known public video id with `VideoLoader` to a stream URL.
  2. Play 10 s through mpv into a `hue-jack-dev-in` null sink, and confirm samples arrive.
  3. Start the receiver for ≤ 60 s and confirm the DIAL device description answers at `http://127.0.0.1:8098/` and SSDP M-SEARCH sees it.
  4. Stop it.

**AirPlay:** a shairport-sync config file:
- `general.name = "hue-jack"`, `pw` or `pa` backend into the default sink, MPRIS interface enabled.
- Check in the Containerfile whether Fedora's build has AirPlay 2 (`shairport-sync -V` contains `AirPlay2`). If it doesn't, build shairport-sync from source with `--with-airplay-2 --with-pw --with-mpris-interface --with-avahi --with-ssl=openssl`.
- Build `nqptp` from source (a systemd system service, root).

**Arbitration:** when a source goes to Playing, call Pause on the others (MPRIS or ytcr). `/api/status` shows the active source.

### M7: OS image, GHCR and ISO
`os/Containerfile`, multi-stage:
1. **Builder stage** `FROM quay.io/fedora/fedora:44`: build hue-jack in release mode (after `pnpm --filter hue-jack-web build`), build the `ytcr` dist and `pnpm deploy --prod` it, and build nqptp (and shairport-sync if needed).
2. **Final stage** `FROM quay.io/fedora/fedora-bootc:44`:
   - `dnf install` pipewire, wireplumber, pipewire-pulseaudio, pipewire-alsa, bluez, avahi, nss-mdns, mpv, nodejs, shairport-sync (unless built from source), alsa-utils, then `dnf clean all`.
   - Copy the binaries, `ytcr`, and `os/rootfs/`.
   - sysusers.d: user `huejack` (groups `audio`, `bluetooth` if present).
   - tmpfiles.d: `/var/lib/hue-jack` owned by `huejack`; **`f /var/lib/systemd/linger/huejack`** (bootc only seeds `/var` at install, so tmpfiles is the reliable way).
   - User units in `/usr/lib/systemd/user/`: `hue-jack.service` (`Environment=HUEJACK_STATE_DIR=/var/lib/hue-jack`, `ExecStart=/usr/bin/hue-jack serve --listen 0.0.0.0:80`), `hue-jack-ytcr.service`, `hue-jack-shairport.service`, `mpris-proxy.service` (if bluez doesn't ship one). Enable them for the user with `systemctl --global enable` (or `default.target.wants` symlinks).
   - The PipeWire null sink `hue-jack-in` via `/usr/share/pipewire/pipewire.conf.d/50-hue-jack.conf` (`support.null-audio-sink`, `node.name = hue-jack-in`, `media.class = Audio/Sink`, `audio.position = [ FL FR ]`, rate 48000). WirePlumber rules give it `priority.session = 2500`, so it's the default sink, while hue-jack targets ALSA explicitly.
   - System units: `bluetooth.service`, `avahi-daemon.service`, `nqptp.service` enabled; `bootc-fetch-apply-updates.timer` **masked** (no surprise reboots).
   - `sysctl.d`: `net.ipv4.ip_unprivileged_port_start = 80`.
   - `modprobe.d`: `blacklist iwlwifi` (Wi-Fi off; BT keeps working because it's `btusb`/`btintel`).
   - `/etc/hostname`: `hue-jack`.
   - `/etc/sudoers.d/wheel-nopasswd`: `%wheel ALL=(ALL) NOPASSWD: ALL`.
   - `LABEL org.opencontainers.image.source=https://github.com/axle-h/hue-jack`.
   - `RUN bootc container lint` at the end.

`os/bib/config.toml` for bootc-image-builder:
- User `alex`, groups `["wheel"]`, `key = "<contents of ~/.ssh/id_ed25519.pub on the dev machine>"`.
- Kickstart: `text --non-interactive`, `zerombr`, `clearpart --all --initlabel --disklabel=gpt`, `autopart --noswap`, `network --bootproto=dhcp --device=link --activate --onboot=on --hostname=hue-jack`, `reboot --eject`.
- It wipes the target disk. That's intended, and called out in the runbook.

Workflows:
- `.github/workflows/image.yml` (push to `main` touching `os/**`, `crates/**`, `web/**`, `ytcr/**`; plus `workflow_dispatch`): build with podman/buildah on `ubuntu-24.04` and push `ghcr.io/axle-h/hue-jack:latest` and `:sha-<short>` using `GITHUB_TOKEN` (`permissions: packages: write`).
- `.github/workflows/iso.yml` (`workflow_dispatch` + after a successful `image.yml` on `main`):
  - Free disk space (remove `/usr/share/dotnet`, `/opt/ghc`, `/usr/local/lib/android`).
  - `sudo podman pull` the image, then `sudo podman run --rm --privileged --pull=newer --security-opt label=type:unconfined_t -v ./os/bib/config.toml:/config.toml:ro -v ./output:/output -v /var/lib/containers/storage:/var/lib/containers/storage quay.io/centos-bootc/bootc-image-builder:latest --type anaconda-iso --rootfs xfs --use-librepo=True ghcr.io/axle-h/hue-jack:latest`.
  - Upload `output/bootiso/install.iso` as artifact `hue-jack-iso` (retention 14 days).

**Accept:**
- Locally: `podman build -f os/Containerfile -t localhost/hue-jack:dev .` succeeds; `podman run --rm localhost/hue-jack:dev hue-jack --version` works; `podman run --rm localhost/hue-jack:dev shairport-sync -V` shows AirPlay2; `bootc container lint` passes.
- In CI: `image.yml` and `iso.yml` succeed, and the ISO artifact exists.
- **Anonymous pull:** `skopeo inspect docker://ghcr.io/axle-h/hue-jack:latest` without credentials. If it fails because the package is private, add a step for Alex to `HOME-RUNBOOK.md` H0: make the package public at `https://github.com/users/axle-h/packages/container/hue-jack/settings`.

### M8: wrap-up
- `docs/STATUS.md` covering:
  - What's done.
  - Test results.
  - **Verified locally** vs **untested on hardware** (bridge pairing and DTLS against the real bridge, bulbs, the NUC's audio output, Bluetooth pairing, YouTube casting from a phone, Premium itag, AirPlay, the bridge restoring light state after stop).
  - Known issues.
  - The run URL and artifact name of the latest ISO.
- Update `HOME-RUNBOOK.md` if any commands, names or ports changed.
- Final push; CI, image and ISO all green.
