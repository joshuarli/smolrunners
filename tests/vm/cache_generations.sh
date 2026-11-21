#!/bin/sh
set -eu

runtime=/opt/runtime
repo=/work/smolrunners
fixture=$repo/crates/smolrunners/src/host/cache/smolrunners-cache
state=/work/tmp/cache-state
image='alpine@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf'

fail() { printf 'cache_generations: %s\n' "$*" >&2; exit 1; }

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$state/home" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/home/.config" XDG_CACHE_HOME="$state/home/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" --error-format json --state-dir "$state/smolvm" "$@"
}

cache() { "$fixture" --state "$state" "$@"; }

jassert() {
  document=$1
  expression=$2
  printf '%s' "$document" | python3 -c '
import json, sys
doc = json.load(sys.stdin)
assert eval(sys.argv[1]), (sys.argv[1], doc)
' "$expression" || fail "assertion failed: $expression for $document"
}

expect_failure() {
  label=$1
  shift
  set +e
  output=$("$@" 2>&1)
  code=$?
  set -e
  [ "$code" -ne 0 ] || fail "$label unexpectedly succeeded: $output"
  printf '%s' "$output"
}

leftovers() {
  [ -d "$state/smolvm" ] || return 0
  smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}'
}

cleanup() {
  for machine in $(leftovers); do
    smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
    smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
  done
  for cgroup in /sys/fs/cgroup/sr-*; do
    [ -d "$cgroup" ] && rmdir "$cgroup" 2>/dev/null || true
  done
}
trap cleanup EXIT

assert_clean() {
  remaining="$(leftovers)"
  [ -z "$remaining" ] || fail "leftover machines: $remaining"
  cgroups="$(ls /sys/fs/cgroup 2>/dev/null | awk '/^sr-/ {print}')"
  [ -z "$cgroups" ] || fail "leftover cgroups: $cgroups"
}

wait_for_machine() {
  count=0
  while [ "$count" -lt 60 ]; do
    if smolvm machine exec --name "$1" --timeout 10sec -- true >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
    count=$((count + 1))
  done
  fail "timeout waiting for machine $1"
}

wait_for_gone() {
  count=0
  while [ "$count" -lt 60 ]; do
    smolvm machine status --name "$1" >/dev/null 2>&1 || return 0
    sleep 1
    count=$((count + 1))
  done
  fail "machine $1 still present"
}

run_job() {
  job=$1
  tag=$2
  generation=$3
  cache acquire --job "$job" --gen "$generation" --repo acme/widgets --tier maintainer \
    >"$state/$job.acquire.json"
  cache --job "$job" write --path bin/gcc --data "$tag" >"$state/$job.write.json"
  cache --job "$job" read --path bin/gcc >"$state/$job.read.json"
  cache --job "$job" release >"$state/$job.release.json"
}

printf 'cache_generations: initialize the host-owned store\n'
[ -x "$fixture" ] || fail "missing cache fixture at $fixture"
[ -x "$runtime/smolvm" ] || fail "missing pinned runtime at $runtime/smolvm"
rm -rf "$state"
mkdir -p "$state/home" "$state/tmp" "$state/home/.config" "$state/home/.cache"
for stale in /sys/fs/cgroup/sr-*; do
  [ -d "$stale" ] && rmdir "$stale" 2>/dev/null || true
done
cache init >"$state/init.json"
jassert "$(cat "$state/init.json")" 'doc["initialized"] is True and doc["schema_version"] == 1'

printf 'cache_generations: toolchain baseline with a private view\n'
cache seed --name toolchain --path bin/gcc --data gcc-14 >/dev/null
cache seed --name toolchain --path bin/ld --data ld-2 >/dev/null
cache quota --generations 16 --bytes 100000000 >"$state/quota.json"
jassert "$(cat "$state/quota.json")" 'doc["max_generations"] == 16'
baseline=$(cache baseline --name toolchain --repo acme/widgets --tier maintainer)
generation=$(printf '%s' "$baseline" | python3 -c 'import json,sys; print(json.load(sys.stdin)["generation"])')
[ -n "$generation" ] || fail "baseline produced no generation: $baseline"
cache publish --gen "$generation" >"$state/publish-baseline.json"
jassert "$(cat "$state/publish-baseline.json")" 'doc["prior"] is None'
current=$(cache current --repo acme/widgets)
jassert "$current" 'doc["generation"] == "'"$generation"'"'

