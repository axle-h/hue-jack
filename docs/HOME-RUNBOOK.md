# hue-jack: home runbook

Hardware verification and install, for Alex with an agent on the dev machine (10.0.0.161). Run the steps in order. If a step fails, fix it in the repo (commit, push, let CI rebuild) before moving on, and record the outcome in `docs/STATUS.md`.

Before starting, read `docs/STATUS.md` for anything the build left untested or changed.

## H0: before you start
- [ ] If `STATUS.md` says the GHCR package is private, make it public at `https://github.com/users/axle-h/packages/container/hue-jack/settings`. It only matters for future `bootc upgrade`s.
- [ ] Web UI built on the dev machine, so local builds embed it (the build packages are already installed):
  ```sh
  pnpm install && pnpm --filter hue-jack-web build
  ```
- [ ] Bulbs powered on (wall switches on), and visible in the Hue app.
- [ ] In the Hue app: Settings → Entertainment areas. Make sure there's an area containing the lights you want (**max 10 colour lights**), positioned roughly where they are in the room. Name it e.g. `hue-jack`.
- [ ] Close the Hue Sync app or desktop app if open. Only one entertainment stream can run at once.

## H1: bridge from the dev machine
```sh
cargo run --release -p hue-jack -- discover          # expect: 10.0.0.166  ecb5fafffea77674
cargo run --release -p hue-jack -- pair              # press the round link button on the bridge when prompted (30 s)
cargo run --release -p hue-jack -- areas             # expect: your area(s), channel ids and positions
```
- [ ] Pairing saved credentials to `~/.local/state/hue-jack/state.json`.

## H2: stream test patterns (DTLS + bulbs)
```sh
cargo run --release -p hue-jack -- test-pattern --area <id> --pattern identify --secs 30
cargo run --release -p hue-jack -- test-pattern --area <id> --pattern chase --secs 20
cargo run --release -p hue-jack -- test-pattern --area <id> --pattern strobe --secs 10
```
- [ ] `identify`: each light lights in turn, matching the printed channel ids and positions.
- [ ] `chase`: smooth movement in left-to-right order.
- [ ] `strobe`: visible at about 2–4 Hz without dropped flashes.
- [ ] After the pattern ends, the lights **return to their previous state**. If not, note it in STATUS.md; the snapshot-and-restore fallback is needed.
- [ ] (Optional) Latency: film the screen output of the `strobe` log and a bulb at 240 fps, and note frames × 4.17 ms in STATUS.md.

## H3: full pipeline on the dev machine (no NUC yet)
```sh
cargo run --release -p hue-jack -- gen-test-audio test-audio
tools/passthrough-test.sh                       # sanity check: delay measured correctly (creates and removes its own sinks)
tools/dev-sinks.sh up                           # dedicated null sinks hue-jack-dev-in / hue-jack-dev-out
cargo run --release -p hue-jack -- serve --input-sink hue-jack-dev-in --output hue-jack-dev-out
# in another terminal:
pw-play --target hue-jack-dev-in test-audio/drums_128.wav
# afterwards:
tools/dev-sinks.sh down
```
Don't pass `--sources` on the dev machine: it registers a Bluetooth pairing agent and watches (and pauses) desktop media players.
- [ ] Open `http://localhost:8080`, select the area, pick `pulse`. The bulbs react to the drums.
- [ ] Try `spectrum` and `chase` too, and play some real music through the dev sink (e.g. `pw-play` on any local file).
- [ ] After about 20 s of silence the stream stops and the lights return to normal.

## H4: build the installer USB
```sh
gh run list -R axle-h/hue-jack -w iso.yml -L 1           # latest successful ISO run
gh run download -R axle-h/hue-jack <run-id> -n hue-jack-iso -D ~/Downloads/hue-jack-iso
lsblk -o NAME,SIZE,MODEL,TRAN                            # identify the USB stick carefully
sudo dd if=~/Downloads/hue-jack-iso/install.iso of=/dev/sdX bs=4M status=progress oflag=sync
```
> ⚠️ The installer is **fully automatic and wipes the NUC's internal disk**. Only boot it on the NUC.

