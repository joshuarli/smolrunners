#!/bin/sh
set -eu

repo=/work/smolrunners
runtime=/opt/runtime
work=/work/tmp/connected-job
coord_state=$work/coordinator
host_state=$work/host
scenarios=$work/scenarios
image_cache=/work/tmp/connected-image
bin=$repo/tests/vm/bin
agent=$bin/smolrunners
api=http://127.0.0.1:8787
profile=smol-debian13-arm64-2c2g

fail() {
  printf 'connected_job: %s\n' "$*" >&2
  [ -f "$coord_state/coordinator.log" ] && tail -40 "$coord_state/coordinator.log" >&2
  exit 1
}
say() { printf 'connected_job: %s\n' "$*"; }

coord_pid=''
follow_pid=''
cleanup() {
  [ -n "$follow_pid" ] && kill "$follow_pid" 2>/dev/null || true
  [ -n "$coord_pid" ] && kill -9 "$coord_pid" 2>/dev/null || true
  pkill -9 -f smolrunners-adapter 2>/dev/null || true
  [ -d "$host_state" ] && "$agent" host --state-dir "$host_state" --runtime-dir "$runtime" \
    --launcher "$bin/smolrunners-launcher" list --json 2>/dev/null |
    python3 -c 'import json,sys; [print(m["machine"]) for m in json.load(sys.stdin)]' 2>/dev/null |
    while read -r machine; do
      "$agent" host --state-dir "$host_state" --runtime-dir "$runtime" \
        --launcher "$bin/smolrunners-launcher" stop --name "$machine" 2>/dev/null || true
    done
  sleep 2
  pkill -9 -f "provision --name x" 2>/dev/null || true
  "$agent" host --state-dir "$host_state" --runtime-dir "$runtime" \
    --launcher "$bin/smolrunners-launcher" reap >/dev/null 2>&1 || true
}
trap cleanup EXIT

json() { python3 -c "import json,sys; data=json.load(sys.stdin); print($1)"; }
get() { curl -fsS "$api$1"; }

job_field() {
  get "/api/v1/jobs/$1" | json "data['data']$2" 2>/dev/null || true
}

wait_job() {
  id=$1 expected=$2 limit=$3
  count=0
  while [ "$count" -lt "$limit" ]; do
    state=$(job_field "$id" "['state']")
    case " $expected " in *" $state "*) return 0 ;; esac
    count=$((count + 1))
    sleep 1
  done
  fail "job $id stayed '$state', expected one of: $expected"
}

samples() { get "/api/v1/jobs/$1/metrics" | json "data['data']['samples']" 2>/dev/null || echo 0; }

