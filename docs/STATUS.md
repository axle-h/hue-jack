# hue-jack: status

State after the unattended build on 2026-10-06. Everything below "Untested on hardware" was built against the documented assumptions and covered with fakes; [`HOME-RUNBOOK.md`](HOME-RUNBOOK.md) is the checklist that verifies it.

## Milestones

| Milestone | State | Acceptance |
|---|---|---|
| M0 scaffold + CI | done | `ci.yml` green on `main` (rust in `fedora:44`, web, ytcr) |
| M1 Hue client + fake bridge | done | Integration test pairs (refused first, then after the button), lists areas, streams `chase` for 3 s: the fake bridge records ≥ 140 packets (≈ 150) with the right area and channel ids, then `stop`. `hue-jack discover` on the dev machine printed `10.0.0.166  ecb5fafffea77674` |
| M2 audio I/O + delay line | done | Unit tests: exact sample delay at several quanta, crossfaded `D` changes (bounded sample-to-sample jump), drift guard both ways, latency-step resync. `tools/passthrough-test.sh`: D = 150 → 149.6–150.0 ms, D = 300 → 300.0 ms (±2 ms required), repeatable. 10-minute soak (`tools/soak-test.sh 600`): 0 underruns, 0 resyncs, 0 drift corrections, error 0 frames throughout |
| M3 analysis | done | clicks_120: 100 % detected, worst timing error 1.0 ms, 120 BPM ± 1. drums_128: kick recall 100 %, false positives 3.8 %, 128 BPM ± 1. sweep: dominant band monotonic sub → air. silence_gap: flagged 0.98 s after the gap starts, cleared at once when sound resumes. 60 s analysed in ≈ 0.12 s (release; CI asserts < 1 s) |
| M4 effects, engine, simulate | done | Deterministic frames, attack/decay envelope never exceeded, brightness cap respected, calibration flashes, test patterns; `simulate` writes a valid preview for drums_128 with every effect (30 s clip, 1500 frames) |
| M5 daemon, web UI, calibration | done | End-to-end (CI): file audio + fake bridge: `start` ≈ 1 s in, ≥ 45 packets/s during drums, `stop` after the idle period, `start` again, clean `stop` on SIGTERM. API tests for every endpoint and the WebSocket. Locally, `serve --virtual` between dev sinks: UI served, `/ws` at 20 Hz with moving virtual lights (`tools/virtual-ws-check.sh`; no screenshot, the Chrome extension wasn't connected) |
| M6 sources | done (no hardware) | Bluetooth window/agent state machine and API tested with a mock adapter; MPRIS mapping and the ytcr client tested; arbitration tested. ytcr sidecar: 44 unit tests; local smoke test resolved and played a public video (itag 251 via YTMUSIC) into a dev sink and answered DIAL + SSDP for < 60 s |
| M7 OS image, GHCR, ISO | done | Image builds in CI and passes its smoke test (`hue-jack --version`, `shairport-sync -V` shows AirPlay2); a local `podman build -f os/Containerfile -t localhost/hue-jack:dev .` with the final code passed, including `bootc container lint` (13 checks passed, 1 warning: a file under `/var` from the base image), and `hue-jack --version` and `shairport-sync -V` (`5.5.2-AirPlay2-…-PipeWire-…-mpris`) run in it. Anonymous pull works (`skopeo inspect --no-creds docker://ghcr.io/axle-h/hue-jack:latest`), so the package is public: no H0 step needed |
| M8 wrap-up | done | This file, runbook updates, all workflows green |

Latest ISO: run [37606676324](https://github.com/axle-h/hue-jack/actions/runs/37606676324), artifact `hue-jack-iso` (`install.iso`, 2.46 GB, expires 2026-10-21), built from the image of commit `5267e83` (tempo hold). Download with `gh run download -R axle-h/hue-jack 37606676324 -n hue-jack-iso -D ~/Downloads/hue-jack-iso`.

## Test results

- Rust (`cargo test --workspace`, CI): 44 unit tests; integration tests `hue_fake_bridge` (3), `analysis` (5 + release perf), `simulate` (1), `api` (6), `serve_e2e` (1). `local_readonly` (2, ignored by default) were run on the dev machine: read-only MPRIS listing and Bluetooth adapter enumeration (`hci0`).
- Web UI: 20 vitest tests (API client, WebSocket client, settings/calibration flow, virtual lights).
- ytcr: 44 vitest tests (mpv IPC client against a fake socket, format choice 774 > 141 > 251 > 140, stream proxy, control API, config).
- Local scripts: `tools/passthrough-test.sh`, `tools/soak-test.sh`, `tools/virtual-ws-check.sh` all pass. They use dedicated `hue-jack-dev-*` null sinks and check that the configured default devices don't change.

## Verified locally (dev machine, no bulbs)

- mDNS discovery of the real bridge; unauthenticated `GET /api/0/config` (bridge id `ECB5FAFFFEA77674`, certificate CN `ecb5fafffea77674`, compared case-insensitively).
- Pairing, CLIP v2, DTLS 1.2 PSK (`PSK-AES128-GCM-SHA256`) and HueStream v2 against the fake bridge, which uses the same OpenSSL and Fedora crypto policy (security level set to 1 and the cipher list set explicitly).
- PipeWire capture from a sink monitor and playback to a named node; the measured end-to-end delay equals `D`.
- Analysis, effects, engine, the web UI and API, the WebSocket, the stream lifecycle, YouTube stream resolution and mpv playback.

## Untested on hardware

- Pairing with and DTLS streaming to the **real bridge** (needs the link button), and the real bulbs: identify/chase/strobe, smoothness, latency.
- Whether the bridge **restores the lights' previous state** after `stop` (assumed; if not, the snapshot-and-restore fallback in BUILD.md is needed).
- The NUC: booting the ISO, the unattended install, lingering user services, PipeWire/WirePlumber as `huejack`, `hue-jack-in` as the default sink, `output = auto` picking the onboard `alsa_output` (the dev machine has no ALSA sink at all), the 3.5 mm output, and the drift guard against a real sound-card clock (locally both sinks share the system clock, so it never had to correct).
- Bluetooth: the pairing window and agent against BlueZ, phone pairing (expect a confirm prompt, see DECISIONS), A2DP-only profile (no hands-free), codecs, AVRCP absolute volume, reconnects, the D-Bus policy for `huejack`, `mpris-proxy` metadata and pause.
- YouTube Music casting from a phone (DIAL discovery, Lounge session, play/pause/skip), the `ctt` token, **Premium itag 141/774**, YT Music metadata, `POST /pause` reaching the phone, and whether casts appear in YT Music history.
- AirPlay 2 (shairport-sync 5.5.2 + nqptp 1.2.8 built from source), its MPRIS metadata and pause.
- Delay calibration by eye (the final `D`), and the strobe latency measurement.

## Known issues and notes

- The pairing agent is DisplayYesNo rather than NoInputNoOutput (a `bluer` limitation; see DECISIONS.md). Phones may show a code to confirm; it's accepted automatically while the window is open.
- Bluetooth and the source watchers only run with `serve --sources` (the appliance unit passes it); plain dev runs never touch the desktop's adapter or media players.
- Source metadata and arbitration poll once a second, so pausing the previous source can lag by up to a second.
- `pnpm audit` reports 2 moderate advisories, both in `yt-cast-receiver`'s dependencies (`uuid` via `peer-dial`, `decode-uri-component` via `query-string`); accepted for a LAN-only appliance.
- YouTube stream access changes often; the sidecar now tries clients in a chain and plays through a local range proxy (see DECISIONS.md). Expect to update `youtubei.js` from time to time.
- Dev machine: build packages are installed natively (`pipewire-devel clang-devel dbus-devel openssl-devel`), with rustup `stable` only. CI builds with Fedora 44's packaged Rust (1.98), which can lag the dev machine's stable (1.99 as of 2026-10-06), so a lint from a newer clippy may show up locally before CI.
