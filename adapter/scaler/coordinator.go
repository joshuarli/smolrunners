package scaler

import (
	"context"
	"fmt"
	"io"
	"sync"

	"github.com/joshuarli/smolrunners/adapter/proto"
)

type Coordinator interface {
	Submit(ctx context.Context, frame proto.Frame) (proto.Frame, error)
}

type FramedCoordinator struct {
	in  io.Reader
	out io.Writer
	mu  sync.Mutex
}

func NewFramedCoordinator(in io.Reader, out io.Writer) *FramedCoordinator {
	return &FramedCoordinator{in: in, out: out}
}

func (c *FramedCoordinator) Submit(ctx context.Context, frame proto.Frame) (proto.Frame, error) {
	c.mu.Lock()
	defer c.mu.Unlock()

	if err := proto.WriteFrame(c.out, &frame); err != nil {
		return proto.Frame{}, fmt.Errorf("coordinator: write event: %w", err)
	}
	for {
		select {
		case <-ctx.Done():
			return proto.Frame{}, ctx.Err()
		default:
		}
		ack, err := proto.ReadFrame(c.in)
		if err != nil {
			return proto.Frame{}, fmt.Errorf("coordinator: read ack: %w", err)
		}
		if !ack.IsAck() {
			return proto.Frame{}, fmt.Errorf("coordinator: expected ack, got %q", ack.Kind)
		}
		if ack.CorrelationID() != frame.ID {
			return proto.Frame{}, fmt.Errorf("coordinator: ack for %q, want %q", ack.CorrelationID(), frame.ID)
		}
		if ack.Recorded == nil || !*ack.Recorded {
			return proto.Frame{}, fmt.Errorf("coordinator: ack for %q was not recorded durably", frame.ID)
		}
		return *ack, nil
	}
}

type RecordingCoordinator struct {
	mu          sync.Mutex
	recorded    []proto.Frame
	decisions   map[string]string
	failOn      map[string]error
	runningVMs  map[string]struct{}
	deletedVMs  []string
	ackedBefore []string
}

func NewRecordingCoordinator() *RecordingCoordinator {
	return &RecordingCoordinator{
		decisions:  make(map[string]string),
		failOn:     make(map[string]error),
		runningVMs: make(map[string]struct{}),
	}
}

func (c *RecordingCoordinator) Decide(eventID string, decision string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.decisions[eventID] = decision
}

func (c *RecordingCoordinator) FailOn(eventID string, err error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.failOn[eventID] = err
}

func (c *RecordingCoordinator) Submit(ctx context.Context, frame proto.Frame) (proto.Frame, error) {
	c.mu.Lock()
	defer c.mu.Unlock()

	if err := c.failOn[frame.ID]; err != nil {
		return proto.Frame{}, err
	}

	c.recorded = append(c.recorded, frame)

	decision := c.decisions[frame.ID]
	if decision == "" {
		decision = "accept"
	}
	if frame.Reason != "" {
		decision = "reject"
	}

	switch frame.Kind {
	case proto.KindJobAssigned, proto.KindAcquireOutcome:
		if decision != "reject" && frame.JobID != "" {
			c.runningVMs[frame.JobID] = struct{}{}
		}
	case proto.KindVmDelete:
		delete(c.runningVMs, frame.JobID)
		c.deletedVMs = append(c.deletedVMs, frame.JobID)
	}

	ack := proto.AckFor(frame.ID, decision)
	c.ackedBefore = append(c.ackedBefore, frame.ID)
	return ack, nil
}

func (c *RecordingCoordinator) Recorded() []proto.Frame {
	c.mu.Lock()
	defer c.mu.Unlock()
	out := make([]proto.Frame, len(c.recorded))
	copy(out, c.recorded)
	return out
}

func (c *RecordingCoordinator) AckOrder() []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	out := make([]string, len(c.ackedBefore))
	copy(out, c.ackedBefore)
	return out
}

func (c *RecordingCoordinator) RunningVMCount() int {
	c.mu.Lock()
	defer c.mu.Unlock()
	return len(c.runningVMs)
}

func (c *RecordingCoordinator) DeletedVMs() []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	out := make([]string, len(c.deletedVMs))
	copy(out, c.deletedVMs)
	return out
}

func (c *RecordingCoordinator) KindsRecorded() []string {
	c.mu.Lock()
	defer c.mu.Unlock()
	out := make([]string, 0, len(c.recorded))
	for _, f := range c.recorded {
		out = append(out, f.Kind)
	}
	return out
}
