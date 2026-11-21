package main

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"sync"
	"testing"

	"github.com/actions/scaleset/listener"
	"github.com/joshuarli/smolrunners/adapter/internal/fakebroker"
	"github.com/joshuarli/smolrunners/adapter/proto"
	"github.com/joshuarli/smolrunners/adapter/scaler"
)

var errLifecycleCrash = errors.New("coordinator killed at effect boundary")

type lifecycleRecord struct {
	EventID    string  `json:"event_id"`
	Kind       string  `json:"kind"`
	Decision   string  `json:"decision"`
	JobID      string  `json:"job_id,omitempty"`
	RequestID  *int64  `json:"request_id,omitempty"`
	RequestIDs []int64 `json:"request_ids,omitempty"`
	Recorded   bool    `json:"recorded"`
}

type durableLifecycleCoordinator struct {
	mu        sync.Mutex
	path      string
	records   map[string]lifecycleRecord
	acks      []proto.Frame
	killAt    string
	killPhase string
}

func loadDurableCoordinator(t *testing.T, path string, killAt, killPhase string) *durableLifecycleCoordinator {
	t.Helper()
	c := &durableLifecycleCoordinator{
		path:      path,
		records:   make(map[string]lifecycleRecord),
		killAt:    killAt,
		killPhase: killPhase,
	}
	file, err := os.Open(path)
	if errors.Is(err, os.ErrNotExist) {
		return c
	}
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	scanner := bufio.NewScanner(file)
	var reconciled []lifecycleRecord
	for scanner.Scan() {
		var record lifecycleRecord
		if err := json.Unmarshal(scanner.Bytes(), &record); err != nil {
			t.Fatal(err)
		}
		c.records[record.EventID] = record
		if record.Kind == proto.KindAcquireOutcome && record.Decision == "uncertain" {
			reconciled = append(reconciled, c.markOfferUncertain(record.RequestIDs)...)
		}
	}
	if err := scanner.Err(); err != nil {
		t.Fatal(err)
	}
	for _, record := range reconciled {
		c.records[record.EventID] = record
		if err := c.append(record); err != nil {
			t.Fatal(err)
		}
	}
	return c
}

func (c *durableLifecycleCoordinator) markOfferUncertain(requestIDs []int64) []lifecycleRecord {
	var changed []lifecycleRecord
	for id, record := range c.records {
		if record.Kind != proto.KindJobAvailable || record.Decision == "uncertain" {
			continue
		}
		offeredIDs := append([]int64(nil), record.RequestIDs...)
		if record.RequestID != nil {
			offeredIDs = append(offeredIDs, *record.RequestID)
		}
		for _, requestID := range requestIDs {
			for _, offered := range offeredIDs {
				if offered == requestID {
					record.Decision = "uncertain"
					changed = append(changed, record)
					c.records[id] = record
					break
				}
			}
		}
	}
	return changed
}

func (c *durableLifecycleCoordinator) append(record lifecycleRecord) error {
	file, err := os.OpenFile(c.path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return err
	}
	defer file.Close()
	line, err := json.Marshal(record)
	if err != nil {
		return err
	}
	_, err = file.Write(append(line, '\n'))
	return err
}

func (c *durableLifecycleCoordinator) Submit(_ context.Context, frame proto.Frame) (proto.Frame, error) {
	c.mu.Lock()
	defer c.mu.Unlock()

	if existing, ok := c.records[frame.ID]; ok {
		return proto.AckFor(frame.ID, existing.Decision), nil
	}

	record := lifecycleRecord{
		EventID:    frame.ID,
		Kind:       frame.Kind,
		Decision:   "accept",
		JobID:      frame.JobID,
		RequestID:  frame.RequestID,
		RequestIDs: append([]int64(nil), frame.RequestIDs...),
		Recorded:   true,
	}
	if frame.Reason != "" {
		record.Decision = "reject"
	}
	kill := frame.ID == c.killAt
	if kill {
		record.Decision = "uncertain"
	}

	if err := c.append(record); err != nil {
		return proto.Frame{}, err
	}
	c.records[frame.ID] = record
	if kill {
		return proto.Frame{}, errLifecycleCrash
	}

	ack := proto.AckFor(frame.ID, record.Decision)
	c.acks = append(c.acks, ack)
	return ack, nil
}

func (c *durableLifecycleCoordinator) hasAck(eventID string) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	for _, ack := range c.acks {
		if ack.EventID == eventID {
			return true
		}
	}
	return false
}