newest_job() {
  count=0
  while [ "$count" -lt 120 ]; do
    id=$(get /api/v1/jobs | python3 -c '
import json, sys
jobs = [job for job in json.load(sys.stdin)["data"]["jobs"] if job.get("runner_name")]
known = set(sys.argv[1].split(",")) - {""}
fresh = [job["id"] for job in jobs if str(job["id"]) not in known]
print(fresh[-1] if fresh else "")
' "$1" 2>/dev/null || true)
    [ -n "$id" ] && { printf '%s\n' "$id"; return 0; }
    count=$((count + 1))
    sleep 1
  done
  fail 'no execution appeared through the API'
}

write_pilot() {
  scenario=$1 entrypoint=$2
  cat > "$work/pilot.toml" <<EOF
state_dir = "$coord_state"
listen = "127.0.0.1:8787"
max_concurrency = 1
idle_timeout_secs = 900
completion_grace_secs = 60

[adapter]
mode = "fake"
program = "$repo/tests/cli/bin/smolrunners-adapter"
scenario = "$scenario"
scenarios_dir = "$scenarios"

[host]
key = "sr-nested"
arch = "arm64"
vcpu = 6
memory_gib = 12
disk_gib = 40
agent = "$agent"
launcher = "$bin/smolrunners-launcher"
runtime_dir = "$runtime"
state_dir = "$host_state"

[profile]
name = "$profile"
labels = ["$profile"]
vcpu = 2
memory_gib = 2
storage_gib = 8
overlay_gib = 4
image = "$image_cache/rootfs/"
entrypoint = "$entrypoint"

[vmm_overhead]
memory_mib = 512
vcpu_milli = 250

[telemetry]
collector = "/opt/smolrunners/bin/smolrunners-collector"
interval_ms = 500
EOF
}

start_coordinator() {
  write_pilot "$1" "$2"
  setsid "$agent" serve --pilot "$work/pilot.toml" >>"$work/coord.out" 2>&1 &
  coord_pid=$!
  count=0
  until get /api/v1/jobs >/dev/null 2>&1; do
    count=$((count + 1))
    [ "$count" -lt 60 ] || fail 'coordinator API did not come up'
    kill -0 "$coord_pid" 2>/dev/null || fail "coordinator exited: $(tail -5 "$work/coord.out")"
    sleep 0.5
  done
}

stop_coordinator() {
  [ -n "$coord_pid" ] && kill -9 "$coord_pid" 2>/dev/null || true
  wait "$coord_pid" 2>/dev/null || true
  coord_pid=''
  pkill -9 -f smolrunners-adapter 2>/dev/null || true
  count=0
  while curl -fsS "$api/api/v1/jobs" >/dev/null 2>&1 && [ "$count" -lt 20 ]; do
    count=$((count + 1)); sleep 0.5
  done
}

assert_released() {
  id=$1
  machine=$(job_field "$id" "['machine']")
  [ -n "$machine" ] || fail "job $id never recorded a machine"
  markers=$(ls -A "$host_state/markers" 2>/dev/null | grep -v '^\.' || true)
  [ -z "$markers" ] || fail "host markers outlived job $id: $markers"
  [ ! -d "$host_state/vms/$machine" ] || fail "VM state for $machine outlived job $id"
  [ ! -d "/sys/fs/cgroup/$machine" ] || fail "cgroup for $machine outlived job $id"
  admissions=$(ls "$host_state/storage" 2>/dev/null | grep admission || true)
  [ -z "$admissions" ] || fail "storage admissions outlived job $id: $admissions"
  claims=$(ls -A "$host_state/uids" 2>/dev/null || true)
  [ -z "$claims" ] || fail "uid claims outlived job $id: $claims"
  grep -q "RELEASE job=$id " "$coord_state/coordinator.log" || fail "job $id did not release its reservation"
  grep -q "DEREGISTER job=$id " "$coord_state/coordinator.log" || fail "job $id did not deregister its runner"
  [ -z "$(ls -A "$coord_state/jit")" ] || fail "a JIT config file outlived job $id"
  [ "$(samples "$id")" -gt 0 ] || [ "${2:-}" = no-samples ] || fail "job $id has no samples through the API"
}

scenario() {
  name=$1
  shift
  saved_ifs=$IFS
  IFS=,
  steps="$*"
  IFS=$saved_ifs
  printf '{"name":"%s","session":{"session_id":"11111111-2222-3333-4444-%012d","owner_name":"acme","statistics":{"total_assigned_jobs":0}},"acquire_results":[[1]],"steps":[%s]}\n' \
    "$name" "$(printf '%s' "$name" | cksum | cut -c1-6)" "$steps" > "$scenarios/$name.json"
}

job_json() { printf '{"request_id":%s,"job_id":"%s","labels":["%s"],"workflow_run_id":9901,"owner_name":"acme","repository_name":"repo","runner_name":"%s"}' "$1" "$2" "$profile" "$3"; }

build_image() {
  key=$(cat "$repo/images/debian13/build.sh" "$repo/images/debian13/run-job" "$repo/images/debian13/manifest.json" \
    "$bin/smolrunners-collector" | sha256sum | cut -c1-16)
  if [ "$(cat "$image_cache/key" 2>/dev/null)" = "$key" ]; then
    say "reusing the debian13 rootfs built from this recipe ($key)"
    install_stand_ins
    return 0
  fi
  say 'building the debian13 rootfs from the current recipe'
  rm -rf "$image_cache"
  mkdir -p "$image_cache/rootfs" "$image_cache/cache"
  ARCH=aarch64 SMOLRUNNERS_COLLECTOR="$bin/smolrunners-collector" \
    sh "$repo/images/debian13/build.sh" "$image_cache/rootfs" "$image_cache/cache" >"$image_cache/build.log" 2>&1 ||
    fail "image build failed: $(tail -20 "$image_cache/build.log")"
  install_stand_ins
  printf '%s\n' "$key" > "$image_cache/key"
}

install_stand_ins() {
  stand_in succeed 0
  stand_in fail 1
  cat > "$image_cache/rootfs/opt/smolrunners/bin/fixture-hang" <<'EOF'
#!/bin/sh
set -eu
rm -f -- "$1"
exec sleep 3600
EOF
  chmod 0755 "$image_cache/rootfs/opt/smolrunners/bin/fixture-hang"
}

stand_in() {
  cat > "$image_cache/rootfs/opt/smolrunners/bin/fixture-$1" <<EOF
#!/bin/sh
set -eu
jit=\$(cat -- "\$1")
rm -- "\$1"
printf '%s' "\$jit" | base64 -d > /run/jit.json
grep -q '".runner"' /run/jit.json || { echo 'stand-in: JIT config lacks .runner' >&2; exit 90; }
rm -f /run/jit.json
echo 'stand-in: cpu phase'
end=\$((\$(date +%s) + 60))
for worker in 1 2; do ( while [ "\$(date +%s)" -lt "\$end" ]; do :; done ) & done
wait
echo 'stand-in: memory phase'
dd if=/dev/zero of=/dev/shm/fill bs=1M count=256 status=none
sleep 12
rm -f /dev/shm/fill
exit $2
EOF
  chmod 0755 "$image_cache/rootfs/opt/smolrunners/bin/fixture-$1"
}

for tool in curl python3 setsid base64; do command -v "$tool" >/dev/null || fail "missing $tool"; done
[ -x "$agent" ] || fail "missing $agent"
rm -rf "$work"
mkdir -p "$work" "$scenarios"
for stale in /sys/fs/cgroup/sr-*; do [ -d "$stale" ] && rmdir "$stale" 2>/dev/null || true; done
build_image

say 'A: a successful job observed only through the public API, with a coordinator restart'
scenario connected-a \
  "{\"kind\":\"message\",\"message_id\":100,\"statistics\":{\"total_assigned_jobs\":1,\"total_available_jobs\":1},\"job_available\":[$(job_json 101 job-a '')]}" \
  "{\"kind\":\"message\",\"message_id\":101,\"after_ms\":30000,\"job_started\":[$(job_json 101 job-a '$jit')]}" \
  "{\"kind\":\"message\",\"message_id\":102,\"after_ms\":900000}"
start_coordinator connected-a /opt/smolrunners/bin/fixture-succeed
job_a=$(newest_job '')
say "execution $job_a appeared through /api/v1/jobs"
wait_job "$job_a" "idle busy" 300
count=0
while [ "$(samples "$job_a")" -eq 0 ] && [ "$count" -lt 60 ]; do count=$((count + 1)); sleep 1; done
runner_a=$(job_field "$job_a" "['runner_name']")
machine_a=$(job_field "$job_a" "['machine']")
cpu_max=$(job_field "$job_a" "['limits']['host_cpu_max']")
memory_max=$(job_field "$job_a" "['limits']['host_memory_max_bytes']")
[ "$(job_field "$job_a" "['limits']['guest_vcpu']")" = 2 ] || fail 'API does not report the profile guest vCPU'
[ "$cpu_max" = '225000 100000' ] || fail "API cpu limit is '$cpu_max'"
[ "$memory_max" = $(((2048 + 512) * 1024 * 1024)) ] || fail "API memory limit is '$memory_max'"
[ "$(cat "/sys/fs/cgroup/$machine_a/cpu.max")" = "$cpu_max" ] || fail "host cpu.max $(cat "/sys/fs/cgroup/$machine_a/cpu.max") differs from the profile"
[ "$(cat "/sys/fs/cgroup/$machine_a/memory.max")" = "$memory_max" ] || fail 'host memory.max differs from the profile'
say "profile limits agree: API and $machine_a cgroup both cpu.max='$cpu_max' memory.max=$memory_max"

SMOLRUNNERS_API_URL=$api "$agent" jobs metrics "$job_a" --follow >"$work/follow.out" 2>&1 &
follow_pid=$!
first=$(samples "$job_a")
sleep 4
second=$(samples "$job_a")
[ "$second" -gt "$first" ] || fail "API samples did not change while busy ($first -> $second)"
case "$(job_field "$job_a" "['state']")" in idle|busy) ;; *) fail 'job finished before the live checks' ;; esac
lines=$(wc -l < "$work/follow.out")
[ "$lines" -ge 2 ] || fail "CLI follow emitted $lines lines before completion: $(cat "$work/follow.out")"
say "live: API samples $first -> $second while running; CLI follow already streamed $lines lines"

