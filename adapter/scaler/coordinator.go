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

type CommandHandler func(context.Context, proto.Frame) proto.Frame

type FramedCoordinator struct {
	in         io.Reader
	out        io.Writer
	writeMu    sync.Mutex
	mu         sync.Mutex
	pending    map[string]chan proto.Frame
	readErr    error
	done       chan struct{}
	once       sync.Once
	handler    CommandHandler
	ctx        context.Context
	commands   chan proto.Frame
	workerDone chan struct{}
	writeErr   error
	stopped    bool
}

func NewFramedCoordinator(in io.Reader, out io.Writer) *FramedCoordinator {
	return &FramedCoordinator{in: in, out: out, pending: make(map[string]chan proto.Frame), done: make(chan struct{}), ctx: context.Background()}
}

func (c *FramedCoordinator) Start(ctx context.Context, handler CommandHandler) {
	c.once.Do(func() {
		c.ctx = ctx
		c.handler = handler
		if handler != nil {
			c.commands = make(chan proto.Frame, 64)
			c.workerDone = make(chan struct{})
			go c.handleCommands()
		}
		go c.read()
	})
}

func (c *FramedCoordinator) Write(frame proto.Frame) error {
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	return proto.WriteFrame(c.out, &frame)
}

func (c *FramedCoordinator) read() {
	defer close(c.done)
	for {
		frame, err := proto.ReadFrame(c.in)
		if err != nil {
			c.mu.Lock()
			c.readErr = err
			c.mu.Unlock()
			return
		}
		if frame.IsAck() {
			c.mu.Lock()
			ch := c.pending[frame.EventID]
			c.mu.Unlock()
			if ch != nil {
				select {
				case ch <- *frame:
				default:
				}
			}
			continue
		}
		if c.handler == nil {
			c.mu.Lock()
			c.readErr = fmt.Errorf("coordinator: unexpected frame %q", frame.Kind)
			c.mu.Unlock()
			return
		}
		c.mu.Lock()
		if c.stopped {
			c.mu.Unlock()
			return
		}
		select {
		case c.commands <- *frame:
			c.mu.Unlock()
		default:
			c.readErr = fmt.Errorf("coordinator: command queue full")
			c.mu.Unlock()
			return
		}
	}
}

func (c *FramedCoordinator) handleCommands() {
	defer close(c.workerDone)
	for frame := range c.commands {
		result := c.handler(c.ctx, frame)
		if err := c.Write(result); err != nil {
			c.mu.Lock()
			c.writeErr = err
			c.mu.Unlock()
			return
		}
	}
}

func (c *FramedCoordinator) Stop() error {
	c.mu.Lock()
	c.stopped = true
	if c.commands != nil {
		close(c.commands)
	}
	c.mu.Unlock()
	if c.workerDone != nil {
		<-c.workerDone
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.writeErr
}

func (c *FramedCoordinator) Submit(ctx context.Context, frame proto.Frame) (proto.Frame, error) {
	ch := make(chan proto.Frame, 1)
	c.mu.Lock()
	if _, exists := c.pending[frame.ID]; exists {
		c.mu.Unlock()
		return proto.Frame{}, fmt.Errorf("coordinator: duplicate event %q", frame.ID)
	}
	c.pending[frame.ID] = ch
	c.mu.Unlock()
	defer func() { c.mu.Lock(); delete(c.pending, frame.ID); c.mu.Unlock() }()
	c.Start(ctx, nil)
	if err := c.Write(frame); err != nil {
		return proto.Frame{}, fmt.Errorf("coordinator: write event: %w", err)
	}
	select {
	case ack := <-ch:
		if ack.Recorded == nil || !*ack.Recorded {
			return proto.Frame{}, fmt.Errorf("coordinator: ack for %q was not recorded durably", frame.ID)
		}
		return ack, nil
	case <-ctx.Done():
		return proto.Frame{}, ctx.Err()
	case <-c.done:
		select {
		case ack := <-ch:
			if ack.Recorded == nil || !*ack.Recorded {
				return proto.Frame{}, fmt.Errorf("coordinator: ack for %q was not recorded durably", frame.ID)
			}
			return ack, nil
		default:
		}
		c.mu.Lock()
		err := c.readErr
		c.mu.Unlock()
		return proto.Frame{}, fmt.Errorf("coordinator: read ack: %w", err)
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
