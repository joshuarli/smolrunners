#!/bin/sh
set -eu

repo=/work/smolrunners
runtime=/opt/runtime
state=/work/tmp/sizing-trial-state
executor="$repo/tests/fixtures/executor"
ALPINE='alpine@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf'
machine_a=sr-sizing-a
machine_b=sr-sizing-b

fail() { printf 'sizing_trial: %s\n' "$*" >&2; exit 1; }

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$state/home" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/home/.config" XDG_CACHE_HOME="$state/home/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" --error-format json --state-dir "$state/smolvm" "$@"
}

leftovers() {
  smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}' || true
}

cleanup() {
  for machine in "$machine_a" "$machine_b"; do
    smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
    smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
  done
  for machine in $(leftovers); do
    smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
    smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
  done
}
trap cleanup EXIT

[ -x "$executor" ] || fail 'missing fixture executor'
[ -x "$runtime/smolvm" ] || fail 'missing pinned runtime'

rm -rf "$state"
mkdir -p "$state/tmp" "$state/home/.config" "$state/home/.cache" "$state/scenarios"

run_shape() {
  machine=$1
  cpus=$2
  mem=$3
  scenario=$4
  log=$5

  printf 'sizing_trial: booting %s at %sc/%sMiB\n' "$machine" "$cpus" "$mem"
  smolvm machine create --name "$machine" -I "$ALPINE" --cpus "$cpus" --mem "$mem" \
    -v "$repo:/work/smolrunners:ro" -v "$state/scenarios:/work/scenarios:ro" \
    >"$log.create" 2>&1 \
    || fail "create $machine failed: $(tail -5 "$log.create")"
  smolvm machine start --name "$machine" >"$log.start" 2>&1 \
    || fail "start $machine failed: $(tail -5 "$log.start")"

  count=0
  while [ "$count" -lt 60 ]; do
    smolvm machine status --name "$machine" >/dev/null 2>&1 && break
    count=$((count + 1))
    sleep 1
  done
  [ "$count" -lt 60 ] || fail "$machine did not reach running state"

  printf 'sizing_trial: running the fixture workload on %s\n' "$machine"
  smolvm machine exec --name "$machine" -e HOME=/tmp \
    -e ACTIONS_RUNNER_HOOK_JOB_STARTED= -e ACTIONS_RUNNER_HOOK_JOB_COMPLETED= -- \
    sh /work/smolrunners/tests/fixtures/executor "$scenario" >"$log" 2>&1 \
    || fail "workload failed on $machine: $(tail -10 "$log")"
  grep -q '^phase cpu begin' "$log" || fail "cpu phase missing on $machine"
  grep -q '^phase allocate begin' "$log" || fail "allocate phase missing on $machine"
  grep -q 'executor: status=0' "$log" || fail "executor did not succeed on $machine"

  smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
  smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
}

cat > "$state/scenarios/small.sh" <<'EOF'
cpu 1
allocate 8 1
EOF
cat > "$state/scenarios/balanced.sh" <<'EOF'
cpu 1
allocate 8 1
EOF

run_shape "$machine_a" 2 2048 /work/scenarios/small.sh "$state/small.log"
run_shape "$machine_b" 4 4096 /work/scenarios/balanced.sh "$state/balanced.log"

printf 'sizing_trial: checking for leftover sr-* resources\n'
remaining="$(leftovers)"
[ -z "$remaining" ] || fail "leftover machines: $remaining"

printf 'sizing_trial: ok\n'
