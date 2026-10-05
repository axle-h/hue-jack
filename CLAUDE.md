# hue-jack

A headless music → Philips Hue appliance on an Intel NUC. It receives Bluetooth, AirPlay 2 and YouTube Music casts, plays the audio delayed by the light latency, and streams synced effects to the Hue bridge.

- Design: `docs/PLAN.md`. Build plan, ground rules and acceptance criteria: `docs/BUILD.md`. Hardware steps: `docs/HOME-RUNBOOK.md`.
- Current state: `docs/STATUS.md` (verified vs untested on hardware). Design changes: `docs/DECISIONS.md`.
- Follow the ground rules in `docs/BUILD.md` (dev-machine audio and Bluetooth safety, bridge access, no secrets).
- Rust workspace in `crates/`, web UI in `web/`, YouTube sidecar in `ytcr/`, bootc image in `os/`.
