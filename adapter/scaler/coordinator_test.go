package scaler

import (
	"bytes"
	"context"
	"strings"
	"testing"

	"github.com/joshuarli/smolrunners/adapter/proto"
)

func SubmitWithAck(frameID string, ack proto.Frame) (*proto.Frame, error) {
	var input bytes.Buffer
	if err := proto.WriteFrame(&input, &ack); err != nil {
		return nil, err
	}
	coordinator := NewFramedCoordinator(&input, &bytes.Buffer{})
	event := proto.Frame{Kind: proto.KindJobAssigned, SchemaVersion: proto.SchemaVersion, ID: frameID, RequestID: proto.Int64(1), JobID: "job-1"}
	return submitAndReturn(coordinator, event)
}

func TestAckRequiresRecordedTrue(t *testing.T) {
	falseValue := false
	bad := proto.Frame{Kind: proto.KindAck, SchemaVersion: proto.SchemaVersion, ID: "ack-1", EventID: "evt-1", Decision: "accepted", Recorded: &falseValue}
	_, err := SubmitWithAck("evt-1", bad)
	if err == nil || !strings.Contains(err.Error(), "not recorded durably") {
		t.Fatalf("expected unrecorded ack to be rejected, got %v", err)
	}
}

func TestAckAcceptsRecordedTrue(t *testing.T) {
	good := proto.Frame{Kind: proto.KindAck, SchemaVersion: proto.SchemaVersion, ID: "ack-1", EventID: "evt-1", Decision: "accepted", Recorded: proto.Bool(true)}
	ack, err := SubmitWithAck("evt-1", good)
	if err != nil {
		t.Fatalf("recorded ack rejected: %v", err)
	}
	if ack.Recorded == nil || !*ack.Recorded {
		t.Fatal("ack did not carry recorded=true")
	}
}

func submitAndReturn(c *FramedCoordinator, event proto.Frame) (*proto.Frame, error) {
	result, err := c.Submit(context.Background(), event)
	if err != nil {
		return nil, err
	}
	return &result, nil
}
