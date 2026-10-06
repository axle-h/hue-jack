#!/usr/bin/env bash
# Local smoke test for the YouTube sidecar (docs/BUILD.md M6). Run once, it stops everything it starts:
#   1. resolve a public video id to an audio stream URL;
#   2. play 10 s of it through mpv into a dedicated `hue-jack-dev-ytcr` null sink and check samples arrive;
#   3. run the receiver for at most 60 s and check the DIAL device description and an SSDP M-SEARCH;
#   4. stop it.
# Never touches the default sink or source. Uses mpv if installed, else scripts/mpv-podman.sh.
# Usage: scripts/smoke-test.sh [videoId]
set -euo pipefail
cd "$(dirname "$0")/.."

VIDEO_ID=${1:-dQw4w9WgXcQ}
SINK=hue-jack-dev-ytcr
NAME=hue-jack-dev-smoke
PORT=${HUEJACK_YTCR_PORT:-8098}
MPV_BIN=${HUEJACK_MPV_BIN:-$(command -v mpv || echo "$PWD/scripts/mpv-podman.sh")}
: "${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set}"
WORK=$(mktemp -d)
RUN_DIR="$XDG_RUNTIME_DIR/hue-jack-smoke"
MODULE=""
REC_PID=""
RECV_PID=""
# The user-configured defaults (what must never change). The automatic default can move on its own while dev
# sinks exist on a machine without a real sink, so it isn't compared.
configured_defaults() {
  pw-metadata 0 default.configured.audio.sink 2>/dev/null | grep -o "value:'[^']*'" || true
  pw-metadata 0 default.configured.audio.source 2>/dev/null | grep -o "value:'[^']*'" || true
}
DEFAULTS=$(configured_defaults)

cleanup() {
  [[ -n "$RECV_PID" ]] && kill "$RECV_PID" 2>/dev/null && wait "$RECV_PID" 2>/dev/null || true
  [[ -n "$REC_PID" ]] && kill "$REC_PID" 2>/dev/null || true
  [[ -n "$MODULE" ]] && pactl unload-module "$MODULE" || true
  rm -rf "$WORK" "$RUN_DIR"
}
trap cleanup EXIT

step() { printf '\n== %s\n' "$*"; }

pnpm -s run build

step "1. resolve $VIDEO_ID"
node dist/resolve.js "$VIDEO_ID" >"$WORK/info.json"
node -e 'const i=JSON.parse(require("fs").readFileSync(process.argv[1]));console.log(`${i.title}: itag ${i.itag}, ${i.bitrate} kbps, ${i.client}`)' "$WORK/info.json"

step "2. play 10 s through the sidecar's player (mpv: $MPV_BIN) into $SINK"
MODULE=$(pactl load-module module-null-sink sink_name=$SINK sink_properties=device.description=$SINK)
pw-record -P '{ stream.capture.sink = true }' --target "$SINK" --rate 48000 --channels 2 "$WORK/out.wav" &
REC_PID=$!
sleep 1
mkdir -p "$RUN_DIR"
HUEJACK_MPV_SOCKET="$RUN_DIR/mpv.sock" \
  HUEJACK_MPV_BIN="$MPV_BIN" \
  HUEJACK_MPV_EXTRA_ARGS="--audio-device=pipewire/$SINK" \
  node dist/play.js "$VIDEO_ID" --secs 10
sleep 0.5
kill -INT "$REC_PID"
wait "$REC_PID" || true
REC_PID=""
node scripts/wav-rms.mjs "$WORK/out.wav"

step "3. run the receiver ($NAME) for at most 60 s"
HUEJACK_YTCR_NAME=$NAME \
  HUEJACK_YTCR_PORT=$PORT \
  HUEJACK_YTCR_SOCKET="$RUN_DIR/ytcr.sock" \
  HUEJACK_MPV_SOCKET="$RUN_DIR/mpv.sock" \
  HUEJACK_YTCR_DATA_DIR="$WORK/data" \
  HUEJACK_MPV_BIN="$MPV_BIN" \
  HUEJACK_MPV_EXTRA_ARGS="--audio-device=pipewire/$SINK" \
  timeout 60 node dist/main.js >"$WORK/receiver.log" 2>&1 &
RECV_PID=$!
for _ in $(seq 1 40); do
  curl -sf "http://127.0.0.1:$PORT/ytcr/ssdp/device-desc.xml" >"$WORK/desc.xml" && break
  sleep 0.5
done
grep -o "<friendlyName>[^<]*</friendlyName>" "$WORK/desc.xml"
grep -q "<friendlyName>$NAME</friendlyName>" "$WORK/desc.xml"
echo "DIAL device description OK"
node scripts/ssdp-search.mjs 3 ":$PORT/ytcr/ssdp/device-desc.xml"
echo "SSDP M-SEARCH OK"
for _ in $(seq 1 20); do
  [[ -S "$RUN_DIR/ytcr.sock" ]] && break
  sleep 0.5
done
curl -sf --unix-socket "$RUN_DIR/ytcr.sock" http://localhost/status
echo
echo "control API OK"

step "4. stop the receiver"
kill -TERM "$RECV_PID"
wait "$RECV_PID" || true
RECV_PID=""
if curl -sf -o /dev/null "http://127.0.0.1:$PORT/ytcr/ssdp/device-desc.xml"; then
  echo "receiver still answering after stop" >&2
  exit 1
fi
echo "receiver stopped"
sed -n '1,40p' "$WORK/receiver.log"

[[ "$(configured_defaults)" == "$DEFAULTS" ]] || { echo "configured default sink/source changed!" >&2; exit 1; }
echo
echo "smoke test passed (configured default sink/source unchanged: ${DEFAULTS:-none set})"