printf 'cache_generations: concurrent jobs never share mutable state\n'
run_job job-a A "$generation" &
pid_a=$!
run_job job-b B "$generation" &
pid_b=$!
wait "$pid_a" || fail "job-a failed"
wait "$pid_b" || fail "job-b failed"
jassert "$(cat "$state/job-a.read.json")" 'doc["job"] == "job-a" and doc["data"] == "A"'
jassert "$(cat "$state/job-b.read.json")" 'doc["job"] == "job-b" and doc["data"] == "B"'
jassert "$(cache --job job-a read --path bin/gcc)" 'doc["data"] == "A"'
jassert "$(cache --job job-b read --path bin/gcc)" 'doc["data"] == "B"'
jassert "$(cache --job job-a shared --path bin/gcc)" 'doc["shared"] is False'
jassert "$(cache --job job-b shared --path bin/gcc)" 'doc["shared"] is False'
cache acquire --job fresh --gen "$generation" --repo acme/widgets --tier maintainer >/dev/null
jassert "$(cache --job fresh read --path bin/gcc)" 'doc["data"] == "gcc-14"'
cache --job fresh release >/dev/null

printf 'cache_generations: a job VM reads the baseline through its own view\n'
chmod -R a+rwX "$state"
smolvm machine create --name sr-cache-guest -I "$image" --cpus 1 --mem 512 \
  --storage 2 --overlay 1 -v "$state:/work/cache:rw" >/dev/null
