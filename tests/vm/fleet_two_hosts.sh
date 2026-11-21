#!/bin/sh
set -eu

repo=/work/smolrunners
runtime=/opt/runtime
bin=$repo/tests/cli/bin/smolrunners
state=/work/tmp/fleet-two-hosts
network=sr-net
port=8793
image='alpine@sha256:f59abe656b692d901e184be3445e084786602108599d2a51d229c0f5a48c71bf'
coord=sr-coord
host_a=sr-host-a
host_b=sr-host-b
local_base=http://127.0.0.1:$port/api/v1

fail() { printf 'fleet_two_hosts: %s\n' "$*" >&2; exit 1; }

smolvm() {
  env -i PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    HOME="$state/home" TMPDIR="$state/tmp" \
    XDG_CONFIG_HOME="$state/home/.config" XDG_CACHE_HOME="$state/home/.cache" \
    SMOLVM_LIB_DIR="$runtime/lib" SMOLVM_AGENT_ROOTFS="$runtime/agent-rootfs" \
    SMOLVM_VM_UID_DROP=off \
    "$runtime/smolvm" --error-format json --state-dir "$state/smolvm" "$@"
}

field() {
  python3 -c '
import json, sys
value = json.load(sys.stdin)
for key in sys.argv[1].split("."):
    value = value[int(key)] if isinstance(value, list) else value[key]
print(value)
' "$1"
}

status_of() { printf '%s\n' "$1" | tail -1; }
body_of() { printf '%s\n' "$1" | sed '$d'; }

call() {
  method=$1
  path=$2
  body=${3:-}
  if [ -n "$body" ]; then
    smolvm machine exec --name "$coord" -- curl -sS -X "$method" \
      -H 'Content-Type: application/json' -d "$body" -w '\n%{http_code}' \
      "$local_base$path"
  else
    smolvm machine exec --name "$coord" -- curl -sS -X "$method" \
      -w '\n%{http_code}' "$local_base$path"
  fi
}

expect_status() {
  out=$1
  want=$2
  label=$3
  got=$(status_of "$out")
  [ "$got" = "$want" ] || fail "$label: status $got, expected $want: $(body_of "$out")"
}

demand() {
  protocol=$1
  request=$2
  profile=$3
  out=$(call POST /demand "{\"protocol_job_id\":\"$protocol\",\"request_id\":$request,\"job_key\":\"build\",\"profile\":\"$profile\"}")
  expect_status "$out" 200 "demand $profile"
  body_of "$out" | field data.id
}

start_job() {
  id=$1
  body=${2:-'{}'}
  call POST "/jobs/$id/start" "$body"
}

complete_job() {
  id=$1
  out=$(call POST "/jobs/$id/complete" '{"result":"success"}')
  expect_status "$out" 200 "complete job $id"
}

hostctl() {
  smolvm machine exec --name "$coord" -- \
    env "SMOLRUNNERS_API_URL=http://127.0.0.1:$port" "$bin" "$@"
}

inspect_host() {
  hostctl hosts inspect "$1" --json
}

stop_all() {
  for machine in $(smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}'); do
    smolvm machine stop --name "$machine" >/dev/null 2>&1 || true
    smolvm machine delete --name "$machine" -f >/dev/null 2>&1 || true
  done
}

cleaned=0
cleanup() {
  [ "$cleaned" -eq 1 ] && return 0
  stop_all
  rm -rf "$state"
}
trap cleanup EXIT

[ -x "$bin" ] || fail "missing CLI binary at $bin"
[ -x "$runtime/smolvm" ] || fail "missing runtime at $runtime/smolvm"

rm -rf "$state"
mkdir -p "$state/home" "$state/tmp" "$state/home/.config" "$state/home/.cache"

printf 'fleet_two_hosts: creating three machines on %s (2 vCPU / 4 GiB each)\n' "$network"
for machine in "$coord" "$host_a" "$host_b"; do
  smolvm machine create --name "$machine" -I "$image" --network "$network" \
    --cpus 2 --mem 4096 -v "$repo:/work/smolrunners:ro" >/dev/null 2>&1 \
    || fail "create $machine"
done
for machine in "$coord" "$host_a" "$host_b"; do
  smolvm machine start --name "$machine" >/dev/null 2>&1 || fail "start $machine"
done
for machine in "$coord" "$host_a" "$host_b"; do
  smolvm machine exec --name "$machine" -- apk add -q curl >/dev/null 2>&1 \
    || fail "install curl in $machine"
done

coord_ip=$(smolvm machine exec --name "$coord" -- \
  sh -c "ip -4 addr show eth0 | awk '/inet /{print \$2}' | cut -d/ -f1" | tr -d '\r')
[ -n "$coord_ip" ] || fail 'coordinator has no network address'

smolvm machine exec --name "$coord" -- mkdir -p /state
smolvm machine exec -d --name "$coord" -- "$bin" serve \
  --listen "0.0.0.0:$port" --state-dir /state >"$state/server.log" 2>&1 || fail 'start coordinator'
