#!/bin/sh
set -eu

runtime=/opt/runtime
state=/work/tmp/smoke-state
mkdir -p "$state/tmp" "$state/.config" "$state/.cache"

smolvm() {
  env -i PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \
    HOME="$state" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/.config" XDG_CACHE_HOME="$state/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" "$@"
}

fail() { printf 'smoke: %s\n' "$*" >&2; exit 1; }

ALPINE='alpine@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf'
DEBIAN='debian@sha256:9c6e381421d0f48d1b8133e573f33c185ee16cbf505eda9c4a8a4791b25ea8a6'

smolvm machine run --name sr-smoke-alpine -I "$ALPINE" -- true
smolvm machine run --name sr-smoke-debian -I "$DEBIAN" -- true

leftovers="$(smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}')"
[ -z "$leftovers" ] || fail "leftover machines: $leftovers"

printf 'smoke: ok\n'
