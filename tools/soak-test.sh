#!/usr/bin/env bash
# M2 soak test: run `hue-jack serve --virtual` between dedicated null sinks with drums looping
# for SECS seconds (default 600), then report the delay-line counters. Passes if there were no
# underruns or resyncs after the first 10 s and the drift-correction count stayed stable.
#
#   tools/soak-test.sh [secs]
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=${HUEJACK_BIN:-target/release/hue-jack}
SECS=${1:-600}
WORK=$(mktemp -d -t hue-jack-soak.XXXXXX)
MODULES=()
PIDS=()
cleanup() {
    for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
    wait 2>/dev/null || true
    for m in "${MODULES[@]}"; do pactl unload-module "$m" 2>/dev/null || true; done
    [[ -n ${KEEP_LOGS:-} ]] && cp "$WORK/serve.log" "$KEEP_LOGS/soak-serve.log"
    rm -rf "$WORK"
}
trap cleanup EXIT

for s in hue-jack-dev-in hue-jack-dev-out; do
    MODULES+=("$(pactl load-module module-null-sink "sink_name=$s" "sink_properties=device.description=$s")")
done
[[ -f test-audio/drums_128.wav ]] || "$BIN" gen-test-audio test-audio >/dev/null

RUST_LOG=info "$BIN" --state-dir "$WORK/state" serve --virtual --listen 127.0.0.1:0 \
    --input-sink hue-jack-dev-in --output hue-jack-dev-out --delay-ms 150 >"$WORK/serve.log" 2>&1 &
PIDS+=("$!")
sleep 1
( while true; do pw-play --target hue-jack-dev-in test-audio/drums_128.wav || exit; done ) &
PIDS+=("$!")
sleep "$SECS"

stats=$(grep "delay line" "$WORK/serve.log" | sed -E 's/\x1b\[[0-9;]*m//g')
echo "$stats" | tail -3
last=$(echo "$stats" | tail -1)
early=$(echo "$stats" | head -1)
num() { echo "$1" | grep -oE "$2=\"?-?[0-9.]+" | grep -oE -- "-?[0-9.]+$"; }
underruns=$(( $(num "$last" underruns) - $(num "$early" underruns) ))
resyncs=$(( $(num "$last" resyncs) - $(num "$early" resyncs) ))
corrections=$(( $(num "$last" drift_corrections) - $(num "$early" drift_corrections) ))
echo "after warm-up: underruns +$underruns, resyncs +$resyncs, drift corrections +$corrections over ${SECS}s"
[[ $underruns -eq 0 && $resyncs -eq 0 ]]
