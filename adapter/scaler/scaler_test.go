package scaler

import (
	"context"
	"errors"
	"math/rand"
	"path/filepath"
	"testing"
	"time"

	"github.com/actions/scaleset/listener"
	"github.com/joshuarli/smolrunners/adapter/internal/fakebroker"
	"github.com/joshuarli/smolrunners/adapter/proto"
)

func scenarioPath(name string) string {
	return filepath.Join("..", "testdata", "scenarios", name+".json")
}

func runScenario(t *testing.T, name string, maxRunners int) (*RecordingCoordinator, *fakebroker.Client, error) {
	t.Helper()
	scenario, err := fakebroker.LoadScenario(scenarioPath(name))
	if err != nil {
		t.Fatal(err)
	}
	broker := fakebroker.New(scenario)
	coordinator := NewRecordingCoordinator()
	scale := New(broker, coordinator, maxRunners)
	l, err := listener.New(broker, listener.Config{ScaleSetID: 1, MaxRunners: maxRunners})
	if err != nil {
		t.Fatal(err)
	}
	err = l.Run(context.Background(), scale)
	return coordinator, broker, err
}

func expectScenarioEnd(t *testing.T, err error) {
	t.Helper()
	if err != nil && !errors.Is(err, fakebroker.ErrScenarioEnd) {
		t.Fatalf("listener error: %v", err)
	}
}

func hasKind(frames []proto.Frame, kind string) bool {
	for _, f := range frames {
		if f.Kind == kind {
			return true
		}
	}
	return false
}

func TestScenarioStatisticsOnlyFirstMessage(t *testing.T) {
	coordinator, broker, err := runScenario(t, "statistics-only-first-message", 4)
	expectScenarioEnd(t, err)
	frames := coordinator.Recorded()
	if len(frames) != 1 || frames[0].Kind != proto.KindCapacity {
		t.Fatalf("frames = %+v", frames)
	}
	if frames[0].AssignedJobs == nil || *frames[0].AssignedJobs != 2 {
		t.Fatalf("assigned_jobs = %+v", frames[0].AssignedJobs)
	}
	if len(broker.AcquireCalls()) != 0 {
		t.Fatal("statistics-only callback acquired jobs")
	}
}

func TestScenarioNilCallback(t *testing.T) {
	coordinator, broker, err := runScenario(t, "nil-callback", 4)
	expectScenarioEnd(t, err)
	frames := coordinator.Recorded()
	if len(frames) != 1 || frames[0].Kind != proto.KindCapacity {
		t.Fatalf("frames = %+v", frames)
	}
	if len(broker.AcquireCalls()) != 0 {
		t.Fatal("nil callback acquired jobs")
	}
}

func TestScenarioJobAvailableToAcquire(t *testing.T) {
	coordinator, broker, err := runScenario(t, "job-available-to-acquire", 8)
	expectScenarioEnd(t, err)
	kinds := coordinator.KindsRecorded()
	if !hasKind(coordinator.Recorded(), proto.KindJobAvailable) || !hasKind(coordinator.Recorded(), proto.KindAcquireOutcome) {
		t.Fatalf("kinds = %v", kinds)
	}
	calls := broker.AcquireCalls()
	if len(calls) != 1 || len(calls[0]) != 1 || calls[0][0] != 101 {
		t.Fatalf("acquire calls = %v", calls)
	}
	if len(broker.DeletedMessages()) != 1 || broker.DeletedMessages()[0] != 100 {
		t.Fatalf("deleted = %v", broker.DeletedMessages())
	}
}

func TestScenarioDirectJobAssigned(t *testing.T) {
	coordinator, broker, err := runScenario(t, "direct-job-assigned", 4)
	expectScenarioEnd(t, err)
	if !hasKind(coordinator.Recorded(), proto.KindJobAssigned) {
		t.Fatalf("kinds = %v", coordinator.KindsRecorded())
	}
	if len(broker.AcquireCalls()) != 0 {
		t.Fatalf("direct assignment should not acquire: %v", broker.AcquireCalls())
	}
	if coordinator.RunningVMCount() != 1 {
		t.Fatalf("running vms = %d", coordinator.RunningVMCount())
	}
}