smolvm machine start --name sr-cache-guest >/dev/null
wait_for_machine sr-cache-guest
smolvm machine cp "$fixture" sr-cache-guest:/tmp/smolrunners-cache >/dev/null
smolvm machine exec --name sr-cache-guest --timeout 30sec -- chmod 0755 /tmp/smolrunners-cache
guest_output=$(smolvm machine exec --name sr-cache-guest --timeout 120sec -- sh -eu -c "
  bin=/tmp/smolrunners-cache
  \"\$bin\" --state /work/cache --repo acme/widgets --tier maintainer \
    acquire --job guest-job --gen '$generation' >/dev/null
  \"\$bin\" --state /work/cache --job guest-job \
    write --path bin/gcc --data GUEST >/dev/null
  \"\$bin\" --state /work/cache --job guest-job \
    read --path bin/gcc
  \"\$bin\" --state /work/cache --job guest-job release >/dev/null
")
jassert "$guest_output" 'doc["job"] == "guest-job" and doc["data"] == "GUEST"'
smolvm machine stop --name sr-cache-guest >/dev/null 2>&1 || true
smolvm machine delete --name sr-cache-guest -f >/dev/null 2>&1 || true
wait_for_gone sr-cache-guest
assert_clean

printf 'cache_generations: publication waits for quiescence\n'
cache seed --name next --path bin/gcc --data gcc-15 >/dev/null
next=$(cache baseline --name next --repo acme/widgets --tier maintainer)
next_generation=$(printf '%s' "$next" | python3 -c 'import json,sys; print(json.load(sys.stdin)["generation"])')
jassert "$next" 'doc["parent"] == "'"$generation"'"'
cache acquire --job holder --gen "$next_generation" --repo acme/widgets --tier maintainer >"$state/holder.json"
jassert "$(cat "$state/holder.json")" 'doc["granted"] is True and doc["active_writers"] == 1'
busy=$(expect_failure "publish with an active writer" cache publish --gen "$next_generation")
printf '%s\n' "$busy" | grep -q 'active writers' || fail "busy publish has no reason: $busy"
cache --job holder release >/dev/null
cache publish --gen "$next_generation" >"$state/publish-next.json"
jassert "$(cat "$state/publish-next.json")" 'doc["prior"] == "'"$generation"'"'
current=$(cache current --repo acme/widgets)
jassert "$current" 'doc["generation"] == "'"$next_generation"'"'

printf 'cache_generations: an interrupted publish leaves the prior generation valid\n'
cache seed --name staged --path bin/gcc --data gcc-16 >/dev/null
staged=$(cache baseline --name staged --repo acme/widgets --tier maintainer)
staged_generation=$(printf '%s' "$staged" | python3 -c 'import json,sys; print(json.load(sys.stdin)["generation"])')
cache interrupt --gen "$staged_generation" >"$state/interrupt.json"
jassert "$(cat "$state/interrupt.json")" 'doc["prior"] == "'"$next_generation"'"'
current=$(cache current --repo acme/widgets)
jassert "$current" 'doc["generation"] == "'"$next_generation"'"'
jassert "$(cache inspect --gen "$next_generation")" 'doc["state"] == "published"'
cache acquire --job after-interrupt --gen "$next_generation" --repo acme/widgets --tier maintainer >/dev/null
jassert "$(cache --job after-interrupt read --path bin/gcc)" 'doc["data"] == "gcc-15"'
cache --job after-interrupt release >/dev/null
recovered=$(cache recover --repo acme/widgets)
jassert "$recovered" 'doc["current"] == "'"$next_generation"'"'

printf 'cache_generations: active generations survive collection\n'
cache pin --gen "$next_generation" >"$state/pin.json"
jassert "$(cat "$state/pin.json")" 'doc["pins"] == 1'
cache acquire --job active --gen "$staged_generation" --repo acme/widgets --tier maintainer >/dev/null
cache seed --name doomed --path bin/gcc --data gcc-17 >/dev/null
doomed=$(cache baseline --name doomed --repo acme/widgets --tier maintainer)
doomed_generation=$(printf '%s' "$doomed" | python3 -c 'import json,sys; print(json.load(sys.stdin)["generation"])')
cache terminate --gen "$doomed_generation" >/dev/null
collected=$(cache gc)
jassert "$collected" 'doc["removed"] == ["'"$doomed_generation"'"]'
cache inspect --gen "$next_generation" >/dev/null
cache inspect --gen "$staged_generation" >/dev/null
if cache inspect --gen "$doomed_generation" >/dev/null 2>&1; then
  fail "collected generation $doomed_generation is still readable"
fi

printf 'cache_generations: repositories and trust tiers cannot broaden authority\n'
foreign=$(cache acquire --job cross-repo --gen "$generation" --repo other/repo --tier maintainer)
jassert "$foreign" 'doc["granted"] is False and "repository" in doc["fallback_reason"]'
lowtier=$(cache acquire --job low-tier --gen "$generation" --repo acme/widgets --tier contributor)
jassert "$lowtier" 'doc["granted"] is False and "broaden" in doc["fallback_reason"]'
narrowed=$(cache acquire --job narrowed --gen "$generation" --repo acme/widgets --tier maintainer)
jassert "$narrowed" 'doc["granted"] is True'
cache --job narrowed release >/dev/null
cache --job active release >/dev/null
cache unpin --gen "$next_generation" >/dev/null

printf 'cache_generations: quotas refuse an over-budget generation with a reason\n'
generation_count=$(python3 - "$state" <<'PY'
import os
import sys
root = os.path.join(sys.argv[1], "content", "generations")
print(len([name for name in os.listdir(root) if name.endswith(".json")]))
PY
)
cache quota --generations "$generation_count" --bytes 100000000 >/dev/null
over=$(expect_failure "generation quota" cache baseline --name overquota --repo acme/widgets --tier maintainer)
printf '%s\n' "$over" | grep -q 'quota' || fail "quota refusal has no reason: $over"
cache quota --generations 16 --bytes 8 >/dev/null
cache seed --name oversized --path bin/gcc --data 'a generation larger than the byte quota' >/dev/null
over_bytes=$(expect_failure "byte quota" cache baseline --name oversized --repo acme/widgets --tier maintainer)
printf '%s\n' "$over_bytes" | grep -q 'exceed limit' || fail "byte quota refusal has no reason: $over_bytes"
cache quota --generations 16 --bytes 100000000 >/dev/null

printf 'cache_generations: action archives stay with the runner mechanism\n'
action=$(cache action-cache --job action-job --repo acme/widgets --tier maintainer)
jassert "$action" 'doc["managed_by"] == "runner" and doc["mechanism"] == "actions/cache"'

printf 'cache_generations: outcome fields appear in job history\n'
cache history >"$state/history.json"
python3 - "$state/history.json" "$state/store.sqlite" <<'PY'
import json
import sqlite3
import sys

history = json.load(open(sys.argv[1]))
assert len(history) >= 8, len(history)
by_job = {row["job"]: row for row in history}
assert by_job["job-a"]["payload"]["hit"] is True
assert by_job["job-a"]["payload"]["generation"]
assert by_job["job-a"]["payload"]["private_view"]
assert by_job["job-a"]["payload"]["repository"] == "acme/widgets"
assert by_job["job-a"]["payload"]["trust_tier"] == "maintainer"
assert by_job["cross-repo"]["payload"]["fallback"]["visible"] is True
assert "repository" in by_job["cross-repo"]["payload"]["fallback"]["reason"]
assert by_job["low-tier"]["payload"]["fallback"]["visible"] is True
assert "broaden" in by_job["low-tier"]["payload"]["fallback"]["reason"]
assert by_job["action-job"]["payload"]["fallback"]["reason"].startswith("action archives")

db = sqlite3.connect(sys.argv[2])
rows = db.execute(
    """SELECT j.github_job_key, wr.repository, e.payload_json, e.occurred_at
       FROM event e
       JOIN execution_attempt ea ON ea.id = e.execution_attempt_id
       JOIN job j ON j.id = ea.job_id
       JOIN workflow_run_attempt wra ON wra.id = j.workflow_run_attempt_id
       JOIN workflow_run wr ON wr.id = wra.workflow_run_id
       WHERE e.kind = 'cache-outcome'"""
).fetchall()
assert len(rows) >= 8, len(rows)
keys = {row[0] for row in rows}
assert {"job-a", "job-b", "cross-repo", "low-tier", "action-job"}.issubset(keys)
for _, repository, payload, occurred in rows:
    decoded = json.loads(payload)
    assert decoded["schema_version"] == 1
    assert decoded["repository"] == repository
    assert occurred
print("cache_generations: job history carries %d cache outcomes" % len(rows))
PY

printf 'cache_generations: clean teardown\n'
wait_for_gone sr-cache-guest
assert_clean
printf 'cache_generations: ok\n'
