#!/bin/sh
set -eu

repo=/work/smolrunners
runtime=/opt/runtime
state=/work/tmp/cli-e2e-state
machine=sr-cli-e2e
port=8791
base=http://127.0.0.1:$port
export SMOLRUNNERS_API_URL=$base
api=$repo/tests/cli/bin/smolrunners
adapter=$repo/tests/cli/bin/smolrunners-adapter

fail() { printf 'cli_e2e: %s\n' "$*" >&2; exit 1; }

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$state/home" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/home/.config" XDG_CACHE_HOME="$state/home/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    "$runtime/smolvm" --error-format json --state-dir "$state/smolvm" "$@"
}

field() {
  python3 -c '
import json, sys
value = json.load(sys.stdin)
for key in sys.argv[1].split("."):
    if isinstance(value, list):
        value = value[int(key)]
    else:
        value = value[key]
print(value)
' "$1"
}

log_pid=''
server_pid=''
stop_logger() {
  [ -n "$log_pid" ] || return 0
  for child in $(ps -eo pid,ppid 2>/dev/null | awk -v parent="$log_pid" '$2 == parent {print $1}'); do
    kill -9 "$child" 2>/dev/null || true
  done
  kill -9 "$log_pid" 2>/dev/null || true
  log_pid=''
}

