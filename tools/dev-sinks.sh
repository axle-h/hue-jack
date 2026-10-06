#!/usr/bin/env bash
# Create or remove the dedicated dev null sinks (hue-jack-dev-in, hue-jack-dev-out) used to run
# hue-jack on the dev machine without touching the real audio devices or the default sink.
#
#   tools/dev-sinks.sh up      # load both sinks (module ids remembered in $XDG_RUNTIME_DIR)
#   tools/dev-sinks.sh down    # unload them
set -euo pipefail
STATE=${XDG_RUNTIME_DIR:-/tmp}/hue-jack-dev-sinks
case "${1:-}" in
    up)
        if [[ -s $STATE ]]; then echo "already up (modules $(tr '\n' ' ' <"$STATE"))"; exit 0; fi
        for s in hue-jack-dev-in hue-jack-dev-out; do
            pactl load-module module-null-sink "sink_name=$s" "sink_properties=device.description=$s" >>"$STATE"
        done
        echo "loaded hue-jack-dev-in and hue-jack-dev-out (modules $(tr '\n' ' ' <"$STATE"))"
        ;;
    down)
        [[ -s $STATE ]] || { echo "not up"; exit 0; }
        while read -r m; do pactl unload-module "$m" || true; done <"$STATE"
        rm -f "$STATE"
        echo "unloaded"
        ;;
    *) echo "usage: $0 up|down" >&2; exit 2 ;;
esac