func (c *durableLifecycleCoordinator) acquisitionRecorded(eventID string) bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	record, ok := c.records[eventID]
	return ok && record.Kind == proto.KindAcquireOutcome
}

func runLifecycleScenario(t *testing.T, scenario string, coordinator scaler.Coordinator) (*fakebroker.Client, error) {
	t.Helper()
	loaded, err := fakebroker.LoadScenario(fakebroker.ScenarioPath(filepath.Join("testdata", "scenarios"), scenario))
	if err != nil {
		t.Fatal(err)
	}
	broker := fakebroker.New(loaded)
	s := scaler.New(broker, coordinator, 8)
	l, err := listener.New(broker, listener.Config{ScaleSetID: 1, MaxRunners: 8})
	if err != nil {
		t.Fatal(err)
	}
	err = l.Run(context.Background(), s)
	if errors.Is(err, errLifecycleCrash) {
		return broker, err
	}
	if err != nil && !errors.Is(err, fakebroker.ErrScenarioEnd) {
		t.Fatalf("listener run: %v", err)
	}
	return broker, nil
}

func TestScenarioLifecycleCrashBeforeAcquireHasNoUnpersistedAck(t *testing.T) {
	path := filepath.Join(t.TempDir(), "lifecycle.jsonl")
	const offerID = "evt-m100-offer-0-101"
	first := loadDurableCoordinator(t, path, offerID, "before")

	broker, err := runLifecycleScenario(t, "job-available-to-acquire", first)
	if !errors.Is(err, errLifecycleCrash) {
		t.Fatalf("first run error = %v, want coordinator crash", err)
	}
	if calls := broker.AcquireCalls(); len(calls) != 0 {
		t.Fatalf("acquire ran before offer was durably acked: %v", calls)
	}
	if first.hasAck(offerID) {
		t.Fatal("offer was acked before its acquisition was persisted")
	}
	if first.acquisitionRecorded("evt-m100-acquire") {
		t.Fatal("acquisition was recorded despite crash before the effect")
	}

	restarted := loadDurableCoordinator(t, path, "", "")
	secondBroker, err := runLifecycleScenario(t, "job-available-to-acquire", restarted)
	if err != nil {
		t.Fatalf("restart run: %v", err)
	}
	if calls := secondBroker.AcquireCalls(); len(calls) != 0 {
		t.Fatalf("uncertain offer was replayed after restart: %v", calls)
	}
	record := restarted.records[offerID]
	if record.Decision != "uncertain" {
		t.Fatalf("offer decision after restart = %q, want uncertain", record.Decision)
	}
}

func TestScenarioLifecycleCrashAfterAcquireNeverReplays(t *testing.T) {
	path := filepath.Join(t.TempDir(), "lifecycle.jsonl")
	const acquireID = "evt-m100-acquire"
	first := loadDurableCoordinator(t, path, acquireID, "after")

	broker, firstErr := runLifecycleScenario(t, "job-available-to-acquire", first)
	if !errors.Is(firstErr, errLifecycleCrash) {
		t.Fatalf("first run error = %v, want coordinator crash", firstErr)
	}
	if calls := broker.AcquireCalls(); len(calls) != 1 {
		t.Fatalf("acquire calls = %v, want one", calls)
	}
	if !first.acquisitionRecorded(acquireID) {
		t.Fatal("acquisition was not persisted before the crash")
	}
	if first.hasAck(acquireID) {
		t.Fatal("acquire outcome was acked before its acquisition was persisted")
	}

	restarted := loadDurableCoordinator(t, path, "", "")
	secondBroker, err := runLifecycleScenario(t, "job-available-to-acquire", restarted)
	if err != nil {
		t.Fatalf("restart run: %v", err)
	}
	if calls := secondBroker.AcquireCalls(); len(calls) != 0 {
		t.Fatalf("uncertain acquisition was replayed after restart: %v", calls)
	}
	if !restarted.acquisitionRecorded(acquireID) {
		t.Fatal("acquisition record lost across restart")
	}
}

func TestScenarioLifecycleRedeliveryIsRecordedOnce(t *testing.T) {
	path := filepath.Join(t.TempDir(), "lifecycle.jsonl")
	coordinator := loadDurableCoordinator(t, path, "", "")
	_, err := runLifecycleScenario(t, "redelivery", coordinator)
	if err != nil {
		t.Fatalf("redelivery scenario: %v", err)
	}
	offers := 0
	for _, record := range coordinator.records {
		if record.Kind == proto.KindJobAvailable {
			offers++
		}
	}
	if offers != 1 {
		t.Fatalf("offer records = %d, want one idempotent record", offers)
	}
}