say 'A: killing the coordinator mid-job'
stop_coordinator
"$agent" host --state-dir "$host_state" --runtime-dir "$runtime" --launcher "$bin/smolrunners-launcher" list --json |
  python3 -c "import json,sys; m=[e for e in json.load(sys.stdin) if e['machine']=='$machine_a']; assert m and m[0]['owner_alive'], m" ||
  fail "the VM $machine_a lost its host agent with the coordinator"
scenario connected-a-resume \
  "{\"kind\":\"message\",\"message_id\":110,\"statistics\":{\"total_assigned_jobs\":1}}" \
  "{\"kind\":\"message\",\"message_id\":111,\"after_ms\":40000,\"statistics\":{\"total_assigned_jobs\":0},\"job_completed\":[$(job_json 101 job-a "$runner_a" | sed 's/}$/,"result":"succeeded"}/')]}" \
  "{\"kind\":\"message\",\"message_id\":112,\"after_ms\":900000}"
start_coordinator connected-a-resume /opt/smolrunners/bin/fixture-succeed
grep -q "REATTACH job=$job_a " "$coord_state/coordinator.log" || fail 'restarted coordinator did not reattach'
before=$(samples "$job_a")
count=0
while [ "$(samples "$job_a")" -le "$before" ] && [ "$count" -lt 30 ]; do count=$((count + 1)); sleep 1; done
[ "$(samples "$job_a")" -gt "$before" ] || fail 'telemetry did not resume through the API after restart'
say "restart: same execution $job_a keeps sampling ($before -> $(samples "$job_a"))"
wait_job "$job_a" done 180
count=0
while [ "$(job_field "$job_a" "['result']")" != succeeded ] && [ "$count" -lt 90 ]; do count=$((count + 1)); sleep 1; done
[ "$(job_field "$job_a" "['result']")" = succeeded ] || fail "job a result: $(job_field "$job_a" "['result']")"
[ "$(job_field "$job_a" "['state']")" = done ] || fail "job a state after GitHub's result: $(job_field "$job_a" "['state']")"
[ "$(job_field "$job_a" "['protocol_job_id']")" = job-a ] || fail 'job_started did not join GitHub job-a to the execution'
[ "$(get "/api/v1/jobs" | json "len(data['data']['jobs'])")" = 1 ] || fail 'the restart created a second execution'
wait "$follow_pid" 2>/dev/null || true
follow_pid=''
tail -1 "$work/follow.out" | grep -q 'state=done' || fail "CLI follow did not end on the terminal state: $(tail -2 "$work/follow.out")"
assert_released "$job_a"
say 'A: done, cleaned up, capacity released and runner deregistered'
stop_coordinator

