package fakebroker

import (
	"context"
	"errors"
	"path/filepath"
	"testing"

	"github.com/google/uuid"
)

func scenarioPath(name string) string {
	return filepath.Join("..", "..", "testdata", "scenarios", name+".json")
}

var scenarioNames = []string{
	"statistics-only-first-message",
	"nil-callback",
	"job-available-to-acquire",
	"direct-job-assigned",
	"redelivery",
	"burst-120-truncated-to-50",
	"zero-capacity-profile",
	"expired-assignment",
	"session-refresh-failure",
}

func TestAllD7ScenariosLoad(t *testing.T) {
	for _, name := range scenarioNames {
		t.Run(name, func(t *testing.T) {
			scenario, err := LoadScenario(scenarioPath(name))
			if err != nil {
				t.Fatal(err)
			}
			if scenario.Name != name {
				t.Fatalf("name = %q, want %q", scenario.Name, name)
			}
			client := New(scenario)
			session := client.Session()
			if session.SessionID == uuid.Nil {
				t.Fatal("session id is nil")
			}
		})
	}
}

func TestBurstTruncationBatches(t *testing.T) {
	scenario, err := LoadScenario(scenarioPath("burst-120-truncated-to-50"))
	if err != nil {
		t.Fatal(err)
	}
	client := New(scenario)
	want := []int{50, 50, 20}
	wantIDs := []int{600, 601, 602}
	for i, count := range want {
		msg, err := client.GetMessage(context.Background(), 0, 200)
		if err != nil {
			t.Fatal(err)
		}
		if len(msg.JobAvailableMessages) != count {
			t.Fatalf("batch %d: %d offers, want %d", i, len(msg.JobAvailableMessages), count)
		}
		if msg.MessageID != wantIDs[i] {
			t.Fatalf("batch %d: message id %d, want %d", i, msg.MessageID, wantIDs[i])
		}
	}
	msg, err := client.GetMessage(context.Background(), 602, 200)
	if err != nil || msg != nil {
		t.Fatalf("nil step after batches: msg=%v err=%v", msg, err)
	}
	if _, err := client.GetMessage(context.Background(), 602, 200); !errors.Is(err, ErrScenarioEnd) {
		t.Fatalf("after batches err = %v", err)
	}
}

func TestNilStepAndDeleteAndAcquire(t *testing.T) {
	scenario, err := LoadScenario(scenarioPath("nil-callback"))
	if err != nil {
		t.Fatal(err)
	}
	client := New(scenario)
	msg, err := client.GetMessage(context.Background(), 0, 4)
	if err != nil || msg != nil {
		t.Fatalf("nil step: msg=%v err=%v", msg, err)
	}
	if err := client.DeleteMessage(context.Background(), 42); err != nil {
		t.Fatal(err)
	}
	got, err := client.AcquireJobs(context.Background(), []int64{1, 2})
	if err != nil {
		t.Fatal(err)
	}
	if len(got) != 0 {
		t.Fatalf("unscripted acquire = %v", got)
	}
	if len(client.DeletedMessages()) != 1 || client.DeletedMessages()[0] != 42 {
		t.Fatalf("deleted = %v", client.DeletedMessages())
	}
	if len(client.AcquireCalls()) != 1 {
		t.Fatalf("acquire calls = %v", client.AcquireCalls())
	}
}

func TestSessionRefreshFailureStep(t *testing.T) {
	scenario, err := LoadScenario(scenarioPath("session-refresh-failure"))
	if err != nil {
		t.Fatal(err)
	}
	client := New(scenario)
	if _, err := client.GetMessage(context.Background(), 0, 4); err == nil {
		t.Fatal("expected session refresh failure")
	}
}
