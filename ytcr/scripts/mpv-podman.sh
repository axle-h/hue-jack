#!/usr/bin/env bash
# Runs Fedora 44's mpv from a rootless podman container, for dev machines without mpv installed.
# It talks to the host's PipeWire and creates its IPC socket under $XDG_RUNTIME_DIR, so it can stand in
# for mpv via HUEJACK_MPV_BIN=scripts/mpv-podman.sh.
set -euo pipefail

IMAGE=localhost/hue-jack-mpv:dev
: "${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set}"

if ! podman image exists "$IMAGE"; then
  podman build -q -t "$IMAGE" -f - . >&2 <<'EOF'
FROM quay.io/fedora/fedora:44
RUN dnf install -y --setopt=install_weak_deps=False mpv pipewire-libs && dnf clean all
EOF
fi

# --init: as PID 1, mpv would ignore the SIGTERM podman forwards and outlive us.
# Leftovers, if any: podman ps --filter label=hue-jack-dev-mpv
exec podman run --rm -i --init --network host --userns keep-id --security-opt label=disable \
  --label hue-jack-dev-mpv=1 \
  -v "$XDG_RUNTIME_DIR:$XDG_RUNTIME_DIR" -e XDG_RUNTIME_DIR \
  "$IMAGE" mpv "$@"
