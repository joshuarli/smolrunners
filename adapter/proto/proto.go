package proto

import (
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
)

const (
	SchemaVersion = 1
	MaxFrameBytes = 1 << 20
)

const (
	KindRun            = "run"
	KindCapacity       = "capacity"
	KindJobAvailable   = "job_available"
	KindJobAssigned    = "job_assigned"
	KindJobStarted     = "job_started"
	KindJobCompleted   = "job_completed"
	KindAcquireOutcome = "acquire_outcome"
	KindSession        = "session"
	KindVmDelete       = "vm_delete"
	KindAck            = "ack"
	KindJITRequest     = "jit_request"
	KindRemoveRunner   = "remove_runner"
	KindCommandResult  = "command_result"
)

type Frame struct {
	Kind             string   `json:"kind"`
	SchemaVersion    int      `json:"schema_version"`
	ID               string   `json:"id"`
	EventID          string   `json:"event_id,omitempty"`
	Decision         string   `json:"decision,omitempty"`
	Recorded         *bool    `json:"recorded,omitempty"`
	AssignedJobs     *int64   `json:"assigned_jobs,omitempty"`
	ReportedCapacity *int64   `json:"reported_capacity,omitempty"`
	RequestID        *int64   `json:"request_id,omitempty"`
	JobID            string   `json:"job_id,omitempty"`
	Labels           []string `json:"labels,omitempty"`
	RunnerID         *int64   `json:"runner_id,omitempty"`
	RunnerName       string   `json:"runner_name,omitempty"`
	Result           string   `json:"result,omitempty"`
	RequestIDs       []int64  `json:"request_ids,omitempty"`
	AcquiredIDs      []int64  `json:"acquired_ids,omitempty"`
	SessionEvent     string   `json:"session_event,omitempty"`
	RetryAfterMs     *int64   `json:"retry_after_ms,omitempty"`
	Reason           string   `json:"reason,omitempty"`
	Scenario         string   `json:"scenario,omitempty"`
	CommandID        string   `json:"command_id,omitempty"`
	OK               *bool    `json:"ok,omitempty"`
	Error            string   `json:"error,omitempty"`
	EncodedJITConfig string   `json:"encoded_jit_config,omitempty"`
}

func Bool(v bool) *bool    { return &v }
func Int64(v int64) *int64 { return &v }

func (f *Frame) CorrelationID() string {
	if f.Kind == KindAck {
		return f.EventID
	}
	return f.ID
}

func (f *Frame) IsAck() bool {
	return f.Kind == KindAck
}

var (
	ErrEmptyFrame      = errors.New("proto: empty frame")
	ErrFrameTooLarge   = errors.New("proto: frame exceeds 1 MiB")
	ErrBadSchema       = errors.New("proto: unsupported schema_version")
	ErrTruncatedPrefix = errors.New("proto: truncated length prefix")
	ErrTruncatedFrame  = errors.New("proto: truncated frame body")
)

func validate(f *Frame) error {
	if f == nil {
		return ErrEmptyFrame
	}
	if f.SchemaVersion != SchemaVersion {
		return fmt.Errorf("%w: got %d, want %d", ErrBadSchema, f.SchemaVersion, SchemaVersion)
	}
	if f.Kind == "" {
		return errors.New("proto: missing kind")
	}
	return nil
}

func EncodeFrame(f *Frame) ([]byte, error) {
	if err := validate(f); err != nil {
		return nil, err
	}
	body, err := json.Marshal(f)
	if err != nil {
		return nil, fmt.Errorf("proto: marshal: %w", err)
	}
	if len(body) == 0 {
		return nil, ErrEmptyFrame
	}
	if len(body) > MaxFrameBytes {
		return nil, fmt.Errorf("%w: got %d bytes", ErrFrameTooLarge, len(body))
	}
	out := make([]byte, 4+len(body))
	binary.BigEndian.PutUint32(out[:4], uint32(len(body)))
	copy(out[4:], body)
	return out, nil
}

func DecodeFrame(data []byte) (*Frame, error) {
	if len(data) < 4 {
		return nil, ErrTruncatedPrefix
	}
	n := binary.BigEndian.Uint32(data[:4])
	if n == 0 {
		return nil, ErrEmptyFrame
	}
	if n > MaxFrameBytes {
		return nil, fmt.Errorf("%w: got %d bytes", ErrFrameTooLarge, n)
	}
	if uint64(len(data)-4) < uint64(n) {
		return nil, ErrTruncatedFrame
	}
	var f Frame
	if err := json.Unmarshal(data[4:4+n], &f); err != nil {
		return nil, fmt.Errorf("proto: unmarshal: %w", err)
	}
	if err := validate(&f); err != nil {
		return nil, err
	}
	return &f, nil
}

func WriteFrame(w io.Writer, f *Frame) error {
	data, err := EncodeFrame(f)
	if err != nil {
		return err
	}
	_, err = w.Write(data)
	return err
}

func ReadFrame(r io.Reader) (*Frame, error) {
	var prefix [4]byte
	if _, err := io.ReadFull(r, prefix[:]); err != nil {
		if errors.Is(err, io.EOF) {
			return nil, io.EOF
		}
		return nil, fmt.Errorf("%w: %v", ErrTruncatedPrefix, err)
	}
	n := binary.BigEndian.Uint32(prefix[:])
	if n == 0 {
		return nil, ErrEmptyFrame
	}
	if n > MaxFrameBytes {
		return nil, fmt.Errorf("%w: got %d bytes", ErrFrameTooLarge, n)
	}
	body := make([]byte, n)
	if _, err := io.ReadFull(r, body); err != nil {
		return nil, fmt.Errorf("%w: %v", ErrTruncatedFrame, err)
	}
	var f Frame
	if err := json.Unmarshal(body, &f); err != nil {
		return nil, fmt.Errorf("proto: unmarshal: %w", err)
	}
	if err := validate(&f); err != nil {
		return nil, err
	}
	return &f, nil
}

func AckFor(eventID string, decision string) Frame {
	return Frame{
		Kind:          KindAck,
		SchemaVersion: SchemaVersion,
		ID:            "ack-" + eventID,
		EventID:       eventID,
		Decision:      decision,
		Recorded:      Bool(true),
	}
}