ready=0
attempt=0
while [ "$attempt" -lt 60 ]; do
  if smolvm machine exec --name "$coord" -- curl -fsS --max-time 2 \
    "$local_base/status" >"$state/status.json" 2>/dev/null; then
    ready=1
    break
  fi
  attempt=$((attempt + 1))
  sleep 1
done
[ "$ready" -eq 1 ] || fail "coordinator did not become ready: $(cat "$state/server.log")"

out=$(call POST /init '{}')
expect_status "$out" 200 'init'

printf 'fleet_two_hosts: single-use invite, local key, fingerprint and mTLS\n'
out=$(call POST /host/invite '{"host_key":"sr-host-a","cert_fingerprint":"cert-a-fp"}')
expect_status "$out" 200 'invite host-a'
token_a=$(body_of "$out" | field data.token)
key_fp=$(body_of "$out" | field data.key_fingerprint)
expected_fp=$(python3 -c 'import hashlib; print(hashlib.sha256(b"sr-host-a").hexdigest())')
[ "$key_fp" = "$expected_fp" ] || fail 'invite key fingerprint is not a SHA-256 of the local key'

out=$(call POST /host/join "{\"token\":\"$token_a\",\"hostname\":\"sr-host-a\",\"arch\":\"arm64\",\"vcpu\":2,\"memory_gib\":4,\"host_key\":\"wrong-key\",\"cert_fingerprint\":\"cert-a-fp\"}")
expect_status "$out" 401 'join with wrong local key'
body_of "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["data"]["message"]=="fingerprint_mismatch", d' \
  || fail 'fingerprint mismatch was not reported'

out=$(smolvm machine exec --name "$host_a" -- env "SMOLRUNNERS_API_URL=http://$coord_ip:$port" \
  "$bin" host join --token "$token_a" --hostname sr-host-a --arch arm64 \
  --vcpu 2 --memory-gib 4 --host-key sr-host-a --cert-fingerprint cert-a-fp \
  --non-interactive --json 2>&1)
host_a_id=$(printf '%s\n' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["id"])')
[ "$host_a_id" -gt 0 ] || fail "join from sr-host-a failed: $out"

out=$(call POST /host/join "{\"token\":\"$token_a\",\"hostname\":\"sr-host-a\",\"arch\":\"arm64\",\"vcpu\":2,\"memory_gib\":4,\"host_key\":\"sr-host-a\",\"cert_fingerprint\":\"cert-a-fp\"}")
expect_status "$out" 409 'reuse of consumed invite'

inspect=$(inspect_host "$host_a_id" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["data"]))')
[ "$(printf '%s' "$inspect" | field key_fingerprint)" = "$expected_fp" ] || fail 'joined host key fingerprint differs'
[ "$(printf '%s' "$inspect" | field cert_fingerprint)" = 'cert-a-fp' ] || fail 'joined host certificate fingerprint differs'
[ "$(printf '%s' "$inspect" | field mtls)" = 'True' ] || fail 'joined host did not record mTLS'
[ "$(printf '%s' "$inspect" | field incarnation)" = '1' ] || fail 'joined host incarnation'

out=$(call POST /host/invite '{"host_key":"sr-host-b","cert_fingerprint":"cert-b-fp"}')
expect_status "$out" 200 'invite host-b'
token_b=$(body_of "$out" | field data.token)
out=$(smolvm machine exec --name "$host_b" -- env "SMOLRUNNERS_API_URL=http://$coord_ip:$port" \
  "$bin" host join --token "$token_b" --hostname sr-host-b --arch arm64 \
  --vcpu 2 --memory-gib 3 --host-key sr-host-b --cert-fingerprint cert-b-fp \
  --non-interactive --json 2>&1)
host_b_id=$(printf '%s\n' "$out" | python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["id"])')
[ "$host_b_id" -gt 0 ] || fail "join from sr-host-b failed: $out"

printf 'fleet_two_hosts: jobs go only to capability-matching hosts\n'
for spec in 'big-2c4g 2 4' 'small-1c2g 1 2' 'huge-4c8g 4 8'; do
  set -- $spec
  out=$(call POST /profiles/apply "{\"name\":\"$1\",\"vcpu\":$2,\"memory_gib\":$3,\"arch\":\"arm64\"}")
  expect_status "$out" 200 "apply profile $1"
done

job_big=$(demand fleet-big 1 big-2c4g)
out=$(start_job "$job_big")
expect_status "$out" 200 'start big job'
[ "$(body_of "$out" | field data.host_id)" = "$host_a_id" ] || fail 'big job did not go to the 2c4g host'

job_small=$(demand fleet-small 2 small-1c2g)
out=$(start_job "$job_small")
expect_status "$out" 200 'start small job'
[ "$(body_of "$out" | field data.host_id)" = "$host_b_id" ] || fail 'small job did not go to the smaller matching host'

