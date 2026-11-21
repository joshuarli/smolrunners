package scaler

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/actions/scaleset"
	"github.com/actions/scaleset/listener"
	"github.com/joshuarli/smolrunners/adapter/proto"
)

const assignmentExpiry = time.Hour

type idGenerator struct {
	prefix string
	seq    uint64
}

func (g *idGenerator) next(kind string) string {
	g.seq++
	return fmt.Sprintf("%s-%s-%d", g.prefix, kind, g.seq)
}

func (g *idGenerator) forMessage(messageID int, suffix string) string {
	return fmt.Sprintf("%s-m%d-%s", g.prefix, messageID, suffix)
}

type Scaler struct {
	client      listener.Client
	coordinator Coordinator
	maxRunners  int
	ids         idGenerator
	mu          sync.Mutex
	auth        SessionAuth
}

func New(client listener.Client, coordinator Coordinator, maxRunners int) *Scaler {
	return &Scaler{
		client:      client,
		coordinator: coordinator,
		maxRunners:  maxRunners,
		ids:         idGenerator{prefix: "evt"},
	}
}

var _ listener.Scaler = (*Scaler)(nil)

func (s *Scaler) revoked() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.auth.Revoked
}

func expired(assignTime time.Time) bool {
	return !assignTime.IsZero() && time.Since(assignTime) > assignmentExpiry
}

func (s *Scaler) Scale(ctx context.Context, msg *scaleset.RunnerScaleSetMessage) error {
	if msg == nil {
		return nil
	}

	if msg.Statistics != nil {
		assigned := int64(msg.Statistics.TotalAssignedJobs)
		reported := assigned
		if s.maxRunners > 0 && reported > int64(s.maxRunners) {
			reported = int64(s.maxRunners)
		}
		event := proto.Frame{
			Kind:             proto.KindCapacity,
			SchemaVersion:    proto.SchemaVersion,
			ID:               s.ids.forMessage(msg.MessageID, "capacity"),
			AssignedJobs:     proto.Int64(assigned),
			ReportedCapacity: proto.Int64(reported),
		}
		if _, err := s.coordinator.Submit(ctx, event); err != nil {
			return fmt.Errorf("scale: report capacity: %w", err)
		}
	}

	var acceptedIDs []int64
	for index, offer := range msg.JobAvailableMessages {
		if offer == nil {
			continue
		}
		event := proto.Frame{
			Kind:          proto.KindJobAvailable,
			SchemaVersion: proto.SchemaVersion,
			ID:            s.ids.forMessage(msg.MessageID, fmt.Sprintf("offer-%d-%d", index, offer.RunnerRequestID)),
			RequestID:     proto.Int64(offer.RunnerRequestID),
			JobID:         offer.JobID,
			Labels:        append([]string(nil), offer.RequestLabels...),
		}
		if expired(offer.ScaleSetAssignTime) {
			event.Reason = "assignment_expired"
		} else if s.revoked() {
			event.Reason = "installation_revoked"
		}
		ack, err := s.coordinator.Submit(ctx, event)
		if err != nil {
			return fmt.Errorf("scale: submit offer %d: %w", offer.RunnerRequestID, err)
		}
		if ack.Decision == "accept" {
			acceptedIDs = append(acceptedIDs, offer.RunnerRequestID)
		}
	}

	if len(acceptedIDs) > 0 {
		acquired, err := s.client.AcquireJobs(ctx, acceptedIDs)
		if err != nil {
			return fmt.Errorf("scale: acquire jobs: %w", err)
		}
		outcome := proto.Frame{
			Kind:          proto.KindAcquireOutcome,
			SchemaVersion: proto.SchemaVersion,
			ID:            s.ids.forMessage(msg.MessageID, "acquire"),
			RequestIDs:    append([]int64{}, acceptedIDs...),
			AcquiredIDs:   append([]int64{}, acquired...),
			JobID:         firstJobID(msg, acceptedIDs),
		}
		if _, err := s.coordinator.Submit(ctx, outcome); err != nil {
			return fmt.Errorf("scale: persist acquisition: %w", err)
		}
	}

	for _, assignment := range msg.JobAssignedMessages {
		if assignment == nil {
			continue
		}
		event := proto.Frame{
			Kind:          proto.KindJobAssigned,
			SchemaVersion: proto.SchemaVersion,
			ID:            s.ids.forMessage(msg.MessageID, fmt.Sprintf("assigned-%d", assignment.RunnerRequestID)),
			RequestID:     proto.Int64(assignment.RunnerRequestID),
			JobID:         assignment.JobID,
		}
		if expired(assignment.ScaleSetAssignTime) {
			event.Reason = "assignment_expired"
		} else if s.revoked() {
			event.Reason = "installation_revoked"
		}
		if _, err := s.coordinator.Submit(ctx, event); err != nil {
			return fmt.Errorf("scale: submit assignment: %w", err)
		}
	}

	for _, started := range msg.JobStartedMessages {
		if started == nil {
			continue
		}
		event := proto.Frame{
			Kind:          proto.KindJobStarted,
			SchemaVersion: proto.SchemaVersion,
			ID:            s.ids.forMessage(msg.MessageID, fmt.Sprintf("started-%d", started.RunnerRequestID)),
			RequestID:     proto.Int64(started.RunnerRequestID),
			JobID:         started.JobID,
			RunnerName:    started.RunnerName,
		}
		if _, err := s.coordinator.Submit(ctx, event); err != nil {
			return fmt.Errorf("scale: submit started: %w", err)
		}
	}

	for _, completed := range msg.JobCompletedMessages {
		if completed == nil {
			continue
		}
		event := proto.Frame{
			Kind:          proto.KindJobCompleted,
			SchemaVersion: proto.SchemaVersion,
			ID:            s.ids.forMessage(msg.MessageID, fmt.Sprintf("completed-%d", completed.RunnerRequestID)),
			RequestID:     proto.Int64(completed.RunnerRequestID),
			JobID:         completed.JobID,
			Result:        completed.Result,
		}
		if _, err := s.coordinator.Submit(ctx, event); err != nil {
			return fmt.Errorf("scale: submit completed: %w", err)
		}
		teardown := proto.Frame{
			Kind:          proto.KindVmDelete,
			SchemaVersion: proto.SchemaVersion,
			ID:            s.ids.forMessage(msg.MessageID, fmt.Sprintf("teardown-%d", completed.RunnerRequestID)),
			JobID:         completed.JobID,
		}
		if _, err := s.coordinator.Submit(ctx, teardown); err != nil {
			return fmt.Errorf("scale: request vm teardown: %w", err)
		}
	}

	return nil
}

func firstJobID(msg *scaleset.RunnerScaleSetMessage, requestIDs []int64) string {
	for _, offer := range msg.JobAvailableMessages {
		if offer == nil {
			continue
		}
		for _, id := range requestIDs {
			if offer.RunnerRequestID == id {
				return offer.JobID
			}
		}
	}
	return ""
}
