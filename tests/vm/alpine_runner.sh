#!/bin/sh
set -eu

repo=/work/smolrunners
runtime=/opt/runtime
state=/work/tmp/alpine-runner-state
cache=/work/tmp/image-alpine-cache
machine=sr-alpine-runner

fail() { printf 'alpine_runner: %s\n' "$*" >&2; exit 1; }

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$state/home" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/home/.config" XDG_CACHE_HOME="$state/home/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" --state-dir "$state/smolvm" "$@"
}

exec_guest() { smolvm machine exec --name "$machine" --timeout 10m "$@"; }

cleanup() {
  smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
  smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
}
trap cleanup EXIT

wait_machine() {
  i=0
  while [ "$i" -lt 90 ]; do
    smolvm machine status --name "$machine" >/dev/null 2>&1 && return 0
    i=$((i + 1))
    sleep 1
  done
  fail "machine $machine did not reach running state"
}

assert_musl() {
  label=$1
  lines=$2
  printf '%s\n' "$lines" | grep -q 'ld-musl' || fail "$label is not musl linked: $lines"
  if printf '%s\n' "$lines" | grep -Eqi 'glibc|ld-linux|GLIBC_'; then
    fail "$label links glibc: $lines"
  fi
  printf 'alpine_runner: %s: %s\n' "$label" \
    "$(printf '%s\n' "$lines" | grep -m1 'ld-musl')"
}

rm -rf "$state"
mkdir -p "$state/tmp" "$state/home/.config" "$state/home/.cache" "$state/rootfs" "$cache"

[ -x "$runtime/smolvm" ] || fail 'missing pinned runtime'

printf 'alpine_runner: building the Alpine 3.24 rootfs\n'
sh "$repo/images/alpine324/build.sh" "$state/rootfs" "$cache"

printf 'alpine_runner: publishing the native musl-arm64 runner\n'
sh "$repo/images/alpine324/build-runner.sh" "$state/runner" "$cache"

printf 'alpine_runner: installing the runner into the rootfs\n'
mkdir -p "$state/rootfs/opt/runner/bin" \
  "$state/rootfs/opt/runner/externals/node24/bin" \
  "$state/rootfs/opt/smolrunners/fixtures"
cp -a "$state/runner/." "$state/rootfs/opt/runner/bin/"
ln -sf /usr/bin/node "$state/rootfs/opt/runner/externals/node24/bin/node"
cp "$repo/tests/fixtures/alpine-runner/host-step.js" \
  "$state/rootfs/opt/smolrunners/fixtures/host-step.js"

printf 'alpine_runner: booting the Alpine guest\n'
smolvm machine run -d --name "$machine" -I "$state/rootfs/" \
  -- /bin/sleep 7200 > "$state/boot.log" 2>&1 || \
  fail "guest boot failed: $(tail -10 "$state/boot.log")"
wait_machine

printf 'alpine_runner: Runner.Listener --version runs natively\n'
version=$(exec_guest -- /opt/runner/bin/Runner.Listener --version 2>&1) || \
  fail "Runner.Listener --version failed: $version"
[ "$version" = 2.338.2 ] || fail "unexpected runner version: $version"
printf 'alpine_runner: runner version=%s\n' "$version"
assert_musl Runner.Listener "$(exec_guest -- ldd /opt/runner/bin/Runner.Listener 2>&1)"

printf 'alpine_runner: Runner.Worker and Runner.PluginHost start natively\n'
set +e
worker_out=$(exec_guest -- /opt/runner/bin/Runner.Worker 2>&1)
worker_code=$?
set -e
[ "$worker_code" -ne 127 ] || fail "Runner.Worker loader failure: $worker_out"
case "$worker_out" in
  *'Error loading shared library'*) fail "Runner.Worker missing library: $worker_out" ;;
esac
printf '%s\n' "$worker_out" | grep -qi 'argument' || \
  fail "Runner.Worker produced no managed output: $worker_out"
printf 'alpine_runner: Runner.Worker exit=%s\n' "$worker_code"
assert_musl Runner.Worker "$(exec_guest -- ldd /opt/runner/bin/Runner.Worker 2>&1)"
assert_musl Runner.PluginHost "$(exec_guest -- ldd /opt/runner/bin/Runner.PluginHost 2>&1)"

printf 'alpine_runner: host-step Node runs a JS action entrypoint\n'
node_bin=/opt/runner/externals/node24/bin/node
node_version=$(exec_guest -- "$node_bin" --version 2>&1) || \
  fail "runner host-step node failed: $node_version"
case "$node_version" in
  v24.*) ;;
  *) fail "unexpected host-step node version: $node_version" ;;
esac
assert_musl host-step-node "$(exec_guest -- ldd "$node_bin" 2>&1)"
marker=$(exec_guest -e SMOLRUNNERS_ALPINE_MARKER=alpine-host-step-ok -- \
  "$node_bin" /opt/smolrunners/fixtures/host-step.js 2>&1) || \
  fail "JS action entrypoint failed: $marker"
[ "$marker" = alpine-host-step-ok ] || fail "JS action entrypoint output: $marker"
printf 'alpine_runner: host-step node=%s marker=%s\n' "$node_version" "$marker"

printf 'alpine_runner: no sr-* resources survive\n'
smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
leftovers="$(smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}')"
[ -z "$leftovers" ] || fail "leftover machines: $leftovers"

printf 'alpine_runner: ok\n'