cleanup() {
  [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null || true
  stop_logger
  smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
  smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
  for cgroup in /sys/fs/cgroup/sr-*; do
    [ -d "$cgroup" ] && rmdir "$cgroup" 2>/dev/null || true
  done
}
trap cleanup EXIT

[ -x "$api" ] || fail "missing api binary at $api"
[ -x "$adapter" ] || fail "missing adapter binary at $adapter"
[ -x "$runtime/smolvm" ] || fail "missing pinned runtime"
[ -x "$repo/tests/vm/bin/smolrunners-collector" ] || fail 'missing collector fixture binary'

rm -rf "$state"
mkdir -p "$state/tmp" "$state/home/.config" "$state/home/.cache" "$state/api" \
  "$state/rootfs" "$state/cache" "$state/spool"

printf 'cli_e2e: starting API server\n'
"$api" serve --listen "127.0.0.1:$port" --state-dir "$state/api" >"$state/server.log" 2>&1 &
server_pid=$!
attempt=0
ready=0
while [ "$attempt" -lt 60 ]; do
  if curl -fsS "$base/api/v1/status" >"$state/status.json" 2>/dev/null; then
    ready=1
    break
  fi
  attempt=$((attempt + 1))
  sleep 1
done
[ "$ready" -eq 1 ] || fail "server did not become ready: $(cat "$state/server.log")"
[ "$(field data.initialized < "$state/status.json")" = "False" ] || fail 'unexpected initial state'

printf 'cli_e2e: bootstrap and enrollment through the CLI\n'
"$api" init --non-interactive --json >"$state/init.json"
[ "$(field schema_version < "$state/init.json")" = "1" ] || fail 'init schema_version'
[ "$(field scope.kind < "$state/init.json")" != "None" ] || fail 'init has no scope'
"$api" doctor --json >"$state/doctor.json"
[ "$(field schema_version < "$state/doctor.json")" = "1" ] || fail 'doctor schema_version'
"$api" github connect --fake --non-interactive --json >"$state/github.json"
[ "$(field data.fake < "$state/github.json")" = "True" ] || fail 'github connect is not fake-backed'
"$api" host invite --non-interactive --json >"$state/invite.json"
token=$(field data.token < "$state/invite.json")
"$api" host join --token "$token" --non-interactive --json >"$state/join.json"
host_id=$(field data.id < "$state/join.json")
[ "$host_id" -gt 0 ] || fail 'host join returned no id'
"$api" hosts list --json >"$state/hosts-list.json"
[ "$(field data.hosts.0.id < "$state/hosts-list.json")" = "$host_id" ] || fail 'hosts list missing joined host'
"$api" hosts inspect "$host_id" --json >"$state/hosts-inspect.json"
"$api" hosts drain "$host_id" --non-interactive --json >"$state/drain.json"
[ "$(field data.next < "$state/drain.json")" = "drained" ] || fail 'drain did not change admission state'
"$api" hosts resume "$host_id" --non-interactive --json >"$state/resume.json"
[ "$(field data.next < "$state/resume.json")" = "eligible" ] || fail 'resume did not change admission state'

printf 'cli_e2e: profiles, images and caches through the CLI\n'
cat > "$state/profile.toml" <<'EOF'
name = "balanced-4c8g"
vcpu = 4
memory_gib = 8
distro = "debian"
release = "13"
arch = "arm64"
EOF
"$api" profiles apply --file "$state/profile.toml" --non-interactive --json >"$state/profile-apply.json"
revision=$(field data.revision_id < "$state/profile-apply.json")
[ "${#revision}" -eq 64 ] || fail 'profile revision id is not content addressed'
"$api" profiles list --json >"$state/profiles-list.json"
"$api" profiles inspect balanced-4c8g --json >"$state/profiles-inspect.json"
"$api" profiles recommend --json >"$state/recommend.json"
[ "$(field schema_version < "$state/recommend.json")" = "1" ] || fail 'recommend schema_version'
"$api" images build --non-interactive --json >"$state/image-build.json"
image_id=$(field data.id < "$state/image-build.json")
"$api" images qualify "$image_id" --non-interactive --json >"$state/image-qualify.json"
"$api" images promote "$image_id" --non-interactive --json >"$state/image-promote.json"
curl -fsS -X POST -H 'Content-Type: application/json' \
  -d '{"kind":"action-archive","scope":"repo:o/r","generation":2,"lineage":["g1"],"size_bytes":4096,"pinned":false,"active":false}' \
  "$base/api/v1/caches/create" >"$state/cache-create.json"
cache_id=$(field data.id < "$state/cache-create.json")
"$api" caches list --json >"$state/caches-list.json"
"$api" caches inspect "$cache_id" --json >"$state/caches-inspect.json"
"$api" caches prune --dry-run --non-interactive --json >"$state/caches-prune.json"
[ "$(field data.dry_run < "$state/caches-prune.json")" = "True" ] || fail 'prune ignored --dry-run'

printf 'cli_e2e: fakebroker demand into the API\n'
python3 "$repo/tests/cli/fakebroker_demand.py" "$adapter" "$repo/adapter/testdata/scenarios" \
  "$base" balanced-4c8g job-available-to-acquire >"$state/demand.json"
job_id=$(field jobs.0.id < "$state/demand.json")
attempt_id=$(field jobs.0.execution_attempt_id < "$state/demand.json")
[ "$job_id" -gt 0 ] || fail 'fakebroker demand created no job'
curl -fsS -X POST -H 'Content-Type: application/json' -d "{\"url\":\"https://github.com/fixture/fixture/actions/runs/1\"}" \
  "$base/api/v1/jobs/$job_id/github" >/dev/null
"$api" jobs list --json >"$state/jobs-list.json"
[ "$(field data.jobs.0.id < "$state/jobs-list.json")" = "$job_id" ] || fail 'jobs list missing demanded job'
"$api" jobs inspect "$job_id" --json >"$state/jobs-inspect.json"

printf 'cli_e2e: building the fixture image and booting the VM job\n'
sh "$repo/images/debian-fixture/build.sh" "$state/rootfs" "$state/cache"
cp "$repo/tests/vm/bin/smolrunners-collector" "$state/rootfs/opt/smolrunners/bin/smolrunners-collector"
chmod 0755 "$state/rootfs/opt/smolrunners/bin/smolrunners-collector"
cat > "$state/spool/scenario.sh" <<'EOF'
cpu 2
allocate 16 2
io 2 3
EOF
smolvm machine run -d --name "$machine" -I "$state/rootfs/" --net \
  -v "$repo:/work/smolrunners:ro" -v "$state/spool:/spool:rw" \
  -- /opt/smolrunners/bin/smolrunners-collector --interval-ms 60 --batch-lines 18 \
  >"$state/boot.log" 2>&1
attempt=0
while [ "$attempt" -lt 60 ]; do
  smolvm machine status --name "$machine" >/dev/null 2>&1 && break
  attempt=$((attempt + 1))
  sleep 1
done
[ "$attempt" -lt 60 ] || fail 'machine did not reach running state'

smolvm machine logs -f -n "$machine" >"$state/spool/raw.log" 2>&1 &
log_pid=$!
curl -fsS -X POST -H 'Content-Type: application/json' -d "{\"host_id\":$host_id}" \
  "$base/api/v1/jobs/$job_id/start" >"$state/job-start.json"
[ "$(field data.state < "$state/job-start.json")" = "busy" ] || fail 'job did not enter busy state'

smolvm machine exec --name "$machine" -e HOME=/tmp \
  -e ACTIONS_RUNNER_HOOK_JOB_STARTED= -e ACTIONS_RUNNER_HOOK_JOB_COMPLETED= -- \
  sh /work/smolrunners/tests/fixtures/executor /spool/scenario.sh nginx:alpine \
  >"$state/phases.log" 2>&1 || fail "executor failed: $(tail -10 "$state/phases.log")"
grep -q 'executor: status=0' "$state/phases.log" || fail 'fixture executor did not succeed'
grep -q '^phase cpu begin' "$state/phases.log" || fail 'cpu phase missing'
grep -q '^phase allocate begin' "$state/phases.log" || fail 'allocate phase missing'
grep -q '^phase io begin' "$state/phases.log" || fail 'io phase missing'

printf 'cli_e2e: ingest telemetry and finalize the job\n'
sleep 2
smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
stop_logger
sh "$repo/tests/vm/telemetry_spool.sh" "$state/spool/series.ndjson" 20000 <"$state/spool/raw.log"
python3 "$repo/tests/cli/ingest_samples.py" "$base" "$state/spool/series.ndjson" \
  "$attempt_id" "$host_id" >"$state/ingest.json"
ingested=$(field ingested < "$state/ingest.json")
[ "$ingested" -gt 0 ] || fail 'no telemetry samples ingested'
curl -fsS -X POST -H 'Content-Type: application/json' -d '{"result":"success"}' \
  "$base/api/v1/jobs/$job_id/complete" >"$state/job-complete.json"
[ "$(field data.state < "$state/job-complete.json")" = "done" ] || fail 'job did not complete'

printf 'cli_e2e: CLI metrics and CSV report agree with the HTTP API\n'
"$api" jobs metrics --follow --json >"$state/metrics-cli.json"
curl -fsS "$base/api/v1/jobs/$job_id/metrics" >"$state/metrics-api.json"
"$api" jobs report --format csv >"$state/report-cli.csv"
curl -fsS "$base/api/v1/jobs/report" >"$state/report-api.json"
"$api" jobs top --json >"$state/jobs-top.json"
"$api" jobs logs --follow >"$state/logs.txt"
grep -q 'unavailable; see GitHub' "$state/logs.txt" || fail 'logs --follow must defer to GitHub'
"$api" jobs explain "$job_id" --json >"$state/jobs-explain.json"
"$api" jobs compare --a "$job_id" --b "$job_id" --json >"$state/jobs-compare.json"
"$api" jobs debug "$job_id" --json >"$state/jobs-debug.json"

python3 - "$state" <<'PY'
import json
import os
import sys

state = sys.argv[1]


def load(name):
    with open(os.path.join(state, name)) as stream:
        return json.load(stream)


cli = load("metrics-cli.json")
api = load("metrics-api.json")
assert cli["schema_version"] == 1, "metrics schema_version"
assert cli["units"]["cpu_cores"] == "cores", "metrics units"
assert cli["units"]["cpu_percent"] == "percent", "metrics units"
assert cli["data"]["samples"] > 0, "no metrics samples"
assert cli["data"]["samples"] == api["data"]["samples"], "CLI/API sample count differs"
assert cli["data"]["series"] == api["data"]["series"], "CLI/API series differs"
scopes = {row["scope"] for row in cli["data"]["series"]}
assert "guest" in scopes, "guest scope missing from metrics"
units = {row["unit"] for row in cli["data"]["series"]}
assert "cores" in units, "cores unit missing from metrics"

report_rows = load("report-api.json")["data"]["rows"]
with open(os.path.join(state, "report-cli.csv")) as stream:
    csv_text = stream.read()
assert csv_text.startswith("id,name,"), "CSV header"
for row in report_rows:
    assert str(row["id"]) in csv_text, "CSV missing job %s" % row["id"]
    assert str(row["name"]) in csv_text, "CSV missing name"

explain = load("jobs-explain.json")
assert explain["data"]["profile"] == "balanced-4c8g", "explain profile"
debug = load("jobs-debug.json")
assert debug["data"]["vm_scoped"] is True, "debug must be VM scoped"
assert debug["scope"]["kind"] == "vm", "debug scope"
print("cli_e2e: facts agree across CLI and HTTP API")
PY

leftovers="$(smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}')"
[ -z "$leftovers" ] || fail "leftover machines: $leftovers"
cgroups="$(ls /sys/fs/cgroup 2>/dev/null | awk '/^sr-/ {print}')"
[ -z "$cgroups" ] || fail "leftover cgroups: $cgroups"

printf 'cli_e2e: ok\n'
