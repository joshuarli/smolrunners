#!/bin/sh
set -eu

repo=/work/smolrunners
runtime=/opt/runtime
state=/work/tmp/image-debian-state

fail() { printf 'image_debian: %s\n' "$*" >&2; exit 1; }

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$state" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/.config" XDG_CACHE_HOME="$state/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" --state-dir "$state/smolvm" "$@"
}

rm -rf "$state"
mkdir -p "$state/tmp" "$state/.config" "$state/.cache" "$state/rootfs" "$state/cache"

printf 'image_debian: building debian13 rootfs\n'
sh "$repo/images/debian13/build.sh" "$state/rootfs" "$state/cache"

printf 'image_debian: booting the pinned linux runtime on the built image\n'
smolvm machine run -I "$state/rootfs/" -- sh /opt/smolrunners/verify.sh

printf 'image_debian: fixture executor phases and job hooks\n'
hooks="$state/hooks"
mkdir -p "$hooks"
printf '#!/bin/sh\nprintf started > "%s/started"\n' "$hooks" > "$hooks/start.sh"
printf '#!/bin/sh\nprintf completed > "%s/completed"\n' "$hooks" > "$hooks/complete.sh"
chmod 0755 "$hooks/start.sh" "$hooks/complete.sh"
ACTIONS_RUNNER_HOOK_JOB_STARTED="$hooks/start.sh" \
ACTIONS_RUNNER_HOOK_JOB_COMPLETED="$hooks/complete.sh" \
  sh "$repo/tests/fixtures/executor" "$repo/tests/fixtures/scripts/basic.sh"
[ -f "$hooks/started" ] || fail 'ACTIONS_RUNNER_HOOK_JOB_STARTED was not called'
[ -f "$hooks/completed" ] || fail 'ACTIONS_RUNNER_HOOK_JOB_COMPLETED was not called'

leftovers="$(smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}')"
[ -z "$leftovers" ] || fail "leftover machines: $leftovers"

printf 'image_debian: ok\n'
