# hue-jack

Sync Philips Hue lights to whatever you're playing. A small appliance (Fedora bootc on an Intel NUC) acts as a Bluetooth speaker, AirPlay 2 receiver and YouTube Music cast target. It analyses the audio, drives a Hue Entertainment area over the local DTLS streaming API, and plays the music through its own output **delayed by the light latency**, so sound and light land together.

Everything stays on the LAN.

- Design: [`docs/PLAN.md`](docs/PLAN.md). Build plan: [`docs/BUILD.md`](docs/BUILD.md). Hardware steps: [`docs/HOME-RUNBOOK.md`](docs/HOME-RUNBOOK.md).
- What works and what's untested on hardware: [`docs/STATUS.md`](docs/STATUS.md). Deviations from the plan: [`docs/DECISIONS.md`](docs/DECISIONS.md).

## Layout

| Path | What |
|---|---|
| `crates/hue-jack` | The daemon and CLI (Rust): PipeWire capture/playback and delay line, analysis, effects, Hue CLIP v2 + DTLS streaming, web API, Bluetooth/MPRIS/YouTube sources |
| `crates/fake-bridge` | Test double for a Hue bridge: CLIP v2 over HTTPS and a DTLS-PSK HueStream receiver |
| `web/` | Web UI (Vite + TypeScript + Preact), embedded into the binary |
| `ytcr/` | YouTube Music cast receiver sidecar (Node/TypeScript, `yt-cast-receiver` + `youtubei.js` + mpv) |
| `os/` | bootc image (`Containerfile`, files under `rootfs/`) and the installer config (`bib/config.toml`) |
| `tools/` | Dev scripts: passthrough delay test, soak test, virtual-lights check, dev null sinks |

## Develop

Needs Fedora with `pipewire-devel clang-devel openssl-devel dbus-devel`, Rust stable, Node 24+ and pnpm (the version is pinned by `packageManager` in `package.json`; `corepack enable pnpm` picks it up).

```sh
pnpm install && pnpm -r run build               # web UI (embedded by cargo; else a placeholder page) + ytcr sidecar
cargo test --workspace
cargo run -p hue-jack -- gen-test-audio test-audio
cargo run -p hue-jack -- simulate test-audio/drums_128.wav --out preview.html --effect pulse
cargo run -p hue-jack -- --help
```

Run the daemon against dedicated null sinks (never the desktop's own devices):

```sh
tools/dev-sinks.sh up
cargo run -p hue-jack -- serve --virtual --input-sink hue-jack-dev-in --output hue-jack-dev-out
pw-play --target hue-jack-dev-in test-audio/drums_128.wav     # then open http://localhost:8080
tools/dev-sinks.sh down
```

`fake-bridge` runs a fake bridge for manual testing: `cargo run -p fake-bridge -- --link-button`, then `hue-jack --bridge 127.0.0.1:8443 --dtls-port 2100 pair`.

## CI

- `ci.yml`: Rust (fmt, clippy, tests, analysis performance) in a `fedora:44` container, web and ytcr builds and tests.
- `image.yml`: builds the bootc image and pushes `ghcr.io/axle-h/hue-jack:latest` and `:sha-<short>` from `main`.
- `iso.yml`: after each image on `main`, builds the unattended installer ISO (artifact `hue-jack-iso`). **The installer wipes the target disk.**