say 'B: a job that fails'
scenario connected-b \
  "{\"kind\":\"message\",\"message_id\":200,\"statistics\":{\"total_assigned_jobs\":1}}" \
  "{\"kind\":\"message\",\"message_id\":201,\"after_ms\":90000,\"job_started\":[$(job_json 201 job-b '$jit')]}" \
  "{\"kind\":\"message\",\"message_id\":202,\"after_ms\":120000,\"statistics\":{\"total_assigned_jobs\":0},\"job_completed\":[$(job_json 201 job-b '$jit' | sed 's/}$/,"result":"failed"}/')]}" \
  "{\"kind\":\"message\",\"message_id\":203,\"after_ms\":900000}"
start_coordinator connected-b /opt/smolrunners/bin/fixture-fail
job_b=$(newest_job "$job_a")
wait_job "$job_b" failed 240
[ "$(job_field "$job_b" "['runner_exit']")" = 'Code(1)' ] || fail "job b runner exit: $(job_field "$job_b" "['runner_exit']")"
assert_released "$job_b"
say "B: failed with runner exit 1 and released everything"
stop_coordinator

say 'C: operator cancellation'
scenario connected-c \
  "{\"kind\":\"message\",\"message_id\":300,\"statistics\":{\"total_assigned_jobs\":1}}" \
  "{\"kind\":\"message\",\"message_id\":301,\"after_ms\":90000,\"job_started\":[$(job_json 301 job-c '$jit')]}" \
  "{\"kind\":\"message\",\"message_id\":302,\"after_ms\":20000,\"statistics\":{\"total_assigned_jobs\":0}}" \
  "{\"kind\":\"message\",\"message_id\":303,\"after_ms\":900000}"