job_huge=$(demand fleet-huge 3 huge-4c8g)
out=$(start_job "$job_huge")
expect_status "$out" 409 'start impossible job'
body_of "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); assert "no eligible host" in d["data"]["message"], d' \
  || fail 'impossible job had no capability-matching reason'

printf 'fleet_two_hosts: drain finishes current work and admits nothing new\n'
out=$(hostctl hosts drain "$host_a_id" --non-interactive --json)
drain_out=$(printf '%s\n' "$out" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["data"]))')
[ "$(printf '%s' "$drain_out" | field next)" = 'drained' ] || fail 'drain did not change admission state'
[ "$(printf '%s' "$drain_out" | field drain_revision)" = '1' ] || fail 'drain did not bump the revision'
out=$(call GET "/jobs/$job_big")
expect_status "$out" 200 'inspect busy job after drain'
[ "$(body_of "$out" | field data.state)" = 'busy' ] || fail 'drain killed current work'
complete_job "$job_big"

job_big2=$(demand fleet-big2 4 big-2c4g)
out=$(start_job "$job_big2")
expect_status "$out" 409 'start new job on drained host'
out=$(hostctl hosts resume "$host_a_id" --non-interactive --json)
resume_out=$(printf '%s\n' "$out" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["data"]))')
[ "$(printf '%s' "$resume_out" | field next)" = 'eligible' ] || fail 'resume did not restore admission state'
[ "$(printf '%s' "$resume_out" | field drain_revision)" = '2' ] || fail 'resume did not bump the revision'
out=$(start_job "$job_big2")
expect_status "$out" 200 'start job after resume'
[ "$(body_of "$out" | field data.host_id)" = "$host_a_id" ] || fail 'resumed host did not receive the job'
complete_job "$job_big2"

printf 'fleet_two_hosts: a revoked host gets no work\n'
out=$(hostctl hosts revoke "$host_b_id" --non-interactive --json)
revoke_out=$(printf '%s\n' "$out" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["data"]))')
[ "$(printf '%s' "$revoke_out" | field next)" = 'revoked' ] || fail 'revoke did not change admission state'
[ "$(printf '%s' "$revoke_out" | field drain_revision)" = '1' ] || fail 'revoke did not bump the revision'
job_revoked=$(demand fleet-revoked 5 small-1c2g)
out=$(start_job "$job_revoked" "{\"host_id\":$host_b_id}")
expect_status "$out" 409 'start job on revoked host'
body_of "$out" | python3 -c 'import json,sys; d=json.load(sys.stdin); assert "revoked" in d["data"]["message"], d' \
  || fail 'revoked host had no revocation reason'
job_elsewhere=$(demand fleet-elsewhere 6 small-1c2g)
out=$(start_job "$job_elsewhere")
expect_status "$out" 200 'start job after revocation'
[ "$(body_of "$out" | field data.host_id)" = "$host_a_id" ] || fail 'work did not move to the eligible host'
complete_job "$job_elsewhere"

printf 'fleet_two_hosts: a killed host never frees its capacity\n'
job_killed=$(demand fleet-killed 7 big-2c4g)
out=$(start_job "$job_killed")
expect_status "$out" 200 'start job before killing host'
[ "$(body_of "$out" | field data.host_id)" = "$host_a_id" ] || fail 'job did not start on the host being killed'
smolvm machine stop --name "$host_a" >/dev/null 2>&1 || true
out=$(hostctl hosts kill "$host_a_id" --non-interactive --json)
kill_out=$(printf '%s\n' "$out" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["data"]))')
[ "$(printf '%s' "$kill_out" | field next)" = 'lost' ] || fail 'kill did not mark the host lost'
[ "$(printf '%s' "$kill_out" | field drain_revision)" = '3' ] || fail 'kill did not bump the revision'
inspect=$(inspect_host "$host_a_id" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["data"]))')
[ "$(printf '%s' "$inspect" | field used_vcpu)" = '2' ] || fail 'killed host freed its vCPU'
[ "$(printf '%s' "$inspect" | field used_memory_gib)" = '4' ] || fail 'killed host freed its memory'
[ "$(printf '%s' "$inspect" | field last_contact_ms)" = 'None' ] || fail 'killed host still has contact'
job_killed2=$(demand fleet-killed2 8 big-2c4g)
out=$(start_job "$job_killed2")
expect_status "$out" 409 'start job on killed host'
complete_job "$job_killed"
inspect=$(inspect_host "$host_a_id" | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)["data"]))')
[ "$(printf '%s' "$inspect" | field used_vcpu)" = '2' ] || fail 'killed host capacity freed after its job completed'
[ "$(printf '%s' "$inspect" | field used_memory_gib)" = '4' ] || fail 'killed host memory freed after its job completed'

printf 'fleet_two_hosts: leaving no sr-* resources\n'
stop_all
leftovers=$(smolvm machine ls 2>/dev/null | awk '$1 ~ /^sr-/ {print $1}' || true)
[ -z "$leftovers" ] || fail "leftover machines: $leftovers"
rm -rf "$state"
cleaned=1
printf 'fleet_two_hosts: ok\n'
