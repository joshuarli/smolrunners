#!/bin/sh
set -u

worktree=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd) || exit 1
cd "$worktree" || exit 1

export TMPDIR="$worktree/.work/tmp"
mkdir -p "$TMPDIR" || exit 1

run_dir="$TMPDIR/fault-corpus-$$"
mkdir -p "$run_dir" || exit 1
trap 'rm -rf "$run_dir"' EXIT HUP INT TERM

clock() { python3 -c 'import time; print(time.time())'; }
elapsed() { python3 -c "print(f'{$2 - $1:.2f}')"; }

started=$(clock)
passed=0
failed=0
total=0
sr_before=$(pgrep -fl 'sr-(coord|host-a|host-b|dev)' 2>/dev/null | wc -l | tr -d ' ')

printf 'faults: local fault corpus (worktree %s)\n' "$worktree"
printf 'faults: scratch %s\n' "$TMPDIR"
printf 'faults: sr processes before = %s\n' "$sr_before"

run_rust_case() {
  case_name=$1
  test_path=$2
  total=$((total + 1))
  log="$run_dir/$case_name.log"
  case_started=$(clock)
  if TMPDIR="$TMPDIR" cargo test -q -p smolrunners-core --lib "$test_path" -- --exact --nocapture >"$log" 2>&1; then
    case_ended=$(clock)
    printf 'CASE %-46s PASS %ss\n' "$case_name" "$(elapsed "$case_started" "$case_ended")"
    grep '^FAULT_STATE case=' "$log" | sed 's/^/  /'
    passed=$((passed + 1))
  else
    case_ended=$(clock)
    printf 'CASE %-46s FAIL %ss\n' "$case_name" "$(elapsed "$case_started" "$case_ended")"
    sed 's/^/  /' "$log"
    failed=$((failed + 1))
  fi
}

run_go_case() {
  case_name=$1
  pattern=$2
  durable=$3
  resources=$4
  total=$((total + 1))
  log="$run_dir/$case_name.log"
  case_started=$(clock)
  if (cd adapter && go test ./... -count=1 -run "$pattern" -v) >"$log" 2>&1; then
    case_ended=$(clock)
    printf 'CASE %-46s PASS %ss\n' "$case_name" "$(elapsed "$case_started" "$case_ended")"
    grep -- '--- PASS:' "$log" | sed 's/^/  evidence /'
    printf '  FAULT_STATE case=%s durable=%s\n' "$case_name" "$durable"
    printf '  FAULT_STATE case=%s resources=%s\n' "$case_name" "$resources"
    passed=$((passed + 1))
  else
    case_ended=$(clock)
    printf 'CASE %-46s FAIL %ss\n' "$case_name" "$(elapsed "$case_started" "$case_ended")"
    sed 's/^/  /' "$log"
    failed=$((failed + 1))
  fi
}

run_rust_case controller_crash lifecycle::harness::faults::f01_controller_crash
run_rust_case host_agent_crash lifecycle::harness::faults::f02_host_agent_crash
run_rust_case vm_crash lifecycle::harness::faults::f03_vm_crash
run_rust_case collector_failure lifecycle::harness::faults::f04_collector_failure
run_rust_case message_redelivery lifecycle::harness::faults::f05_message_redelivery
run_rust_case lost_ack lifecycle::harness::faults::f06_lost_ack
run_rust_case stale_jit_material lifecycle::harness::faults::f07_stale_jit_material
run_rust_case assignment_expiry lifecycle::harness::faults::f08_assignment_expiry
run_rust_case cancel_queued_or_running lifecycle::harness::faults::f09_cancel_queued_and_running
run_rust_case clock_and_counter_resets lifecycle::harness::faults::f10_clock_and_counter_resets
run_rust_case wal_disk_pressure lifecycle::harness::faults::f11_wal_disk_pressure
run_rust_case interrupted_generation_publication lifecycle::harness::faults::f12_interrupted_generation_publication
run_rust_case cache_connections_across_vm_reuse lifecycle::harness::faults::f13_cache_connections_across_vm_reuse
run_rust_case image_runner_rotation_drains_idle lifecycle::harness::faults::f14_image_runner_rotation_drains_idle_capacity
run_rust_case restore_requires_host_reconcile lifecycle::harness::faults::f15_restore_admits_nothing_until_hosts_reconcile
run_rust_case storage_exhaustion lifecycle::harness::faults::f16_storage_exhaustion

run_go_case message_redelivery_fakebroker \
  '^(TestScenarioRedelivery|TestScenarioLifecycleRedeliveryIsRecordedOnce)$' \
  'same_event_id_reused=1 durable_offer_records=1' \
  'duplicate_execution=0 broker_vms=0'
run_go_case burst_120_fakebroker \
  '^(TestScenarioBurstTruncatedTo50|TestBurstTruncationBatches)$' \
  'scenario=burst-120-truncated-to-50 offers=120 acquire_batches=3' \
  'messages_bounded=yes broker_vms=0'
run_go_case assignment_expiry_fakebroker \
  '^TestScenarioExpiredAssignment$' \
  'scenario=expired-assignment reason=assignment_expired acquire_calls=0' \
  'running_vms=0'
run_go_case broker_session_failure \
  '^(TestScenarioSessionRefreshFailure|TestSessionFailureNeverDeletesRunningVM)$' \
  'scenario=session-refresh-failure running_vm_kept=1 deleted_messages=0' \
  'deleted_running_vm=0'
run_go_case durable_ack_fakebroker \
  '^(TestAckOnlyAfterDurableRecording|TestAcquireAwaitPersistedBeforeAck)$' \
  'ack_recorded=1 acquire_persisted_before_ack=1' \
  'unpersisted_acks=0'

sr_after=$(pgrep -fl 'sr-(coord|host-a|host-b|dev)' 2>/dev/null | wc -l | tr -d ' ')
printf 'faults: sr processes after = %s\n' "$sr_after"
if [ "$sr_after" -ne "$sr_before" ]; then
  printf 'faults: FAIL new sr-* resources were left behind\n' >&2
  failed=$((failed + 1))
fi

printf 'PREREQUISITE live-github: broker cancel, session restart, HTTP/2 blackhole, >50-job burst, rate limits and a 90-minute job need a GitHub App key; make live-faults (NE1) with SMOLRUNNERS_GITHUB_APP_FILE\n'
printf 'PREREQUISITE host-agent/VM crash of a real guest: make test-vm T=host_lifecycle (H1) covers kill -9 mid-boot and reaping; nested-host timings are functional evidence only\n'
printf 'PREREQUISITE collector failure inside a live guest: make test-vm T=telemetry_history\n'
printf 'PREREQUISITE image and runner rotation with real runner binaries: make live-corpus (NE1)\n'
printf 'PREREQUISITE restore admission gate enforcement: the local case proves durable state and that every restored host is stale until re-heartbeaten; the admission gate itself is enforced by the coordinator binary, outside the F1 source scope\n'

ended=$(clock)
printf 'faults: summary total=%s passed=%s failed=%s duration=%ss\n' \
  "$total" "$passed" "$failed" "$(elapsed "$started" "$ended")"

[ "$failed" -eq 0 ] || exit 1
printf 'faults: ok\n'
