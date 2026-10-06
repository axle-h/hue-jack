#!/usr/bin/env bash
# M5 local check: `serve --virtual` between dedicated null sinks with drums playing; reads the
# /ws stream and checks the virtual lights move and the UI is served. Never touches the defaults.
set -euo pipefail
cd "$(dirname "$0")/.."
BIN=${HUEJACK_BIN:-target/release/hue-jack}
PORT=${PORT:-18080}
WORK=$(mktemp -d -t hue-jack-virtual.XXXXXX)
MODULES=()
PIDS=()
cleanup() {
    for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
    wait 2>/dev/null || true
    for m in "${MODULES[@]}"; do pactl unload-module "$m" 2>/dev/null || true; done
    rm -rf "$WORK"
}
trap cleanup EXIT
for s in hue-jack-dev-in hue-jack-dev-out; do
    MODULES+=("$(pactl load-module module-null-sink "sink_name=$s" "sink_properties=device.description=$s")")
done
[[ -f test-audio/drums_128.wav ]] || "$BIN" gen-test-audio test-audio >/dev/null
"$BIN" --state-dir "$WORK/state" serve --virtual --listen "127.0.0.1:$PORT" \
    --input-sink hue-jack-dev-in --output hue-jack-dev-out >"$WORK/serve.log" 2>&1 &
PIDS+=("$!")
sleep 1
pw-play --target hue-jack-dev-in test-audio/drums_128.wav &
PIDS+=("$!")
sleep 2
curl -sf "http://127.0.0.1:$PORT/" | grep -q '<div id="app">' && echo "UI served"
curl -sf "http://127.0.0.1:$PORT/api/status" | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{const j=JSON.parse(s);console.log("status: stream",j.stream.state,"active",j.audio.active,"rms",j.audio.rms_db,"bpm",j.audio.bpm)})'
node - "$PORT" <<'JS'
const port = process.argv[2];
const ws = new WebSocket(`ws://127.0.0.1:${port}/ws`);
const frames = [];
ws.onmessage = (ev) => frames.push(JSON.parse(ev.data));
setTimeout(() => {
  ws.close();
  const colours = new Set(frames.flatMap((f) => f.channels.map((c) => c.rgb.join(','))));
  const onsets = frames.filter((f) => f.onset > 0).length;
  const bpm = frames.map((f) => f.bpm).filter(Boolean).pop();
  console.log(`ws: ${frames.length} frames in 3 s, ${frames[0]?.channels.length} lights, ${colours.size} distinct colours, ${onsets} onset frames, bpm ${bpm?.toFixed(1)}`);
  const ok = frames.length >= 50 && colours.size > 20 && onsets > 5;
  console.log(ok ? 'PASS virtual lights move' : 'FAIL');
  process.exit(ok ? 0 : 1);
}, 3000);
JS