start_coordinator connected-c /opt/smolrunners/bin/fixture-hang
job_c=$(newest_job "$job_a,$job_b")
wait_job "$job_c" busy 300
SMOLRUNNERS_API_URL=$api "$agent" jobs cancel "$job_c" >/dev/null || fail 'jobs cancel failed'
wait_job "$job_c" cancelled 120
assert_released "$job_c"
say 'C: cancelled through the CLI, torn down and released'
stop_coordinator

say 'D: the official runner receives the JIT config through run-job'
scenario connected-d \
  "{\"kind\":\"message\",\"message_id\":400,\"statistics\":{\"total_assigned_jobs\":1}}" \
  "{\"kind\":\"message\",\"message_id\":401,\"after_ms\":150000,\"statistics\":{\"total_assigned_jobs\":0}}" \
  "{\"kind\":\"message\",\"message_id\":402,\"after_ms\":900000}"
start_coordinator connected-d /opt/smolrunners/bin/run-job
job_d=$(newest_job "$job_a,$job_b,$job_c")
wait_job "$job_d" "idle failed" 240
machine_d=$(job_field "$job_d" "['machine']")
runner_log=$host_state/logs/$machine_d.runner.log
count=0
while [ "$count" -lt 150 ]; do
  if grep -q '^run-job: starting the official runner' "$runner_log" 2>/dev/null &&
     sed -n '/^run-job: starting the official runner/,$p' "$runner_log" | grep -v '^run-job:' | grep -q .; then
    break
  fi
  grep -q '^run-job: dockerd did not' "$runner_log" 2>/dev/null && break
  count=$((count + 1)); sleep 1
done
grep -v '^time=' "$runner_log" 2>/dev/null | tail -12 | sed 's/^/connected_job:   runner| /'
grep -q '^run-job: starting the official runner' "$runner_log" ||
  fail 'run-job did not reach the official runner'
sed -n '/^run-job: starting the official runner/,$p' "$runner_log" | grep -v '^run-job:' | grep -q . ||
  fail 'the official runner printed nothing after receiving the JIT config'
if [ "$(job_field "$job_d" "['state']")" != failed ]; then
  SMOLRUNNERS_API_URL=$api "$agent" jobs cancel "$job_d" >/dev/null || true
fi
wait_job "$job_d" "failed cancelled" 120
assert_released "$job_d"
say "D: official runner started with the fake JIT config, ended $(job_field "$job_d" "['state']"), released"
stop_coordinator

say ok