## H5: install on the NUC
- [ ] NUC connected to **Ethernet** (Wi-Fi is disabled in the image), 3.5 mm jack → powered speakers, speakers on with the volume low.
- [ ] Boot from USB: press **F10** at power-on for the boot menu and choose the USB stick. Installation runs unattended and reboots. Remove the stick when it ejects.
- [ ] From the dev machine:
  ```sh
  ssh alex@hue-jack.local
  sudo bootc status                                           # booted image ghcr.io/axle-h/hue-jack:latest
  sudo -u huejack XDG_RUNTIME_DIR=/run/user/$(id -u huejack) systemctl --user status hue-jack hue-jack-ytcr hue-jack-shairport pipewire wireplumber
  sudo -u huejack XDG_RUNTIME_DIR=/run/user/$(id -u huejack) wpctl status   # hue-jack-in is the default sink; an alsa_output sink exists
  ```

## H6: configure the appliance
- [ ] Open `http://hue-jack.local`.
- [ ] Pair the bridge: press the link button, then click Pair in the UI. Alternatively, copy the dev machine's credentials:
  `scp ~/.local/state/hue-jack/state.json alex@hue-jack.local:/tmp/ && ssh alex@hue-jack.local 'sudo install -o huejack -g huejack -m 600 /tmp/state.json /var/lib/hue-jack/state.json && sudo -u huejack XDG_RUNTIME_DIR=/run/user/$(id -u huejack) systemctl --user restart hue-jack'`
- [ ] Select the area. Run the test pattern from the UI.

## H7: Bluetooth + calibration
- [ ] UI → Bluetooth → "Pair new device". On the phone, pair with **hue-jack**. It should pair with no PIN (the phone may show a code to confirm: tap Pair; hue-jack accepts automatically while the window is open) and show as a speaker or headphones, **not** as a hands-free or call device.
- [ ] Play music from any app (e.g. Amazon Music). Sound comes from the speakers and the lights react.
- [ ] The phone's volume buttons change the NUC's output volume.
- [ ] UI → Calibration on. Adjust the `D` slider until the click and the flash coincide. Calibration off.
- [ ] Note the final `D` in STATUS.md.
- [ ] Turn phone Bluetooth off and on: it reconnects without re-pairing.
- [ ] Listen for dropouts over a few minutes. If there are any, note them; the first suspect is 2.4 GHz interference.

## H8: YouTube Music cast
- [ ] Phone on the same Wi-Fi, YT Music app → cast icon → **hue-jack** appears → play a song.
- [ ] Sound and lights; play, pause and skip from the phone all work.
- [ ] Check the format log: `ssh alex@hue-jack.local 'sudo journalctl _SYSTEMD_USER_UNIT=hue-jack-ytcr.service --since -10min | grep -i itag'`. **itag 141 or 774 means Premium 256 kbps** is coming through; 140 or 251 means it isn't (the cookie fallback is in PLAN §2).
- [ ] Start Bluetooth playback while casting: the cast pauses and Bluetooth takes over.
- [ ] Afterwards, check whether the cast songs show up in YT Music history and note the result.

## H9: AirPlay (when a friend with an iPhone is around)
- [ ] iPhone Control Centre → AirPlay → **hue-jack** → play. Sound and lights work.

## H10: updates later
```sh
ssh alex@hue-jack.local 'sudo bootc upgrade && sudo systemctl reboot'
# rollback if needed:
ssh alex@hue-jack.local 'sudo bootc rollback && sudo systemctl reboot'
```

## Troubleshooting
| Symptom | Check |
|---|---|
| `pair` keeps saying the link button isn't pressed | Press the button *after* the prompt; the 30 s window starts at the press |
| DTLS handshake fails | `hue-application-id` used as the PSK identity (not the username); clientkey hex-decoded; area not in use by Hue Sync |
| Lights stutter | Bridge Wi-Fi/Zigbee interference; lower the effect speed; check the packet rate in `/api/status` |
| No sound on the NUC | `wpctl status`: hue-jack's playback node is linked to `alsa_output…`; volume not 0; `output` in state.json |
| Phone doesn't see hue-jack over BT | Pairing window still open? `bluetoothctl show` → Discoverable yes; `rfkill list` |
| Not in the cast menu | Phone and NUC on the same subnet; SSDP multicast not blocked by the AP (client isolation off); `curl http://hue-jack.local:8098/ytcr/ssdp/device-desc.xml` answers |
| Bluetooth section says unavailable | `hue-jack.service` runs with `--sources`; `journalctl _SYSTEMD_USER_UNIT=hue-jack.service` for BlueZ / D-Bus policy errors |