func TestScenarioRedelivery(t *testing.T) {
	coordinator, broker, err := runScenario(t, "redelivery", 4)
	expectScenarioEnd(t, err)
	var offerIDs []string
	for _, f := range coordinator.Recorded() {
		if f.Kind == proto.KindJobAvailable {
			offerIDs = append(offerIDs, f.ID)
		}
	}
	if len(offerIDs) != 2 || offerIDs[0] != offerIDs[1] {
		t.Fatalf("redelivery must reuse event id: %v", offerIDs)
	}
	if len(broker.DeletedMessages()) != 2 {
		t.Fatalf("deleted = %v", broker.DeletedMessages())
	}
}

func TestScenarioBurstTruncatedTo50(t *testing.T) {
	coordinator, broker, err := runScenario(t, "burst-120-truncated-to-50", 200)
	expectScenarioEnd(t, err)
	offers := 0
	for _, f := range coordinator.Recorded() {
		if f.Kind == proto.KindJobAvailable {
			offers++
		}
	}
	if offers != 120 {
		t.Fatalf("offers = %d, want 120", offers)
	}
	calls := broker.AcquireCalls()
	if len(calls) != 3 {
		t.Fatalf("acquire calls = %d, want 3 (50/50/20)", len(calls))
	}
	total := 0
	for _, call := range calls {
		total += len(call)
	}
	if total != 120 {
		t.Fatalf("acquired %d offers, want 120", total)
	}
}

func TestScenarioZeroCapacityProfile(t *testing.T) {
	coordinator, _, err := runScenario(t, "zero-capacity-profile", 4)
	expectScenarioEnd(t, err)
	for _, f := range coordinator.Recorded() {
		if f.Kind != proto.KindCapacity {
			continue
		}
		if f.ReportedCapacity == nil || *f.ReportedCapacity != 0 {
			t.Fatalf("reported capacity = %+v", f.ReportedCapacity)
		}
	}
}

func TestScenarioExpiredAssignment(t *testing.T) {
	coordinator, broker, err := runScenario(t, "expired-assignment", 4)
	expectScenarioEnd(t, err)
	for _, f := range coordinator.Recorded() {
		if f.Kind == proto.KindJobAvailable || f.Kind == proto.KindJobAssigned {
			if f.Reason != "assignment_expired" {
				t.Fatalf("%s reason = %q", f.Kind, f.Reason)
			}
		}
	}
	if len(broker.AcquireCalls()) != 0 {
		t.Fatalf("expired offer was acquired: %v", broker.AcquireCalls())
	}
	if coordinator.RunningVMCount() != 0 {
		t.Fatalf("expired assignment created VM: %d", coordinator.RunningVMCount())
	}
}

func TestScenarioSessionRefreshFailure(t *testing.T) {
	coordinator, broker, err := runScenario(t, "session-refresh-failure", 4)
	if err == nil {
		t.Fatal("expected session refresh failure")
	}
	if len(broker.DeletedMessages()) != 0 {
		t.Fatalf("session failure deleted messages: %v", broker.DeletedMessages())
	}
	if len(coordinator.Recorded()) != 1 || coordinator.Recorded()[0].Kind != proto.KindCapacity {
		t.Fatalf("frames = %+v", coordinator.Recorded())
	}
}

func TestAckOnlyAfterDurableRecording(t *testing.T) {
	coordinator := NewRecordingCoordinator()
	scenario, err := fakebroker.LoadScenario(scenarioPath("job-available-to-acquire"))
	if err != nil {
		t.Fatal(err)
	}
	broker := fakebroker.New(scenario)
	scale := New(broker, coordinator, 8)
	frame := proto.Frame{
		Kind:          proto.KindCapacity,
		SchemaVersion: proto.SchemaVersion,
		ID:            "evt-durable",
		AssignedJobs:  proto.Int64(1),
	}
	ack, err := coordinator.Submit(context.Background(), frame)
	if err != nil {
		t.Fatal(err)
	}
	if len(coordinator.Recorded()) != 1 || len(coordinator.AckOrder()) != 1 {
		t.Fatalf("recorded=%d acked=%d", len(coordinator.Recorded()), len(coordinator.AckOrder()))
	}
	if ack.EventID != frame.ID || ack.Decision != "accept" || ack.Recorded == nil || !*ack.Recorded {
		t.Fatalf("ack = %+v", ack)
	}
	_ = scale
}

