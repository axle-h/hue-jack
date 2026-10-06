#!/usr/bin/env bash
# M2 passthrough test: measure hue-jack's end-to-end audio delay on dedicated null sinks.
#
#   tools/passthrough-test.sh [delay_ms ...]        (default: 150 300)
#
# Creates hue-jack-dev-in and hue-jack-dev-out null sinks (unloaded on exit), runs
# `hue-jack serve --virtual` between them, plays clicks into dev-in, and records both monitors
# sample-aligned in one 4-channel pw-record stream (linked by hand with pw-link). The delay
# between the two is measured by cross-correlation and must be D ± 2 ms.
#
# It never changes the default sink or source, and fails if the configured defaults change.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=${HUEJACK_BIN:-target/release/hue-jack}
TOLERANCE_MS=${TOLERANCE_MS:-2}
DELAYS=("${@:-150}")
[[ $# -eq 0 ]] && DELAYS=(150 300)
WORK=$(mktemp -d -t hue-jack-passthrough.XXXXXX)
MODULES=()
PIDS=()

configured_defaults() {
    pw-metadata 0 2>/dev/null | grep -E "default\.configured\.audio\.(sink|source)" | sed -E 's/^update: id:[0-9]+ //' | sort || true
}

cleanup() {
    for pid in "${PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
    wait 2>/dev/null || true
    for m in "${MODULES[@]}"; do pactl unload-module "$m" 2>/dev/null || true; done
    rm -rf "$WORK"
}
trap cleanup EXIT

before=$(configured_defaults)

load_sink() {
    pactl load-module module-null-sink "sink_name=$1" "sink_properties=device.description=$1"
}
MODULES+=("$(load_sink hue-jack-dev-in)")
MODULES+=("$(load_sink hue-jack-dev-out)")

[[ -x $BIN ]] || { echo "build first: cargo build --release -p hue-jack" >&2; exit 1; }
[[ -f test-audio/clicks_120.wav ]] || "$BIN" gen-test-audio test-audio >/dev/null

wait_for_port() { # node port
    for _ in $(seq 50); do
        pw-link -i 2>/dev/null | grep -qx "$1:$2" && return 0
        sleep 0.1
    done
    echo "port $1:$2 did not appear" >&2
    return 1
}

failures=0
for D in "${DELAYS[@]}"; do
    "$BIN" --state-dir "$WORK/state" serve --virtual --listen 127.0.0.1:0 \
        --input-sink hue-jack-dev-in --output hue-jack-dev-out --delay-ms "$D" >"$WORK/serve_$D.log" 2>&1 &
    serve_pid=$!
    PIDS+=("$serve_pid")
    sleep 1.5

    pw-record --target 0 --channels 4 --channel-map FL,FR,RL,RR \
        -P '{ node.name = "hue-jack-dev-rec" }' "$WORK/rec_$D.wav" &
    rec_pid=$!
    PIDS+=("$rec_pid")
    wait_for_port hue-jack-dev-rec input_RR
    pw-link hue-jack-dev-in:monitor_FL hue-jack-dev-rec:input_FL
    pw-link hue-jack-dev-in:monitor_FR hue-jack-dev-rec:input_FR
    pw-link hue-jack-dev-out:monitor_FL hue-jack-dev-rec:input_RL
    pw-link hue-jack-dev-out:monitor_FR hue-jack-dev-rec:input_RR

    timeout 8 pw-play --target hue-jack-dev-in test-audio/clicks_120.wav || true
    sleep 0.6
    kill -INT "$rec_pid"; wait "$rec_pid" 2>/dev/null || true
    kill -INT "$serve_pid"; wait "$serve_pid" 2>/dev/null || true

    [[ -n ${KEEP_LOGS:-} ]] && cp "$WORK/serve_$D.log" "$KEEP_LOGS/serve_$D.log"
    measured=$("$BIN" measure-delay "$WORK/rec_$D.wav")
    if awk -v m="$measured" -v d="$D" -v t="$TOLERANCE_MS" 'BEGIN { e = m - d; if (e < 0) e = -e; exit !(e <= t) }'; then
        echo "PASS  D = $D ms  measured $measured ms"
    else
        echo "FAIL  D = $D ms  measured $measured ms (tolerance ±$TOLERANCE_MS ms)"
        sed 's/^/    serve: /' "$WORK/serve_$D.log" | tail -5
        failures=$((failures + 1))
    fi
done

after=$(configured_defaults)
if [[ "$before" != "$after" ]]; then
    echo "FAIL  configured default devices changed:" >&2
    echo "  before: $before" >&2
    echo "  after:  $after" >&2
    failures=$((failures + 1))
fi
exit $((failures > 0))