func TestAcquireAwaitPersistedBeforeAck(t *testing.T) {
	scenario, err := fakebroker.LoadScenario(scenarioPath("job-available-to-acquire"))
	if err != nil {
		t.Fatal(err)
	}
	broker := fakebroker.New(scenario)
	coordinator := NewRecordingCoordinator()
	coordinator.FailOn("evt-m100-acquire", errors.New("durable store unavailable"))
	scale := New(broker, coordinator, 8)
	l, err := listener.New(broker, listener.Config{ScaleSetID: 1, MaxRunners: 8})
	if err != nil {
		t.Fatal(err)
	}
	err = l.Run(context.Background(), scale)
	if err == nil {
		t.Fatal("expected acquire persistence failure")
	}
	if len(broker.DeletedMessages()) != 0 {
		t.Fatalf("message acked before acquisition persisted: %v", broker.DeletedMessages())
	}
}

func TestReportSessionEvents(t *testing.T) {
	coordinator := NewRecordingCoordinator()
	scale := New(nil, coordinator, 4)

	if err := scale.ReportSession(context.Background(), SessionEvent{
		Kind: SessionKeyRotated, KeyID: "key-2026-11",
	}); err != nil {
		t.Fatal(err)
	}
	if scale.Auth().KeyID != "key-2026-11" || scale.Auth().Revoked {
		t.Fatalf("auth after rotation = %+v", scale.Auth())
	}

	if err := scale.ReportSession(context.Background(), SessionEvent{
		Kind: SessionInstallationRevoked, InstallationID: "inst-9",
	}); err != nil {
		t.Fatal(err)
	}
	if !scale.Auth().Revoked {
		t.Fatal("installation revocation not recorded")
	}
	frames := coordinator.Recorded()
	if frames[1].Kind != proto.KindSession || frames[1].SessionEvent != SessionInstallationRevoked {
		t.Fatalf("frames = %+v", frames)
	}

	if err := scale.ReportSession(context.Background(), SessionEvent{
		Kind: SessionRetryGuidance, RetryAfter: 5 * time.Second,
	}); err != nil {
		t.Fatal(err)
	}
	retry := coordinator.Recorded()[2]
	if retry.RetryAfterMs == nil || *retry.RetryAfterMs != 5000 {
		t.Fatalf("retry frame = %+v", retry)
	}
	guidance := ParseRetryGuidance(SessionEvent{Kind: SessionRetryGuidance, RetryAfter: 5 * time.Second})
	if !guidance.Retryable || guidance.RetryAfter != 5*time.Second {
		t.Fatalf("guidance = %+v", guidance)
	}
	if ParseRetryGuidance(SessionEvent{Kind: SessionInstallationRevoked}).Retryable {
		t.Fatal("revocation must not be retryable")
	}

	if err := scale.ReportSession(context.Background(), SessionEvent{Kind: SessionRecovered}); err != nil {
		t.Fatal(err)
	}
	if scale.Auth().Revoked {
		t.Fatal("recovery did not clear revocation")
	}
}

func TestRecoveryDelayBounds(t *testing.T) {
	rng := rand.New(rand.NewSource(42))
	for attempt := 0; attempt < 12; attempt++ {
		for i := 0; i < 100; i++ {
			d := RecoveryDelay(attempt, rng)
			if d < time.Second || d > 60*time.Second {
				t.Fatalf("attempt %d: delay %v out of bounds", attempt, d)
			}
		}
	}
	if RecoveryDelay(0, nil) != time.Second {
		t.Fatal("zero attempt should be exactly 1s without jitter")
	}
}

func TestSessionFailureNeverDeletesRunningVM(t *testing.T) {
	scenario, err := fakebroker.LoadScenario(scenarioPath("direct-job-assigned"))
	if err != nil {
		t.Fatal(err)
	}
	broker := fakebroker.New(scenario)
	coordinator := NewRecordingCoordinator()
	scale := New(broker, coordinator, 4)

	l, err := listener.New(broker, listener.Config{ScaleSetID: 1, MaxRunners: 4})
	if err != nil {
		t.Fatal(err)
	}
	err = l.Run(context.Background(), scale)
	expectScenarioEnd(t, err)
	if coordinator.RunningVMCount() != 1 {
		t.Fatalf("expected one running VM, got %d", coordinator.RunningVMCount())
	}

	failure := SessionEvent{Kind: SessionRefreshFailed, RetryAfter: 2 * time.Second}
	if err := scale.ReportSession(context.Background(), failure); err != nil {
		t.Fatal(err)
	}
	if coordinator.RunningVMCount() != 1 {
		t.Fatalf("session failure deleted a running VM: %d", coordinator.RunningVMCount())
	}
	for _, f := range coordinator.Recorded() {
		if f.Kind == proto.KindVmDelete {
			t.Fatal("session failure emitted vm_delete")
		}
	}
}
